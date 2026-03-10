//! RunPipelineTool — implements `crew_agent::Tool` to expose pipeline execution.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use crew_agent::{Tool, ToolPolicy, ToolResult};
use crew_llm::{LlmProvider, ProviderRouter};
use crew_memory::EpisodeStore;
use eyre::{Result, WrapErr};
use serde::Deserialize;

use crate::discovery::PipelineDiscovery;
use crate::executor::{ExecutorConfig, PipelineExecutor, PipelineStatusBridge};

/// Tool that runs DOT-based pipelines.
pub struct RunPipelineTool {
    default_provider: Arc<dyn LlmProvider>,
    provider_router: Option<Arc<ProviderRouter>>,
    memory: Arc<EpisodeStore>,
    working_dir: PathBuf,
    provider_policy: Option<ToolPolicy>,
    plugin_dirs: Vec<PathBuf>,
    discovery: PipelineDiscovery,
    /// Per-message status bridge (set via `set_status_bridge` before each call).
    status_bridge: std::sync::Mutex<Option<PipelineStatusBridge>>,
}

impl RunPipelineTool {
    pub fn new(
        default_provider: Arc<dyn LlmProvider>,
        memory: Arc<EpisodeStore>,
        working_dir: PathBuf,
        data_dir: PathBuf,
    ) -> Self {
        let discovery = PipelineDiscovery::new(&data_dir, &working_dir);
        Self {
            default_provider,
            provider_router: None,
            memory,
            working_dir,
            provider_policy: None,
            plugin_dirs: Vec::new(),
            discovery,
            status_bridge: std::sync::Mutex::new(None),
        }
    }

    /// Add the global crew-home skills directory as a search path.
    /// This ensures pipelines installed globally (e.g. `~/.crew/skills/`) are
    /// discoverable even when data_dir is per-profile.
    pub fn with_crew_home(mut self, crew_home: PathBuf) -> Self {
        self.discovery.add_search_path(crew_home.join("skills"));
        self
    }

    pub fn with_provider_router(mut self, router: Arc<ProviderRouter>) -> Self {
        self.provider_router = Some(router);
        self
    }

    pub fn with_provider_policy(mut self, policy: Option<ToolPolicy>) -> Self {
        self.provider_policy = policy;
        self
    }

    pub fn with_plugin_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.plugin_dirs = dirs;
        self
    }

    /// Set the status bridge for the current message.
    /// Called per-message to connect pipeline progress to the messaging channel's
    /// StatusIndicator (status words + token tracker).
    pub fn set_status_bridge(&self, bridge: PipelineStatusBridge) {
        *self.status_bridge.lock().unwrap_or_else(|e| e.into_inner()) = Some(bridge);
    }
}

#[derive(Deserialize)]
struct Input {
    pipeline: String,
    input: String,
    #[serde(default)]
    variables: serde_json::Map<String, serde_json::Value>,
}

#[async_trait]
impl Tool for RunPipelineTool {
    fn name(&self) -> &str {
        "run_pipeline"
    }

    fn description(&self) -> &str {
        "Execute a multi-step pipeline defined as a DOT graph. Each node runs a \
         specialized agent with its own prompt and model. Use for complex workflows \
         like deep research (search -> analyze -> synthesize)."
    }

    fn tags(&self) -> &[&str] {
        &["gateway"]
    }

    fn input_schema(&self) -> serde_json::Value {
        let available = self.discovery.list_available();
        let pipeline_desc = if available.is_empty() {
            "Pipeline name (built-in) or path to a .dot file.".to_string()
        } else {
            let names: Vec<&str> = available.iter().map(|p| p.name.as_str()).collect();
            format!(
                "Pipeline name or path to .dot file. Available: {}",
                names.join(", ")
            )
        };

        serde_json::json!({
            "type": "object",
            "properties": {
                "pipeline": {
                    "type": "string",
                    "description": pipeline_desc
                },
                "input": {
                    "type": "string",
                    "description": "The input query or task description for the pipeline"
                },
                "variables": {
                    "type": "object",
                    "description": "Optional key-value pairs for template substitution in node prompts",
                    "additionalProperties": { "type": "string" }
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds. Estimate based on real execution times: simple 2-node pipeline ~3min → 300s; standard 3-node research pipeline ~8min → 600s; 5-7 topic deep research with crawl+synthesize ~15-20min → 1200s; complex multi-source analysis with many nodes ~25min → 1500s. Max: 1800. Default: 600"
                }
            },
            "required": ["pipeline", "input"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: Input =
            serde_json::from_value(args.clone()).wrap_err("invalid run_pipeline input")?;

        let dot_content = self.discovery.resolve(&input.pipeline).await?;

        let status_bridge = self
            .status_bridge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        let config = ExecutorConfig {
            default_provider: self.default_provider.clone(),
            provider_router: self.provider_router.clone(),
            memory: self.memory.clone(),
            working_dir: self.working_dir.clone(),
            provider_policy: self.provider_policy.clone(),
            plugin_dirs: self.plugin_dirs.clone(),
            status_bridge,
        };

        let executor = PipelineExecutor::new(config);
        let result = executor
            .run(&dot_content, &input.input, &input.variables)
            .await?;

        let summary = result
            .node_summaries
            .iter()
            .map(|n| {
                format!(
                    "- {} ({}): {}ms, {}+{} tokens",
                    n.node_id,
                    n.model.as_deref().unwrap_or("default"),
                    n.duration_ms,
                    n.token_usage.input_tokens,
                    n.token_usage.output_tokens,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolResult {
            output: format!(
                "{}\n\n---\nPipeline execution summary:\n{summary}\nTotal: {} input + {} output tokens",
                result.output, result.token_usage.input_tokens, result.token_usage.output_tokens,
            ),
            success: result.success,
            tokens_used: Some(result.token_usage),
            ..Default::default()
        })
    }
}
