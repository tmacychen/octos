//! Spawn tool for background subagent execution.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use crew_core::{AgentId, InboundMessage, Task, TaskContext, TaskKind};
use crew_llm::LlmProvider;
use crew_memory::EpisodeStore;
use eyre::{Result, WrapErr};
use serde::Deserialize;
use tracing::{info, warn};

use super::{Tool, ToolPolicy, ToolRegistry, ToolResult};
use crate::Agent;

/// Tool that spawns background worker agents for long-running tasks.
pub struct SpawnTool {
    llm: Arc<dyn LlmProvider>,
    memory: Arc<EpisodeStore>,
    working_dir: PathBuf,
    inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    origin: std::sync::Mutex<(String, String)>,
    worker_count: AtomicU32,
}

impl SpawnTool {
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        memory: Arc<EpisodeStore>,
        working_dir: PathBuf,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Self {
        Self {
            llm,
            memory,
            working_dir,
            inbound_tx,
            origin: std::sync::Mutex::new(("cli".into(), "default".into())),
            worker_count: AtomicU32::new(0),
        }
    }

    /// Update the origin context for result delivery (called per inbound message).
    pub fn set_context(&self, channel: &str, chat_id: &str) {
        *self.origin.lock().unwrap() = (channel.to_string(), chat_id.to_string());
    }
}

#[derive(Deserialize)]
struct Input {
    task: String,
    #[serde(default)]
    label: Option<String>,
    /// "background" (default) or "sync".
    #[serde(default = "default_mode")]
    mode: String,
    /// Tool names the subagent is allowed to use. Empty = all builtins.
    #[serde(default)]
    allowed_tools: Vec<String>,
    /// Extra context injected as a system-level prefix.
    #[serde(default)]
    context: Option<String>,
}

fn default_mode() -> String {
    "background".into()
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to work on a task. Use mode='sync' to wait for the result, or 'background' (default) for fire-and-forget."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task for the subagent to complete"
                },
                "label": {
                    "type": "string",
                    "description": "Optional short label for display"
                },
                "mode": {
                    "type": "string",
                    "enum": ["background", "sync"],
                    "description": "background: returns immediately, result announced later. sync: waits and returns the result.",
                    "default": "background"
                },
                "allowed_tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tool names the subagent may use. Empty = all builtins."
                },
                "context": {
                    "type": "string",
                    "description": "Extra context prepended to the task prompt."
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: Input =
            serde_json::from_value(args.clone()).wrap_err("invalid spawn tool input")?;

        let worker_num = self.worker_count.fetch_add(1, Ordering::SeqCst);
        let worker_id = AgentId::new(format!("subagent-{worker_num}"));
        let label = input.label.unwrap_or_else(|| input.task.chars().take(60).collect());

        // Build the task prompt (optionally prepend context)
        let task_desc = match &input.context {
            Some(ctx) => format!("{ctx}\n\n{}", input.task),
            None => input.task.clone(),
        };

        let allowed_tools = input.allowed_tools.clone();
        let is_sync = input.mode == "sync";

        info!(
            worker_id = %worker_id,
            mode = %input.mode,
            task = %input.task,
            "spawning subagent"
        );

        if is_sync {
            // Sync mode: run subagent inline and return the result directly
            let mut tools = ToolRegistry::with_builtins(&self.working_dir);
            let policy = ToolPolicy {
                allow: allowed_tools,
                deny: vec!["spawn".into()],
            };
            tools.apply_policy(&policy);
            let worker = Agent::new(worker_id, self.llm.clone(), tools, self.memory.clone());

            let subtask = Task::new(
                TaskKind::Code {
                    instruction: task_desc.clone(),
                    files: vec![],
                },
                TaskContext {
                    working_dir: self.working_dir.clone(),
                    ..Default::default()
                },
            );

            let result = worker.run_task(&subtask).await;
            match result {
                Ok(r) => Ok(ToolResult {
                    output: r.output,
                    success: r.success,
                    tokens_used: Some(r.token_usage),
                    ..Default::default()
                }),
                Err(e) => Ok(ToolResult {
                    output: format!("Subagent failed: {e}"),
                    success: false,
                    ..Default::default()
                }),
            }
        } else {
            // Background mode: fire-and-forget
            let (origin_channel, origin_chat_id) = self.origin.lock().unwrap().clone();
            let llm = self.llm.clone();
            let memory = self.memory.clone();
            let working_dir = self.working_dir.clone();
            let inbound_tx = self.inbound_tx.clone();
            let wid = worker_id.clone();

            tokio::spawn(async move {
                let mut tools = ToolRegistry::with_builtins(&working_dir);
                let policy = ToolPolicy {
                    allow: allowed_tools,
                    deny: vec!["spawn".into()],
                };
                tools.apply_policy(&policy);
                let worker = Agent::new(wid.clone(), llm, tools, memory);

                let subtask = Task::new(
                    TaskKind::Code {
                        instruction: task_desc.clone(),
                        files: vec![],
                    },
                    TaskContext {
                        working_dir,
                        ..Default::default()
                    },
                );

                let result = worker.run_task(&subtask).await;

                let content = match &result {
                    Ok(r) => format!(
                        "[Subagent {} completed]\nTask: {}\nStatus: {}\n\nResult:\n{}\n\nPlease summarize this result naturally for the user.",
                        wid,
                        task_desc,
                        if r.success { "SUCCESS" } else { "FAILED" },
                        r.output
                    ),
                    Err(e) => format!(
                        "[Subagent {} failed]\nTask: {}\nError: {e}\n\nPlease inform the user about this failure.",
                        wid, task_desc
                    ),
                };

                let announce = InboundMessage {
                    channel: "system".into(),
                    sender_id: "subagent".into(),
                    chat_id: format!("{origin_channel}:{origin_chat_id}"),
                    content,
                    timestamp: chrono::Utc::now(),
                    media: vec![],
                    metadata: serde_json::json!({
                        "deliver_to_channel": origin_channel,
                        "deliver_to_chat_id": origin_chat_id,
                    }),
                };

                if let Err(e) = inbound_tx.send(announce).await {
                    warn!(error = %e, "failed to announce subagent result");
                }
            });

            Ok(ToolResult {
                output: format!("Spawned background task: {label}"),
                success: true,
                ..Default::default()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_returns_immediately() {
        let (in_tx, _in_rx) = tokio::sync::mpsc::channel(16);

        // We can't easily create a real LLM + EpisodeStore for unit tests,
        // so just test the worker count and basic input parsing.
        let tool = SpawnTool {
            llm: Arc::new(MockProvider),
            memory: Arc::new(create_test_store().await),
            working_dir: PathBuf::from("/tmp"),
            inbound_tx: in_tx,
            origin: std::sync::Mutex::new(("cli".into(), "test".into())),
            worker_count: AtomicU32::new(0),
        };

        assert_eq!(tool.worker_count.load(Ordering::SeqCst), 0);

        // Invalid input test
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_err());

        // Worker count should not increment on invalid input
        assert_eq!(tool.worker_count.load(Ordering::SeqCst), 0);
    }

    // Minimal mock provider for testing
    struct MockProvider;

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn chat(
            &self,
            _messages: &[crew_core::Message],
            _tools: &[crew_llm::ToolSpec],
            _config: &crew_llm::ChatConfig,
        ) -> Result<crew_llm::ChatResponse> {
            Ok(crew_llm::ChatResponse {
                content: Some("done".into()),
                tool_calls: vec![],
                stop_reason: crew_llm::StopReason::EndTurn,
                usage: crew_llm::TokenUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            })
        }

        fn model_id(&self) -> &str {
            "mock"
        }

        fn provider_name(&self) -> &str {
            "mock"
        }
    }

    async fn create_test_store() -> EpisodeStore {
        let dir = tempfile::tempdir().unwrap();
        // Leak the dir so it stays alive for the test
        let dir = Box::leak(Box::new(dir));
        EpisodeStore::open(dir.path()).await.unwrap()
    }
}
