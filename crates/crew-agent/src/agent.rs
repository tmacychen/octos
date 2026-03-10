//! Agent implementation.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crew_core::{AgentId, Message, MessageRole, Task, TaskResult, TokenUsage};
use crew_llm::{
    ChatConfig, ChatResponse, ChatStream, EmbeddingProvider, LlmProvider, StopReason, StreamEvent,
    ToolSpec,
};
use crew_memory::{Episode, EpisodeOutcome, EpisodeStore};
use eyre::Result;
use futures::StreamExt;
use tracing::{Instrument, debug, info, info_span, warn};

use crate::hooks::{HookContext, HookEvent, HookExecutor, HookPayload, HookResult};
use crate::loop_detect::LoopDetector;
use crate::progress::{ProgressEvent, ProgressReporter, SilentReporter};
use crate::tools::{ToolContext, ToolRegistry, TOOL_CTX};

tokio::task_local! {
    /// Task-local reporter override.  When set (via `TASK_REPORTER.scope()`),
    /// `Agent::reporter()` returns this instead of the instance-level RwLock
    /// reporter.  This lets concurrent overflow tasks each have their own
    /// stream reporter without mutating the shared `Arc<Agent>`.
    pub static TASK_REPORTER: Arc<dyn ProgressReporter>;
}

/// Compiled-in default worker prompt (from `prompts/worker.txt`).
pub const DEFAULT_WORKER_PROMPT: &str = include_str!("prompts/worker.txt");

/// Configuration for agent execution.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Maximum number of iterations before stopping.
    pub max_iterations: u32,
    /// Maximum total tokens (input + output) before stopping. None = unlimited.
    pub max_tokens: Option<u32>,
    /// Wall-clock timeout for the entire agent run. None = unlimited.
    pub max_timeout: Option<Duration>,
    /// Whether to save episodes to memory.
    pub save_episodes: bool,
    /// Optional worker system prompt override (used by Agent::new as the default prompt).
    /// When None, falls back to the compiled-in prompts/worker.txt.
    pub worker_prompt: Option<String>,
    /// Maximum seconds for all parallel tool calls to complete. Default: 300.
    pub tool_timeout_secs: u64,
}

/// Default tool execution timeout in seconds.
pub const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 600;
/// Maximum tool timeout the LLM can request (30 minutes).
pub const MAX_TOOL_TIMEOUT_SECS: u64 = 1800;
/// Default session processing timeout in seconds.
pub const DEFAULT_SESSION_TIMEOUT_SECS: u64 = 1800;

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            max_tokens: None,
            max_timeout: Some(Duration::from_secs(600)),
            save_episodes: true,
            worker_prompt: None,
            tool_timeout_secs: DEFAULT_TOOL_TIMEOUT_SECS,
        }
    }
}

/// Response from conversation mode (process_message).
#[derive(Debug, Clone)]
pub struct ConversationResponse {
    pub content: String,
    pub token_usage: TokenUsage,
    pub files_modified: Vec<PathBuf>,
    pub streamed: bool,
    /// All messages generated during processing (assistant replies, tool calls,
    /// tool results). Includes the user message at the front. Callers should
    /// persist these to session history so subsequent calls see the full context.
    pub messages: Vec<Message>,
}

/// Shared atomic counters for real-time token tracking (used by status indicators).
pub struct TokenTracker {
    pub input_tokens: AtomicU32,
    pub output_tokens: AtomicU32,
}

impl TokenTracker {
    pub fn new() -> Self {
        Self {
            input_tokens: AtomicU32::new(0),
            output_tokens: AtomicU32::new(0),
        }
    }
}

impl Default for TokenTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Reason why the agent loop stopped due to budget constraints.
enum BudgetStop {
    Shutdown,
    MaxIterations,
    MaxTokens { used: u32, limit: u32 },
    WallClockTimeout { limit: Duration },
}

impl BudgetStop {
    fn message(&self) -> String {
        match self {
            Self::Shutdown => "Interrupted.".into(),
            Self::MaxIterations => "Reached max iterations.".into(),
            Self::MaxTokens { used, limit } => {
                format!("Token budget exceeded ({used} of {limit}).")
            }
            Self::WallClockTimeout { limit } => {
                format!("Wall-clock timeout ({:.0}s limit).", limit.as_secs_f64())
            }
        }
    }
}

/// An agent that can execute tasks.
pub struct Agent {
    /// Unique identifier for this agent.
    pub id: AgentId,
    /// LLM provider for generating responses.
    llm: Arc<dyn LlmProvider>,
    /// Tool registry for executing tool calls (Arc for sharing with spawned tool tasks).
    tools: Arc<ToolRegistry>,
    /// Episode store for memory.
    memory: Arc<EpisodeStore>,
    /// Embedding provider for hybrid memory search.
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// System prompt for this agent (RwLock for hot-reload support).
    system_prompt: RwLock<String>,
    /// Agent configuration.
    config: AgentConfig,
    /// Progress reporter (RwLock for interior-mutable swap without &mut self).
    reporter: RwLock<Arc<dyn ProgressReporter>>,
    /// Lifecycle hooks executor.
    hooks: Option<Arc<HookExecutor>>,
    /// Session-level context for hook payloads.
    hook_context: std::sync::Mutex<Option<HookContext>>,
    /// Shutdown signal.
    shutdown: Arc<AtomicBool>,
}

impl Agent {
    /// Create a new agent.
    pub fn new(
        id: AgentId,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        memory: Arc<EpisodeStore>,
    ) -> Self {
        let system_prompt = include_str!("prompts/worker.txt").to_string();

        Self {
            id,
            llm,
            tools: Arc::new(tools),
            memory,
            embedder: None,
            system_prompt: RwLock::new(system_prompt),
            config: AgentConfig::default(),
            reporter: RwLock::new(Arc::new(SilentReporter)),
            hooks: None,
            hook_context: std::sync::Mutex::new(None),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a new agent sharing pre-existing Arc-wrapped resources.
    /// Useful for per-request agents that share tools/memory with a base agent.
    pub fn new_shared(
        id: AgentId,
        llm: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        memory: Arc<EpisodeStore>,
    ) -> Self {
        let system_prompt = include_str!("prompts/worker.txt").to_string();

        Self {
            id,
            llm,
            tools,
            memory,
            embedder: None,
            system_prompt: RwLock::new(system_prompt),
            config: AgentConfig::default(),
            reporter: RwLock::new(Arc::new(SilentReporter)),
            hooks: None,
            hook_context: std::sync::Mutex::new(None),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the agent configuration.
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        // Apply worker_prompt override if provided
        if let Some(ref wp) = config.worker_prompt {
            *self
                .system_prompt
                .write()
                .unwrap_or_else(|e| e.into_inner()) = wp.clone();
        }
        self.config = config;
        self
    }

    /// Set the progress reporter.
    pub fn with_reporter(self, reporter: Arc<dyn ProgressReporter>) -> Self {
        *self.reporter.write().unwrap_or_else(|e| e.into_inner()) = reporter;
        self
    }

    /// Replace the progress reporter at runtime (e.g. per-message stream reporter).
    /// Takes `&self` (not `&mut self`) — uses interior mutability via RwLock so
    /// the agent can be behind an Arc for concurrent speculative overflow.
    pub fn set_reporter(&self, reporter: Arc<dyn ProgressReporter>) {
        *self.reporter.write().unwrap_or_else(|e| e.into_inner()) = reporter;
    }

    /// Get a clone of the current reporter.
    ///
    /// Checks `TASK_REPORTER` task-local first (set per-overflow-task), then
    /// falls back to the instance-level RwLock reporter.
    fn reporter(&self) -> Arc<dyn ProgressReporter> {
        TASK_REPORTER
            .try_with(|r| r.clone())
            .unwrap_or_else(|_| {
                self.reporter
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            })
    }

    /// Set the shutdown signal.
    pub fn with_shutdown(mut self, shutdown: Arc<AtomicBool>) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Set the embedding provider for hybrid memory search.
    pub fn with_embedder(mut self, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Set lifecycle hooks executor.
    pub fn with_hooks(mut self, hooks: Arc<HookExecutor>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Set session-level context for hook payloads.
    pub fn with_hook_context(self, ctx: HookContext) -> Self {
        *self.hook_context.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx);
        self
    }

    /// Update the session ID in the hook context (call before each message).
    pub fn set_session_id(&self, session_id: &str) {
        let mut guard = self.hook_context.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut ctx) = *guard {
            ctx.session_id = Some(session_id.to_string());
        }
    }

    /// Get a snapshot of the current hook context.
    fn hook_ctx(&self) -> Option<HookContext> {
        self.hook_context
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Override the system prompt (e.g. for gateway mode).
    pub fn with_system_prompt(self, prompt: String) -> Self {
        *self.system_prompt.write().unwrap_or_else(|e| {
            tracing::warn!("system prompt lock was poisoned, recovering");
            e.into_inner()
        }) = prompt;
        self
    }

    /// Append additional content to the current system prompt (e.g. bootstrap files).
    pub fn append_system_prompt(&self, extra: &str) {
        let mut guard = self.system_prompt.write().unwrap_or_else(|e| {
            tracing::warn!("system prompt lock was poisoned, recovering");
            e.into_inner()
        });
        guard.push_str("\n\n");
        guard.push_str(extra);
    }

    /// Update the system prompt at runtime (hot-reload).
    pub fn set_system_prompt(&self, prompt: String) {
        *self.system_prompt.write().unwrap_or_else(|e| {
            tracing::warn!("system prompt lock was poisoned, recovering");
            e.into_inner()
        }) = prompt;
    }

    /// The LLM model ID in use.
    pub fn model_id(&self) -> &str {
        self.llm.model_id()
    }

    /// The LLM provider name in use.
    pub fn provider_name(&self) -> &str {
        self.llm.provider_name()
    }

    /// Get a reference to the LLM provider (for sharing with per-request agents).
    pub fn llm_provider(&self) -> Arc<dyn LlmProvider> {
        self.llm.clone()
    }

    /// Get a reference to the tool registry.
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    /// Get a reference to the episode store.
    pub fn memory_store(&self) -> Arc<EpisodeStore> {
        self.memory.clone()
    }

    /// Get a clone of the agent config.
    pub fn agent_config(&self) -> AgentConfig {
        self.config.clone()
    }

    /// Get a snapshot of the current system prompt.
    pub fn system_prompt_snapshot(&self) -> String {
        self.system_prompt
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Process a single message in conversation mode (chat/gateway).
    /// Takes the user's message, conversation history, and optional media paths.
    pub async fn process_message(
        &self,
        user_content: &str,
        history: &[Message],
        media: Vec<String>,
    ) -> Result<ConversationResponse> {
        self.process_message_inner(user_content, history, media, None)
            .await
    }

    /// Like `process_message`, but updates a `TokenTracker` in real-time after each LLM call.
    /// Used by the gateway status indicator to show live token counts.
    pub async fn process_message_tracked(
        &self,
        user_content: &str,
        history: &[Message],
        media: Vec<String>,
        tracker: &TokenTracker,
    ) -> Result<ConversationResponse> {
        self.process_message_inner(user_content, history, media, Some(tracker))
            .await
    }

    async fn process_message_inner(
        &self,
        user_content: &str,
        history: &[Message],
        media: Vec<String>,
        tracker: Option<&TokenTracker>,
    ) -> Result<ConversationResponse> {
        let mut messages = vec![Message {
            role: MessageRole::System,
            content: self
                .system_prompt
                .read()
                .unwrap_or_else(|e| {
                    tracing::warn!("system prompt lock was poisoned, recovering");
                    e.into_inner()
                })
                .clone(),
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        }];

        messages.extend_from_slice(history);

        let content = if user_content.is_empty() && !media.is_empty() {
            "[User sent an image]".to_string()
        } else {
            user_content.to_string()
        };

        messages.push(Message {
            role: MessageRole::User,
            content,
            media,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        });

        let config = ChatConfig::default();
        let mut total_usage = TokenUsage::default();
        let mut files_modified = Vec::new();
        let mut iteration = 0u32;
        let start = Instant::now();
        let mut loop_detector = LoopDetector::new(12);

        loop {
            if let Some(stop) = self.check_budget(iteration, start, &total_usage) {
                // Skip system prompt + history; return only new messages
                let new_start = (1 + history.len()).min(messages.len());
                return Ok(ConversationResponse {
                    content: stop.message(),
                    token_usage: total_usage,
                    files_modified,
                    streamed: false,
                    messages: messages[new_start..].to_vec(),
                });
            }

            iteration += 1;
            let tools_spec = self.tools.specs();
            self.trim_to_context_window(&mut messages);
            normalize_system_messages(&mut messages);
            repair_message_order(&mut messages);
            repair_tool_pairs(&mut messages);
            truncate_old_tool_results(&mut messages);

            tracing::info!(
                iteration,
                messages = messages.len(),
                tools = tools_spec.len(),
                message_bytes = messages.iter().map(|m| m.content.len()).sum::<usize>(),
                "calling LLM"
            );
            let (response, streamed) = match self
                .call_llm_with_hooks(&messages, &tools_spec, &config, iteration, &total_usage)
                .await
            {
                Ok(r) => r,
                Err(e) if e.to_string().contains("empty response after") => {
                    // Empty response after retries — try once more (adaptive router
                    // may select a different provider on this second attempt).
                    warn!(error = %e, "retrying LLM call for adaptive failover");
                    self.reporter().report(ProgressEvent::LlmStatus {
                        message: "Switching provider...".to_string(),
                        iteration,
                    });
                    self.call_llm_with_hooks(
                        &messages, &tools_spec, &config, iteration, &total_usage,
                    )
                    .await?
                }
                Err(e) => return Err(e),
            };
            {
                let tool_names: Vec<&str> = response
                    .tool_calls
                    .iter()
                    .map(|tc| tc.name.as_str())
                    .collect();
                let tool_ids: Vec<&str> = response
                    .tool_calls
                    .iter()
                    .map(|tc| tc.id.as_str())
                    .collect();
                tracing::info!(
                    iteration,
                    stop_reason = ?response.stop_reason,
                    tool_calls = response.tool_calls.len(),
                    tool_names = %tool_names.join(", "),
                    tool_ids = %tool_ids.join(", "),
                    response_content_len = response.content.as_ref().map(|c| c.len()).unwrap_or(0),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    "LLM response received"
                );
            }
            total_usage.input_tokens += response.usage.input_tokens;
            total_usage.output_tokens += response.usage.output_tokens;
            if let Some(t) = tracker {
                t.input_tokens
                    .store(total_usage.input_tokens, Ordering::Relaxed);
                t.output_tokens
                    .store(total_usage.output_tokens, Ordering::Relaxed);
            }

            match response.stop_reason {
                StopReason::EndTurn | StopReason::StopSequence => {
                    self.emit_cost_update(&total_usage, &response.usage);
                    let new_start = (1 + history.len()).min(messages.len());
                    return Ok(ConversationResponse {
                        content: response.content.unwrap_or_default(),
                        token_usage: total_usage,
                        files_modified,
                        streamed,
                        messages: messages[new_start..].to_vec(),
                    });
                }
                StopReason::ToolUse => {
                    // Check for loop detection before executing
                    for tc in &response.tool_calls {
                        if let Some(warning) = loop_detector.record(&tc.name, &tc.arguments) {
                            warn!("loop detected in tool calls");
                            messages.push(Message {
                                role: MessageRole::System,
                                content: warning,
                                media: vec![],
                                tool_calls: None,
                                tool_call_id: None,
                                reasoning_content: None,
                                timestamp: chrono::Utc::now(),
                            });
                        }
                    }
                    self.handle_tool_use(
                        &response,
                        &mut messages,
                        &mut files_modified,
                        &mut total_usage,
                        tracker,
                    )
                    .await?;
                }
                StopReason::MaxTokens => {
                    self.emit_cost_update(&total_usage, &response.usage);
                    let new_start = (1 + history.len()).min(messages.len());
                    return Ok(ConversationResponse {
                        content: response.content.unwrap_or_default(),
                        token_usage: total_usage,
                        files_modified,
                        streamed,
                        messages: messages[new_start..].to_vec(),
                    });
                }
                StopReason::ContentFiltered => {
                    // After retries in call_llm_with_hooks, content is still filtered.
                    // Return a user-visible message instead of empty content.
                    self.emit_cost_update(&total_usage, &response.usage);
                    warn!("content filtered by provider safety/moderation after retries");
                    let new_start = (1 + history.len()).min(messages.len());
                    return Ok(ConversationResponse {
                        content: response.content.unwrap_or_else(|| {
                            "[Content was blocked by the model's safety filter. \
                             Please rephrase your request.]"
                                .to_string()
                        }),
                        token_usage: total_usage,
                        files_modified,
                        streamed,
                        messages: messages[new_start..].to_vec(),
                    });
                }
            }
        }
    }

    /// Run a task to completion (used by spawn tool).
    pub async fn run_task(&self, task: &Task) -> Result<TaskResult> {
        let task_start = Instant::now();
        let span = info_span!(
            "task",
            task_id = %task.id,
            agent_id = %self.id,
        );

        async {
            info!("starting task");
            self.reporter().report(ProgressEvent::TaskStarted {
                task_id: task.id.to_string(),
            });

            let mut iteration = 0u32;
            let mut messages = self.build_initial_messages(task).await;
            let mut total_usage = TokenUsage::default();
            let mut files_modified = Vec::new();
            let config = ChatConfig::default();

            loop {
                if let Some(stop) = self.check_budget(iteration, task_start, &total_usage) {
                    self.report_budget_stop(&stop, iteration);
                    return Ok(TaskResult {
                        success: false,
                        output: stop.message(),
                        files_modified,
                        subtasks: Vec::new(),
                        token_usage: total_usage,
                    });
                }

                iteration += 1;
                let iter_start = Instant::now();
                self.reporter()
                    .report(ProgressEvent::Thinking { iteration });

                let tools_spec = self.tools.specs();
                self.trim_to_context_window(&mut messages);

                let (response, _streamed) = self
                    .call_llm_with_hooks(&messages, &tools_spec, &config, iteration, &total_usage)
                    .await?;
                total_usage.input_tokens += response.usage.input_tokens;
                total_usage.output_tokens += response.usage.output_tokens;

                debug!(
                    iteration,
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    stop_reason = ?response.stop_reason,
                    duration_ms = iter_start.elapsed().as_millis() as u64,
                    "llm response"
                );

                match response.stop_reason {
                    StopReason::EndTurn | StopReason::StopSequence => {
                        if self.config.save_episodes {
                            let summary = response.content.clone().unwrap_or_default();
                            let summary_truncated = crew_core::truncated_utf8(&summary, 500, "...");

                            let mut episode = Episode::new(
                                task.id.clone(),
                                self.id.clone(),
                                task.context.working_dir.clone(),
                                summary_truncated.clone(),
                                EpisodeOutcome::Success,
                            );
                            episode.files_modified = files_modified.clone();
                            let ep_id = episode.id.clone();

                            if let Err(e) = self.memory.store(episode).await {
                                warn!(error = %e, "failed to save episode to memory");
                            }

                            // Fire-and-forget: embed summary and store embedding
                            if let Some(ref embedder) = self.embedder {
                                let embedder = embedder.clone();
                                let memory = self.memory.clone();
                                let summary_text = summary_truncated;
                                let episode_id = ep_id;
                                tokio::spawn(async move {
                                    match embedder.embed(&[&summary_text]).await {
                                        Ok(vecs) => {
                                            if let Some(vec) = vecs.into_iter().next() {
                                                if let Err(e) =
                                                    memory.store_embedding(&episode_id, vec).await
                                                {
                                                    warn!(error = %e, "failed to store embedding");
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!(
                                                error = %e,
                                                episode_id = %episode_id,
                                                "failed to generate embedding for episode"
                                            );
                                        }
                                    }
                                });
                            }
                        }

                        self.emit_cost_update(&total_usage, &response.usage);
                        self.reporter().report(ProgressEvent::TaskCompleted {
                            success: true,
                            iterations: iteration,
                            duration: task_start.elapsed(),
                        });

                        info!(
                            total_input_tokens = total_usage.input_tokens,
                            total_output_tokens = total_usage.output_tokens,
                            iterations = iteration,
                            files_modified = files_modified.len(),
                            duration_ms = task_start.elapsed().as_millis() as u64,
                            "task completed"
                        );
                        return Ok(self.build_result(&response, total_usage, files_modified));
                    }
                    StopReason::ToolUse => {
                        self.handle_tool_use(
                            &response,
                            &mut messages,
                            &mut files_modified,
                            &mut total_usage,
                            None,
                        )
                        .await?;
                    }
                    StopReason::MaxTokens => {
                        self.emit_cost_update(&total_usage, &response.usage);
                        self.reporter().report(ProgressEvent::TaskCompleted {
                            success: false,
                            iterations: iteration,
                            duration: task_start.elapsed(),
                        });
                        return Ok(self.build_result(&response, total_usage, files_modified));
                    }
                    StopReason::ContentFiltered => {
                        warn!("content filtered by provider safety/moderation in task");
                        self.emit_cost_update(&total_usage, &response.usage);
                        self.reporter().report(ProgressEvent::TaskCompleted {
                            success: false,
                            iterations: iteration,
                            duration: task_start.elapsed(),
                        });
                        let mut result = self.build_result(&response, total_usage, files_modified);
                        if result.output.is_empty() {
                            result.output =
                                "[Content was blocked by the model's safety filter.]".to_string();
                        }
                        return Ok(result);
                    }
                }
            }
        }
        .instrument(span)
        .await
    }

    async fn build_initial_messages(&self, task: &Task) -> Vec<Message> {
        let mut messages = vec![Message {
            role: MessageRole::System,
            content: self
                .system_prompt
                .read()
                .unwrap_or_else(|e| {
                    tracing::warn!("system prompt lock was poisoned, recovering");
                    e.into_inner()
                })
                .clone(),
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        }];

        // Add working memory from context
        messages.extend(task.context.working_memory.clone());

        // Query episodic memory for relevant past experiences
        let query = match &task.kind {
            crew_core::TaskKind::Plan { goal } => goal.clone(),
            crew_core::TaskKind::Code { instruction, .. } => instruction.clone(),
            crew_core::TaskKind::Review { .. } => "code review".to_string(),
            crew_core::TaskKind::Test { command } => command.clone(),
            crew_core::TaskKind::Custom { name, .. } => name.clone(),
        };

        let episodes_result = if let Some(ref embedder) = self.embedder {
            match embedder.embed(&[query.as_str()]).await {
                Ok(vecs) => {
                    let query_emb = vecs.into_iter().next();
                    self.memory.find_relevant_hybrid(&query, query_emb, 6).await
                }
                Err(e) => {
                    warn!(error = %e, "embedding failed, falling back to keyword search");
                    self.memory.find_relevant_hybrid(&query, None, 6).await
                }
            }
        } else {
            self.memory
                .find_relevant(&task.context.working_dir, &query, 3)
                .await
        };

        if let Ok(episodes) = episodes_result {
            if !episodes.is_empty() {
                let mut context_str = String::from("## Relevant Past Experiences\n\n");
                for ep in &episodes {
                    context_str.push_str(&format!(
                        "### {} ({})\n{}\n",
                        ep.task_id,
                        match ep.outcome {
                            crew_memory::EpisodeOutcome::Success => "succeeded",
                            crew_memory::EpisodeOutcome::Failure => "failed",
                            crew_memory::EpisodeOutcome::Blocked => "blocked",
                            crew_memory::EpisodeOutcome::Cancelled => "cancelled",
                        },
                        ep.summary
                    ));
                    if !ep.key_decisions.is_empty() {
                        context_str.push_str("Key decisions:\n");
                        for decision in &ep.key_decisions {
                            context_str.push_str(&format!("- {}\n", decision));
                        }
                    }
                    context_str.push('\n');
                }

                messages.push(Message {
                    role: MessageRole::System,
                    content: context_str,
                    media: vec![],
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    timestamp: chrono::Utc::now(),
                });
            }
        }

        // Add the task as user message
        let task_content = match &task.kind {
            crew_core::TaskKind::Plan { goal } => format!("Plan how to accomplish: {goal}"),
            crew_core::TaskKind::Code { instruction, files } => {
                let files_str = files
                    .iter()
                    .map(|f| f.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Code task: {instruction}\nFiles in scope: {files_str}")
            }
            crew_core::TaskKind::Review { diff } => format!("Review this diff:\n{diff}"),
            crew_core::TaskKind::Test { command } => format!("Run test: {command}"),
            crew_core::TaskKind::Custom { name, params } => {
                format!("Custom task '{name}': {params}")
            }
        };

        messages.push(Message {
            role: MessageRole::User,
            content: task_content,
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        });

        messages
    }

    fn response_to_message(&self, response: &ChatResponse) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: response.content.clone().unwrap_or_default(),
            media: vec![],
            tool_calls: if response.tool_calls.is_empty() {
                None
            } else {
                Some(response.tool_calls.clone())
            },
            tool_call_id: None,
            reasoning_content: response.reasoning_content.clone(),
            timestamp: chrono::Utc::now(),
        }
    }

    async fn execute_tools(
        &self,
        response: &ChatResponse,
    ) -> Result<(Vec<Message>, Vec<std::path::PathBuf>, TokenUsage)> {
        // Log parallel tool execution details
        let tool_names: Vec<&str> = response
            .tool_calls
            .iter()
            .map(|tc| tc.name.as_str())
            .collect();
        tracing::info!(
            parallel_tools = response.tool_calls.len(),
            tool_names = %tool_names.join(", "),
            "executing tools in parallel"
        );

        // Spawn each tool as a separate tokio task so that if the agent-level
        // timeout fires, the tasks keep running and can perform their own cleanup
        // (e.g., browser tool kills Chrome, spawn tool finishes gracefully).
        // Without tokio::spawn, timeout would drop the futures mid-flight,
        // orphaning child processes (Chrome, shell commands, etc.).
        let handles: Vec<_> = response
            .tool_calls
            .iter()
            .map(|tool_call| {
                // Clone Arc-wrapped fields so the spawned task is 'static
                let tools = self.tools.clone();
                let reporter = self.reporter();
                let hooks = self.hooks.clone();
                let hook_ctx = self.hook_ctx();
                let tc_name = tool_call.name.clone();
                let tc_id = tool_call.id.clone();
                let tc_args = tool_call.arguments.clone();

                tokio::spawn(async move {
                    let tool_start = Instant::now();
                    debug!(tool = %tc_name, tool_id = %tc_id, "executing tool");

                    reporter.report(ProgressEvent::ToolStarted {
                        name: tc_name.clone(),
                        tool_id: tc_id.clone(),
                    });

                    // Before-tool hook: may deny execution
                    if let Some(ref hooks) = hooks {
                        let payload = HookPayload::before_tool(
                            &tc_name,
                            tc_args.clone(),
                            &tc_id,
                            hook_ctx.as_ref(),
                        );
                        if let HookResult::Deny(reason) =
                            hooks.run(HookEvent::BeforeToolCall, &payload).await
                        {
                            let deny_msg = if reason.is_empty() {
                                format!("[HOOK DENIED] Tool '{}' was blocked by a lifecycle hook. Do not retry.", tc_name)
                            } else {
                                format!("[HOOK DENIED] Tool '{}' was blocked: {}. Do not retry.", tc_name, reason)
                            };
                            return (
                                Message {
                                    role: MessageRole::Tool,
                                    content: deny_msg,
                                    media: vec![],
                                    tool_calls: None,
                                    tool_call_id: Some(tc_id),
                                    reasoning_content: None,
                                    timestamp: chrono::Utc::now(),
                                },
                                None,
                                None,
                            );
                        }
                    }

                    let ctx = ToolContext {
                        tool_id: tc_id.clone(),
                        reporter: reporter.clone(),
                    };
                    let result = TOOL_CTX
                        .scope(ctx, tools.execute(&tc_name, &tc_args))
                        .await;

                    let duration = tool_start.elapsed();

                    let (content, file_modified, tool_tokens, tool_success) = match result {
                        Ok(tool_result) => {
                            debug!(
                                tool = %tc_name,
                                success = tool_result.success,
                                duration_ms = duration.as_millis() as u64,
                                "tool completed"
                            );

                            if let Some(ref file) = tool_result.file_modified {
                                info!(tool = %tc_name, file = %file.display(), "file modified");
                                reporter.report(ProgressEvent::FileModified {
                                    path: file.display().to_string(),
                                });
                            }

                            let output_preview =
                                crew_core::truncated_utf8(&tool_result.output, 200, "...");

                            reporter.report(ProgressEvent::ToolCompleted {
                                name: tc_name.clone(),
                                tool_id: tc_id.clone(),
                                success: tool_result.success,
                                output_preview,
                                duration,
                            });

                            let success = tool_result.success;
                            (
                                tool_result.output,
                                tool_result.file_modified,
                                tool_result.tokens_used,
                                success,
                            )
                        }
                        Err(e) => {
                            warn!(
                                tool = %tc_name,
                                error = %e,
                                duration_ms = duration.as_millis() as u64,
                                "tool failed"
                            );

                            reporter.report(ProgressEvent::ToolCompleted {
                                name: tc_name.clone(),
                                tool_id: tc_id.clone(),
                                success: false,
                                output_preview: e.to_string(),
                                duration,
                            });

                            (format!("Error: {e}"), None, None, false)
                        }
                    };

                    // After-tool hook (fire-and-forget)
                    if let Some(ref hooks) = hooks {
                        let payload = HookPayload::after_tool(
                            &tc_name,
                            &tc_id,
                            crew_core::truncated_utf8(&content, 500, "..."),
                            tool_success,
                            duration.as_millis() as u64,
                            hook_ctx.as_ref(),
                        );
                        let _ = hooks.run(HookEvent::AfterToolCall, &payload).await;
                    }

                    // Per-tool output truncation with head/tail split
                    let limit = crew_core::tool_output_limit(&tc_name);
                    let content = crew_core::truncate_head_tail(&content, limit, 0.7);
                    let content = crate::sanitize::sanitize_tool_output(&content);

                    (
                        Message {
                            role: MessageRole::Tool,
                            content,
                            media: vec![],
                            tool_calls: None,
                            tool_call_id: Some(tc_id),
                            reasoning_content: None,
                            timestamp: chrono::Utc::now(),
                        },
                        file_modified,
                        tool_tokens,
                    )
                })
            })
            .collect();

        // Let the LLM specify per-tool timeout via `timeout_secs` in tool call args.
        // Use the max of all requested timeouts, clamped to MAX_TOOL_TIMEOUT_SECS.
        let llm_requested_timeout: u64 = response
            .tool_calls
            .iter()
            .filter_map(|tc| {
                tc.arguments
                    .get("timeout_secs")
                    .and_then(|v| v.as_u64())
            })
            .max()
            .unwrap_or(0);
        let tool_timeout_secs = if llm_requested_timeout > 0 {
            llm_requested_timeout.min(MAX_TOOL_TIMEOUT_SECS).max(self.config.tool_timeout_secs)
        } else {
            self.config.tool_timeout_secs
        };
        let tool_timeout = Duration::from_secs(tool_timeout_secs);
        let results: Vec<_> =
            match tokio::time::timeout(tool_timeout, futures::future::join_all(handles)).await {
                Ok(results) => {
                    // Unwrap JoinHandle results — panics in tool tasks become errors
                    results
                        .into_iter()
                        .map(|r| {
                            r.unwrap_or_else(|e| {
                                // Task panicked — return a placeholder error tuple
                                (
                                    Message {
                                        role: MessageRole::Tool,
                                        content: format!("Tool task panicked: {e}"),
                                        media: vec![],
                                        tool_calls: None,
                                        tool_call_id: None,
                                        reasoning_content: None,
                                        timestamp: chrono::Utc::now(),
                                    },
                                    None,
                                    None,
                                )
                            })
                        })
                        .collect()
                }
                Err(_) => {
                    tracing::error!(
                        timeout_secs = tool_timeout_secs,
                        tool_count = response.tool_calls.len(),
                        tools = %tool_names.join(", "),
                        "tool execution timed out — spawned tasks continue running for cleanup"
                    );
                    // Note: spawned tasks are NOT aborted — they continue running so
                    // tools can perform their own cleanup (browser kills Chrome, etc.).
                    // They will eventually complete via their own internal timeouts.
                    let mut messages = Vec::with_capacity(response.tool_calls.len());
                    for tc in &response.tool_calls {
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: format!(
                                "Tool '{}' timed out after {} seconds",
                                tc.name, tool_timeout_secs
                            ),
                            media: vec![],
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                            reasoning_content: None,
                            timestamp: chrono::Utc::now(),
                        });
                    }
                    return Ok((messages, vec![], TokenUsage::default()));
                }
            };

        // Log completion of all parallel tools
        let result_sizes: Vec<usize> = results.iter().map(|(m, _, _)| m.content.len()).collect();
        let total_result_bytes: usize = result_sizes.iter().sum();
        tracing::info!(
            parallel_tools = results.len(),
            result_sizes = %result_sizes.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", "),
            total_result_bytes,
            "all parallel tools completed"
        );

        // Aggregate results — join_all preserves input order.
        let mut messages = Vec::with_capacity(results.len());
        let mut files_modified = Vec::new();
        let mut tokens_used = TokenUsage::default();

        for (message, file_modified, tool_tokens) in results {
            messages.push(message);
            if let Some(file) = file_modified {
                files_modified.push(file);
            }
            if let Some(tokens) = tool_tokens {
                tokens_used.input_tokens += tokens.input_tokens;
                tokens_used.output_tokens += tokens.output_tokens;
            }
        }

        Ok((messages, files_modified, tokens_used))
    }

    fn trim_to_context_window(&self, messages: &mut Vec<Message>) {
        use crate::compaction::{MIN_RECENT_MESSAGES, compact_messages, find_recent_boundary};
        use crew_llm::context::{estimate_message_tokens, estimate_tokens};

        if messages.len() <= 1 + MIN_RECENT_MESSAGES {
            return;
        }

        let window = self.llm.context_window();
        let budget = (window as f64 * 0.8 / crate::compaction::SAFETY_MARGIN) as u32;

        let total: u32 = messages.iter().map(estimate_message_tokens).sum();
        if total <= budget {
            return;
        }

        let system_tokens = estimate_message_tokens(&messages[0]);
        if system_tokens >= budget {
            warn!(
                system_tokens,
                budget, "system prompt exceeds context window budget, cannot trim"
            );
            return;
        }

        let split = find_recent_boundary(messages, budget, system_tokens);
        let recent_tokens: u32 = messages[split..].iter().map(estimate_message_tokens).sum();

        // If recent messages alone exceed budget, fall back to simple truncation
        if system_tokens + recent_tokens >= budget {
            self.fallback_truncate(messages, budget);
            return;
        }

        let old_messages = &messages[1..split];
        if old_messages.is_empty() {
            return;
        }

        let summary_budget = budget - system_tokens - recent_tokens;
        let summary_text = compact_messages(old_messages, summary_budget);
        let summary_tokens = estimate_tokens(&summary_text) + 4;

        let original_count = messages.len();
        let dropped = split - 1;
        messages.drain(1..split);
        messages.insert(
            1,
            Message {
                role: MessageRole::System,
                content: summary_text,
                media: vec![],
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                timestamp: chrono::Utc::now(),
            },
        );

        info!(
            original_tokens = total,
            summary_tokens,
            messages_compacted = dropped,
            messages_remaining = messages.len(),
            original_messages = original_count,
            "compacted conversation history ({} token budget)",
            budget
        );
    }

    /// Simple truncation fallback when even recent messages exceed budget.
    fn fallback_truncate(&self, messages: &mut Vec<Message>, limit: u32) {
        let system_tokens = crew_llm::context::estimate_message_tokens(&messages[0]);
        let mut kept_tokens = system_tokens;
        let mut keep_from = messages.len();

        for i in (1..messages.len()).rev() {
            let msg_tokens = crew_llm::context::estimate_message_tokens(&messages[i]);
            if kept_tokens + msg_tokens > limit {
                break;
            }
            kept_tokens += msg_tokens;
            keep_from = i;
        }

        // Keep at least 2 non-system messages
        let max_keep_from = messages.len().saturating_sub(2);
        if keep_from > max_keep_from {
            keep_from = max_keep_from;
        }

        // Don't split inside a tool-call group
        while keep_from > 1 && messages[keep_from].role == MessageRole::Tool {
            keep_from -= 1;
        }

        if keep_from > 1 {
            let dropped = keep_from - 1;
            messages.drain(1..keep_from);
            warn!(
                messages_dropped = dropped,
                messages_kept = messages.len(),
                "fallback truncation ({} token limit)",
                limit
            );
        }
    }

    /// Wait until the shutdown flag is set. Used with `tokio::select!`
    /// to cancel long-running operations on Ctrl+C.
    async fn wait_for_shutdown(&self) {
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn consume_stream(
        &self,
        mut stream: ChatStream,
        iteration: u32,
    ) -> Result<(ChatResponse, bool)> {
        // Clear any pending status line (e.g., "Thinking...")
        self.reporter().report(ProgressEvent::Response {
            content: String::new(),
            iteration,
        });

        let mut text = String::new();
        let mut reasoning = String::new();
        // (id, name, args_json, metadata)
        let mut tool_calls: Vec<(String, String, String, Option<serde_json::Value>)> = Vec::new();
        let mut usage = crew_llm::TokenUsage::default();
        let mut stop_reason = StopReason::EndTurn;

        loop {
            let event = tokio::select! {
                event = stream.next() => event,
                _ = self.wait_for_shutdown() => {
                    warn!("shutdown received during streaming");
                    break;
                }
            };

            let Some(event) = event else {
                tracing::debug!("stream ended (None)");
                break;
            };
            tracing::debug!(?event, "stream event received");

            match event {
                StreamEvent::ReasoningDelta(delta) => {
                    reasoning.push_str(&delta);
                }
                StreamEvent::TextDelta(delta) => {
                    self.reporter().report(ProgressEvent::StreamChunk {
                        text: delta.clone(),
                        iteration,
                    });
                    text.push_str(&delta);
                }
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => {
                    while tool_calls.len() <= index {
                        tool_calls.push((String::new(), String::new(), String::new(), None));
                    }
                    if let Some(id) = id {
                        tool_calls[index].0 = id;
                    }
                    if let Some(name) = name {
                        tool_calls[index].1 = name;
                    }
                    tool_calls[index].2.push_str(&arguments_delta);
                }
                StreamEvent::ToolCallMetadata { index, metadata } => {
                    while tool_calls.len() <= index {
                        tool_calls.push((String::new(), String::new(), String::new(), None));
                    }
                    tool_calls[index].3 = Some(metadata);
                }
                StreamEvent::Usage(u) => {
                    usage = u;
                }
                StreamEvent::Done(reason) => {
                    stop_reason = reason;
                }
                StreamEvent::Error(err) => {
                    eyre::bail!("Stream error: {}", err);
                }
            }
        }

        let streamed = !text.is_empty();
        if streamed {
            self.reporter()
                .report(ProgressEvent::StreamDone { iteration });
        }

        // Strip <think> tags from accumulated streaming content (some models
        // embed chain-of-thought in <think> tags via TextDelta instead of
        // using ReasoningDelta events).
        let (text, think_extracted) = crew_llm::strip_think_tags(&text);
        if let Some(ref extracted) = think_extracted {
            if reasoning.is_empty() {
                reasoning = extracted.clone();
            }
        }

        let content = if text.is_empty() { None } else { Some(text) };
        let tool_calls: Vec<crew_core::ToolCall> = tool_calls
            .into_iter()
            .filter(|(_, name, _, _)| !name.is_empty())
            .map(|(id, name, args, metadata)| {
                let arguments = serde_json::from_str(&args).unwrap_or_else(|e| {
                    tracing::warn!(tool = %name, error = %e, raw = %args, "malformed tool call JSON");
                    // Return a String value so the tool's deserialize step fails
                    // and the error propagates back to the LLM for correction.
                    serde_json::Value::String(format!(
                        "MALFORMED_JSON: {e}. Raw input: {}",
                        crew_core::truncated_utf8(&args, 200, "...")
                    ))
                });
                crew_core::ToolCall {
                    id,
                    name,
                    arguments,
                    metadata,
                }
            })
            .collect();

        let reasoning_content = if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        };

        // Fix stop_reason mismatch: some models report "stop" / EndTurn even
        // when they produced tool_calls (documented for OpenAI, Gemini).
        if !tool_calls.is_empty() && stop_reason == StopReason::EndTurn {
            tracing::warn!(
                tool_count = tool_calls.len(),
                "fixing stop_reason: EndTurn with tool_calls present → ToolUse"
            );
            stop_reason = StopReason::ToolUse;
        }

        // Detect repetitive/looping output — model got stuck repeating itself.
        // Replace with a short message so the user sees something useful.
        let content = if let Some(ref text) = content {
            if Self::is_repetitive_output(text) {
                tracing::warn!(
                    content_len = text.len(),
                    "detected repetitive LLM output, replacing with error message"
                );
                None
            } else {
                content
            }
        } else {
            content
        };

        Ok((
            ChatResponse {
                content,
                reasoning_content,
                tool_calls,
                stop_reason,
                usage,
            },
            streamed,
        ))
    }

    fn emit_cost_update(&self, total_usage: &TokenUsage, response_usage: &crew_llm::TokenUsage) {
        let pricing = crew_llm::pricing::model_pricing(self.llm.model_id());
        let response_cost =
            pricing.map(|p| p.cost(response_usage.input_tokens, response_usage.output_tokens));
        let session_cost =
            pricing.map(|p| p.cost(total_usage.input_tokens, total_usage.output_tokens));
        self.reporter().report(ProgressEvent::CostUpdate {
            session_input_tokens: total_usage.input_tokens,
            session_output_tokens: total_usage.output_tokens,
            response_cost,
            session_cost,
        });
    }

    fn build_result(
        &self,
        response: &ChatResponse,
        usage: TokenUsage,
        files_modified: Vec<std::path::PathBuf>,
    ) -> TaskResult {
        let success = response.stop_reason != StopReason::MaxTokens;
        TaskResult {
            success,
            output: response.content.clone().unwrap_or_default(),
            files_modified,
            subtasks: Vec::new(),
            token_usage: crew_core::TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                ..Default::default()
            },
        }
    }

    /// Check whether the agent loop should stop due to budget constraints.
    fn check_budget(
        &self,
        iteration: u32,
        start: Instant,
        total_usage: &TokenUsage,
    ) -> Option<BudgetStop> {
        if self.shutdown.load(Ordering::Acquire) {
            return Some(BudgetStop::Shutdown);
        }
        if iteration >= self.config.max_iterations {
            return Some(BudgetStop::MaxIterations);
        }
        if let Some(timeout) = self.config.max_timeout {
            if start.elapsed() > timeout {
                return Some(BudgetStop::WallClockTimeout { limit: timeout });
            }
        }
        if let Some(max_tokens) = self.config.max_tokens {
            let used = total_usage.input_tokens + total_usage.output_tokens;
            if used >= max_tokens {
                return Some(BudgetStop::MaxTokens {
                    used,
                    limit: max_tokens,
                });
            }
        }
        None
    }

    /// Maximum retries for transient LLM failures (empty responses, stream errors).
    const LLM_RETRY_MAX: u32 = 3;

    /// Check if an LLM response is empty or abnormal and should be retried.
    /// Catches:
    /// - Empty content with no tool calls and no reasoning (including output_tokens > 0 bug)
    /// - Content filtered by safety/moderation
    fn is_retriable_response(response: &ChatResponse) -> bool {
        let has_reasoning = response
            .reasoning_content
            .as_ref()
            .is_some_and(|r| !r.is_empty());
        let is_empty = response.content.as_ref().is_none_or(|c| c.is_empty())
            && response.tool_calls.is_empty()
            && !has_reasoning;
        let is_filtered = response.stop_reason == StopReason::ContentFiltered;
        is_empty || is_filtered
    }

    /// Detect if text content is stuck in a repetitive loop.
    /// Returns true if the same phrase (>= 20 chars) repeats 5+ times.
    fn is_repetitive_output(text: &str) -> bool {
        // Use char count for multi-byte safety (Chinese, emoji, etc.)
        let char_count = text.chars().count();
        if char_count < 200 {
            return false;
        }
        // Check last 500 chars for repeating patterns of 20-100 char lengths
        let check_region: String = if char_count > 500 {
            text.chars().skip(char_count - 500).collect()
        } else {
            text.to_string()
        };
        let region_chars: Vec<char> = check_region.chars().collect();
        let region_len = region_chars.len();
        for pattern_len in [20, 40, 60, 100] {
            if region_len < pattern_len * 3 {
                continue;
            }
            let pattern: String = region_chars[region_len - pattern_len..].iter().collect();
            let count = check_region.matches(&pattern).count();
            if count >= 4 {
                return true;
            }
        }
        false
    }

    /// Check if an error looks like a transient server issue worth retrying.
    fn is_retryable_stream_error(err: &eyre::Report) -> bool {
        let msg = err.to_string().to_lowercase();
        msg.contains("overloaded")
            || msg.contains("temporarily")
            || msg.contains("429")
            || msg.contains("502")
            || msg.contains("503")
            || msg.contains("1305")
            || msg.contains("rate limit")
    }

    /// Call the LLM with before/after lifecycle hooks.
    /// Automatically retries on empty responses and retryable stream errors.
    async fn call_llm_with_hooks(
        &self,
        messages: &[Message],
        tools_spec: &[ToolSpec],
        config: &ChatConfig,
        iteration: u32,
        total_usage: &TokenUsage,
    ) -> Result<(ChatResponse, bool)> {
        let ctx = self.hook_ctx();
        if let Some(ref hooks) = self.hooks {
            let payload = HookPayload::before_llm(
                self.llm.model_id(),
                messages.len(),
                iteration,
                ctx.as_ref(),
            );
            if let HookResult::Deny(reason) = hooks.run(HookEvent::BeforeLlmCall, &payload).await {
                eyre::bail!("LLM call denied by hook: {reason}");
            }
        }

        let mut last_error: Option<eyre::Report> = None;
        // Track token usage from retried (discarded) attempts so cost reporting
        // reflects actual consumption, not just the final successful call.
        let mut retry_usage = TokenUsage::default();

        for attempt in 0..=Self::LLM_RETRY_MAX {
            let call_start = Instant::now();
            // Try the full LLM call (stream creation + consumption)
            let call_result = async {
                let stream = self.llm.chat_stream(messages, tools_spec, config).await?;
                self.consume_stream(stream, iteration).await
            }
            .await;

            match call_result {
                Ok((response, streamed)) => {
                    if !Self::is_retriable_response(&response) {
                        // Genuine success — merge retry usage into response
                        let mut response = response;
                        response.usage.input_tokens += retry_usage.input_tokens;
                        response.usage.output_tokens += retry_usage.output_tokens;

                        if let Some(ref hooks) = self.hooks {
                            let latency_ms = call_start.elapsed().as_millis() as u64;
                            let cum_in = total_usage.input_tokens + response.usage.input_tokens;
                            let cum_out = total_usage.output_tokens + response.usage.output_tokens;
                            let pricing = crew_llm::pricing::model_pricing(self.llm.model_id());
                            let session_cost = pricing.map(|p| p.cost(cum_in, cum_out));
                            let response_cost = pricing.map(|p| {
                                p.cost(response.usage.input_tokens, response.usage.output_tokens)
                            });
                            let payload = HookPayload::after_llm(
                                self.llm.model_id(),
                                iteration,
                                &format!("{:?}", response.stop_reason),
                                !response.tool_calls.is_empty(),
                                response.usage.input_tokens,
                                response.usage.output_tokens,
                                self.llm.provider_name(),
                                latency_ms,
                                cum_in,
                                cum_out,
                                session_cost,
                                response_cost,
                                ctx.as_ref(),
                            );
                            let _ = hooks.run(HookEvent::AfterLlmCall, &payload).await;
                        }
                        return Ok((response, streamed));
                    }

                    if attempt == Self::LLM_RETRY_MAX {
                        // All retries exhausted with empty/filtered response — report
                        // failure to the adaptive router so it can failover, then
                        // return error.
                        let reason = if response.stop_reason == StopReason::ContentFiltered {
                            "content filtered by safety/moderation"
                        } else {
                            "empty response (no content or tool_calls)"
                        };
                        self.llm.report_late_failure();
                        warn!(
                            attempts = Self::LLM_RETRY_MAX + 1,
                            reason,
                            "LLM returned empty response after all retries, triggering failover"
                        );
                        return Err(eyre::eyre!(
                            "LLM returned empty response after {} retries: {}",
                            Self::LLM_RETRY_MAX + 1,
                            reason
                        ));
                    }

                    // Empty or abnormal response — accumulate usage and retry
                    retry_usage.input_tokens += response.usage.input_tokens;
                    retry_usage.output_tokens += response.usage.output_tokens;

                    let delay = Duration::from_secs(1 << attempt);
                    let reason = if response.stop_reason == StopReason::ContentFiltered {
                        "content filtered by safety/moderation"
                    } else {
                        "empty response (no content/tool_calls)"
                    };
                    warn!(
                        attempt = attempt + 1,
                        max = Self::LLM_RETRY_MAX,
                        delay_s = delay.as_secs(),
                        iteration,
                        stop_reason = ?response.stop_reason,
                        reason,
                        "abnormal LLM response, retrying"
                    );
                    self.reporter().report(ProgressEvent::LlmStatus {
                        message: format!(
                            "Retrying ({}/{})... {}",
                            attempt + 1,
                            Self::LLM_RETRY_MAX + 1,
                            reason,
                        ),
                        iteration,
                    });
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    if attempt < Self::LLM_RETRY_MAX && Self::is_retryable_stream_error(&e) {
                        let delay = Duration::from_secs(1 << attempt);
                        warn!(
                            attempt = attempt + 1,
                            max = Self::LLM_RETRY_MAX,
                            delay_s = delay.as_secs(),
                            error = %e,
                            iteration,
                            "retryable stream error, retrying"
                        );
                        self.reporter().report(ProgressEvent::LlmStatus {
                            message: format!(
                                "Retrying ({}/{})... stream error",
                                attempt + 1,
                                Self::LLM_RETRY_MAX + 1,
                            ),
                            iteration,
                        });
                        last_error = Some(e);
                        tokio::time::sleep(delay).await;
                    } else {
                        // Non-retryable error or last attempt — propagate
                        return Err(e);
                    }
                }
            }
        }

        // All retries exhausted with errors
        Err(last_error.unwrap_or_else(|| eyre::eyre!("LLM call failed after retries")))
    }

    /// Execute tool calls from an LLM response and accumulate results.
    async fn handle_tool_use(
        &self,
        response: &ChatResponse,
        messages: &mut Vec<Message>,
        files_modified: &mut Vec<PathBuf>,
        total_usage: &mut TokenUsage,
        tracker: Option<&TokenTracker>,
    ) -> Result<()> {
        // Fix tool_call IDs — some models (e.g. qwen via dashscope) generate
        // duplicate or empty IDs which downstream providers reject with 400.
        // Also sanitize characters: some providers (e.g. Moonshot/kimi) generate IDs
        // with colons like "admin_view_sessions:11" which OpenAI rejects.
        // We fix IDs on the response clone so both the assistant message and tool result
        // messages use the same corrected IDs.
        let mut response = response.clone();
        {
            let mut seen_ids = std::collections::HashSet::new();
            for (i, tc) in response.tool_calls.iter_mut().enumerate() {
                // Sanitize characters: keep only alphanumeric, underscore, hyphen
                tc.id = sanitize_tool_call_id(&tc.id);

                if tc.id.is_empty() || !seen_ids.insert(tc.id.clone()) {
                    let new_id = format!("call_{}_{}", i, &tc.name);
                    tracing::warn!(
                        old_id = %tc.id,
                        new_id = %new_id,
                        tool = %tc.name,
                        "fixing empty/duplicate tool_call_id"
                    );
                    tc.id = new_id;
                }
            }
        }

        // Deduplicate tool calls with identical name + arguments (some models
        // return the same call twice, wasting execution).
        {
            let orig_len = response.tool_calls.len();
            let mut seen_calls = std::collections::HashSet::new();
            response.tool_calls.retain(|tc| {
                let key = format!("{}:{}", tc.name, tc.arguments);
                seen_calls.insert(key)
            });
            if response.tool_calls.len() < orig_len {
                tracing::warn!(
                    removed = orig_len - response.tool_calls.len(),
                    "removed duplicate tool calls (same name+arguments)"
                );
            }
        }
        messages.push(self.response_to_message(&response));
        let (tool_messages, tool_files, tool_tokens) = self.execute_tools(&response).await?;
        messages.extend(tool_messages);
        files_modified.extend(tool_files);
        total_usage.input_tokens += tool_tokens.input_tokens;
        total_usage.output_tokens += tool_tokens.output_tokens;
        if let Some(t) = tracker {
            t.input_tokens
                .store(total_usage.input_tokens, Ordering::Relaxed);
            t.output_tokens
                .store(total_usage.output_tokens, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Log and report a budget stop event (used by `run_task`).
    fn report_budget_stop(&self, stop: &BudgetStop, iteration: u32) {
        match stop {
            BudgetStop::Shutdown => {
                info!(iteration, "shutdown signal received");
                self.reporter().report(ProgressEvent::TaskInterrupted {
                    iterations: iteration,
                });
            }
            BudgetStop::MaxIterations => {
                warn!(
                    iteration,
                    max = self.config.max_iterations,
                    "hit max iterations limit"
                );
                self.reporter().report(ProgressEvent::MaxIterationsReached {
                    limit: self.config.max_iterations,
                });
            }
            BudgetStop::MaxTokens { used, limit } => {
                warn!(used, max = limit, "hit token budget limit");
                self.reporter().report(ProgressEvent::TokenBudgetExceeded {
                    used: *used,
                    limit: *limit,
                });
            }
            BudgetStop::WallClockTimeout { limit } => {
                warn!(limit_s = limit.as_secs(), "hit wall-clock timeout");
                self.reporter()
                    .report(ProgressEvent::WallClockTimeoutReached {
                        elapsed: *limit,
                        limit: *limit,
                    });
            }
        }
    }
}

/// Sanitize a tool_call_id to contain only characters accepted by all providers.
/// Some models (e.g. Moonshot/kimi) generate IDs like "admin_view_sessions:11"
/// which OpenAI rejects (only allows letters, numbers, underscores, dashes).
/// Merge all system messages into the first one so providers that require a
/// single leading system message (e.g. Qwen) don't reject the request.
///
/// After context compaction or session history reload, system messages can end
/// up scattered throughout the message list.  This collects their content into
/// the first system message and removes the rest.
fn normalize_system_messages(messages: &mut Vec<Message>) {
    if messages.len() <= 1 {
        return;
    }

    // Convert context-bearing system messages (background task results,
    // conversation summaries) to user messages so they don't bloat the
    // system prompt.  These contain prior conversation content, not
    // instructions for the model.
    for m in messages.iter_mut().skip(1) {
        if m.role == MessageRole::System
            && (m.content.starts_with("[Background task")
                || m.content.starts_with("[Conversation summary]"))
        {
            m.role = MessageRole::User;
            m.content = format!("[System note] {}", m.content);
        }
    }

    // Merge remaining extra system messages (actual instructions) into
    // the first system prompt.
    let mut extra_indices = Vec::new();
    for (i, m) in messages.iter().enumerate().skip(1) {
        if m.role == MessageRole::System {
            extra_indices.push(i);
        }
    }
    if extra_indices.is_empty() {
        return;
    }
    let extra_content: Vec<String> = extra_indices
        .iter()
        .filter_map(|&i| {
            let c = &messages[i].content;
            if c.is_empty() { None } else { Some(c.clone()) }
        })
        .collect();
    if !extra_content.is_empty() {
        let first = &mut messages[0];
        for text in extra_content {
            first.content.push_str("\n\n");
            first.content.push_str(&text);
        }
    }
    for &i in extra_indices.iter().rev() {
        messages.remove(i);
    }
}

/// Gather scattered tool results to be contiguous with their parent assistant.
///
/// OpenAI-compatible APIs require: assistant(tool_calls) → tool(result)*
/// with no other messages in between.  In speculative/concurrent mode,
/// multiple conversation threads (primary + overflow) save messages to the
/// same session, so tool results may be separated from their parent by
/// user messages, system messages, or other threads' tool_call groups.
///
/// Strategy:
/// 1. For each assistant with tool_calls, extract ALL matching tool results
///    from the entire message list (both before and after the assistant).
/// 2. Deduplicate by tool_call_id (keep the latest result for each ID).
/// 3. Re-insert exactly one result per tool_call right after the assistant.
///
/// This handles backward-stranded results (e.g. from overflow tasks saving
/// results before the assistant message) and duplicate results.
fn repair_message_order(messages: &mut Vec<Message>) {
    use std::collections::{HashMap, HashSet};

    let mut i = 0;
    while i < messages.len() {
        // Find assistant message with tool_calls
        let has_tool_calls = messages[i].role == MessageRole::Assistant
            && messages[i]
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty());
        if !has_tool_calls {
            i += 1;
            continue;
        }

        // Collect expected tool_call IDs
        let expected_ids: HashSet<String> = messages[i]
            .tool_calls
            .as_ref()
            .unwrap()
            .iter()
            .map(|tc| tc.id.clone())
            .collect();

        // Extract ALL matching tool results from the entire message list.
        // For duplicate tool_call_ids, keep the LAST one (most recent result).
        let mut collected: HashMap<String, Message> = HashMap::new();
        let mut j = 0;
        while j < messages.len() {
            if j == i {
                j += 1;
                continue;
            }
            let is_match = messages[j].role == MessageRole::Tool
                && messages[j]
                    .tool_call_id
                    .as_ref()
                    .is_some_and(|id| expected_ids.contains(id));
            if is_match {
                let msg = messages.remove(j);
                // Overwrite keeps the last occurrence (latest result)
                let id = msg.tool_call_id.clone().unwrap();
                collected.insert(id, msg);
                // Adjust i if we removed before it
                if j < i {
                    i -= 1;
                }
                continue; // don't increment j — removal shifted elements
            }
            j += 1;
        }

        // Re-insert one result per tool_call right after the assistant,
        // in the same order as tool_calls appear in the assistant message.
        let call_ids: Vec<String> = messages[i]
            .tool_calls
            .as_ref()
            .map(|calls| calls.iter().map(|tc| tc.id.clone()).collect())
            .unwrap_or_default();
        let mut insert_pos = i + 1;
        for id in &call_ids {
            if let Some(msg) = collected.remove(id) {
                messages.insert(insert_pos, msg);
                insert_pos += 1;
            }
        }

        i = insert_pos;
    }
}

/// Repair orphaned tool_call / tool_result pairs in the message list.
///
/// LLM providers reject messages where an assistant has tool_calls but the
/// corresponding tool result messages are missing (or vice versa).  This can
/// happen when compaction or session history truncation splits a tool group.
///
/// Strategy: find matched pairs (call ID exists in both assistant tool_calls
/// AND tool result messages). Strip anything unmatched.
fn repair_tool_pairs(messages: &mut Vec<Message>) {
    use std::collections::HashSet;

    // Collect all tool_call IDs from assistant messages
    let call_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .flat_map(|m| {
            m.tool_calls
                .as_ref()
                .into_iter()
                .flat_map(|calls| calls.iter().map(|tc| tc.id.clone()))
        })
        .collect();

    // Collect all tool_call_ids from Tool result messages
    let result_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .filter_map(|m| m.tool_call_id.clone())
        .collect();

    // Matched = present in both sets
    let matched: HashSet<&String> = call_ids.intersection(&result_ids).collect();

    // Strip tool_calls from assistant messages where ANY call ID is unmatched
    for m in messages.iter_mut() {
        if m.role == MessageRole::Assistant {
            if let Some(ref calls) = m.tool_calls {
                if calls.iter().any(|tc| !matched.contains(&tc.id)) {
                    let names: Vec<_> = calls.iter().map(|tc| tc.name.as_str()).collect();
                    if m.content.is_empty() {
                        m.content = format!("[Called tools: {}]", names.join(", "));
                    }
                    m.tool_calls = None;
                }
            }
        }
    }

    // Remove Tool result messages whose call ID is unmatched or whose
    // parent assistant had its tool_calls stripped.
    let remaining_call_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .flat_map(|m| {
            m.tool_calls
                .as_ref()
                .into_iter()
                .flat_map(|calls| calls.iter().map(|tc| tc.id.clone()))
        })
        .collect();

    messages.retain(|m| {
        if m.role == MessageRole::Tool {
            match m.tool_call_id {
                Some(ref id) => return remaining_call_ids.contains(id),
                None => return false, // Tool messages without tool_call_id are invalid
            }
        }
        true
    });
}

/// Truncate long tool result messages from prior conversation rounds.
///
/// When a session contains multi-round conversations, old tool results
/// (e.g. a 10,000-word research report from `run_pipeline`) dominate the
/// context window and cause the LLM to re-engage with prior questions
/// instead of focusing on the latest user message.
///
/// This function finds the last user message (the current question) and
/// truncates tool result messages that appear BEFORE it if they exceed
/// `MAX_OLD_TOOL_RESULT_CHARS`.  Tool results in the current conversation
/// round (after the last user message) are kept intact so the agent can
/// reference them.
fn truncate_old_tool_results(messages: &mut Vec<Message>) {
    const MAX_OLD_TOOL_RESULT_CHARS: usize = 800;

    // Find the last user message — everything before it is "old" context
    let last_user_idx = messages
        .iter()
        .rposition(|m| m.role == MessageRole::User);
    let boundary = match last_user_idx {
        Some(idx) => idx,
        None => return, // no user message, nothing to truncate
    };

    for msg in messages[..boundary].iter_mut() {
        if msg.role == MessageRole::Tool && msg.content.len() > MAX_OLD_TOOL_RESULT_CHARS {
            let truncated: String = msg.content.chars().take(MAX_OLD_TOOL_RESULT_CHARS).collect();
            msg.content = format!("{truncated}\n\n[... truncated for brevity]");
        }
    }
}

fn sanitize_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use crew_core::ToolCall;
    use crew_llm::{ChatResponse, StopReason, TokenUsage as LlmTokenUsage};

    // ---------- AgentConfig::default ----------

    #[test]
    fn agent_config_default_values() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.max_iterations, 50);
        assert_eq!(cfg.max_tokens, None);
        assert_eq!(cfg.max_timeout, Some(Duration::from_secs(600)));
        assert!(cfg.save_episodes);
        assert_eq!(cfg.tool_timeout_secs, 600);
        assert!(cfg.worker_prompt.is_none());
    }

    // ---------- TokenTracker ----------

    #[test]
    fn token_tracker_new_starts_at_zero() {
        let t = TokenTracker::new();
        assert_eq!(t.input_tokens.load(Ordering::Relaxed), 0);
        assert_eq!(t.output_tokens.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn token_tracker_default_starts_at_zero() {
        let t = TokenTracker::default();
        assert_eq!(t.input_tokens.load(Ordering::Relaxed), 0);
        assert_eq!(t.output_tokens.load(Ordering::Relaxed), 0);
    }

    // ---------- BudgetStop::message ----------

    #[test]
    fn budget_stop_shutdown_message() {
        assert_eq!(BudgetStop::Shutdown.message(), "Interrupted.");
    }

    #[test]
    fn budget_stop_max_iterations_message() {
        assert_eq!(
            BudgetStop::MaxIterations.message(),
            "Reached max iterations."
        );
    }

    #[test]
    fn budget_stop_max_tokens_message() {
        let msg = BudgetStop::MaxTokens {
            used: 1000,
            limit: 500,
        }
        .message();
        assert!(
            msg.contains("token") || msg.contains("Token") || msg.contains("TOKEN"),
            "expected 'token' in: {msg}"
        );
        assert!(msg.contains("1000"), "expected '1000' in: {msg}");
        assert!(msg.contains("500"), "expected '500' in: {msg}");
    }

    #[test]
    fn budget_stop_wall_clock_timeout_message() {
        let msg = BudgetStop::WallClockTimeout {
            limit: Duration::from_secs(120),
        }
        .message();
        assert!(
            msg.to_lowercase().contains("timeout"),
            "expected 'timeout' in: {msg}"
        );
    }

    // ---------- Agent::is_retriable_response ----------

    fn make_response(
        content: Option<&str>,
        tool_calls: Vec<ToolCall>,
        output_tokens: u32,
    ) -> ChatResponse {
        make_response_with_stop(content, tool_calls, output_tokens, StopReason::EndTurn)
    }

    fn make_response_with_stop(
        content: Option<&str>,
        tool_calls: Vec<ToolCall>,
        output_tokens: u32,
        stop_reason: StopReason,
    ) -> ChatResponse {
        ChatResponse {
            content: content.map(String::from),
            reasoning_content: None,
            tool_calls,
            stop_reason,
            usage: LlmTokenUsage {
                input_tokens: 0,
                output_tokens,
                ..Default::default()
            },
        }
    }

    #[test]
    fn should_retry_when_all_empty() {
        let r = make_response(None, vec![], 0);
        assert!(Agent::is_retriable_response(&r));

        let r2 = make_response(Some(""), vec![], 0);
        assert!(Agent::is_retriable_response(&r2));
    }

    #[test]
    fn should_not_retry_with_content() {
        let r = make_response(Some("hello"), vec![], 0);
        assert!(!Agent::is_retriable_response(&r));
    }

    #[test]
    fn should_not_retry_with_tool_calls() {
        let tc = ToolCall {
            id: "1".into(),
            name: "test".into(),
            arguments: serde_json::json!({}),
            metadata: None,
        };
        let r = make_response(None, vec![tc], 0);
        assert!(!Agent::is_retriable_response(&r));
    }

    #[test]
    fn should_retry_with_tokens_but_no_content() {
        let r = make_response(None, vec![], 10);
        assert!(Agent::is_retriable_response(&r));
    }

    #[test]
    fn should_retry_when_content_filtered() {
        let r = make_response_with_stop(None, vec![], 0, StopReason::ContentFiltered);
        assert!(Agent::is_retriable_response(&r));

        // Even with partial content, content_filtered should retry
        let r2 = make_response_with_stop(Some("partial"), vec![], 10, StopReason::ContentFiltered);
        assert!(Agent::is_retriable_response(&r2));
    }

    // ---------- Agent::is_repetitive_output ----------

    #[test]
    fn should_detect_repetitive_output() {
        let repeated = "This is a test phrase. ".repeat(30);
        assert!(Agent::is_repetitive_output(&repeated));
    }

    #[test]
    fn should_not_flag_normal_output() {
        let normal = "The quick brown fox jumps over the lazy dog. \
                      Pack my box with five dozen liquor jugs. \
                      How vexingly quick daft zebras jump.";
        assert!(!Agent::is_repetitive_output(normal));
    }

    #[test]
    fn should_not_flag_short_text() {
        assert!(!Agent::is_repetitive_output("hello hello hello"));
    }

    // ---------- normalize_system_messages ----------

    fn sys(content: &str) -> Message {
        Message {
            role: MessageRole::System,
            content: content.to_string(),
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        }
    }

    fn user(content: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: content.to_string(),
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn should_merge_multiple_system_messages_into_first() {
        let mut msgs = vec![
            sys("system prompt"),
            sys("compaction summary"),
            user("hello"),
        ];
        normalize_system_messages(&mut msgs);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, MessageRole::System);
        assert!(msgs[0].content.contains("system prompt"));
        assert!(msgs[0].content.contains("compaction summary"));
        assert_eq!(msgs[1].role, MessageRole::User);
    }

    #[test]
    fn should_merge_scattered_system_messages() {
        let mut msgs = vec![
            sys("prompt"),
            user("msg1"),
            sys("mid-summary"),
            user("msg2"),
        ];
        normalize_system_messages(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, MessageRole::System);
        assert!(msgs[0].content.contains("prompt"));
        assert!(msgs[0].content.contains("mid-summary"));
        assert_eq!(msgs[1].role, MessageRole::User);
        assert_eq!(msgs[2].role, MessageRole::User);
    }

    #[test]
    fn should_noop_when_single_system_message() {
        let mut msgs = vec![sys("prompt"), user("hello")];
        normalize_system_messages(&mut msgs);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "prompt");
    }

    // ---------- repair_tool_pairs ----------

    fn assistant_with_tools(tool_ids: &[&str]) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: String::new(),
            media: vec![],
            tool_calls: Some(
                tool_ids
                    .iter()
                    .map(|id| crew_core::ToolCall {
                        id: id.to_string(),
                        name: "test_tool".to_string(),
                        arguments: serde_json::json!({}),
                        metadata: None,
                    })
                    .collect(),
            ),
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        }
    }

    fn tool_result_msg(id: &str) -> Message {
        Message {
            role: MessageRole::Tool,
            content: "result".to_string(),
            media: vec![],
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn should_strip_orphaned_tool_calls() {
        let mut msgs = vec![
            sys("prompt"),
            assistant_with_tools(&["tc1", "tc2"]),
            tool_result_msg("tc1"),
            // tc2 result is missing — orphaned
            user("next question"),
        ];
        repair_tool_pairs(&mut msgs);
        // assistant's tool_calls should be stripped (tc2 has no result)
        assert!(msgs[1].tool_calls.is_none());
        assert!(msgs[1].content.contains("test_tool"));
        // tc1 result should also be removed (its assistant lost tool_calls)
        assert_eq!(msgs.len(), 3); // sys, assistant(text), user
    }

    #[test]
    fn should_keep_complete_tool_pairs() {
        let mut msgs = vec![
            sys("prompt"),
            assistant_with_tools(&["tc1"]),
            tool_result_msg("tc1"),
            user("thanks"),
        ];
        repair_tool_pairs(&mut msgs);
        assert_eq!(msgs.len(), 4);
        assert!(msgs[1].tool_calls.is_some());
    }

    #[test]
    fn should_remove_orphaned_tool_results() {
        let mut msgs = vec![
            sys("prompt"),
            tool_result_msg("tc_orphan"), // no matching assistant
            user("hello"),
        ];
        repair_tool_pairs(&mut msgs);
        assert_eq!(msgs.len(), 2); // sys, user
    }

    // ---------- repair_message_order ----------

    #[test]
    fn should_gather_scattered_tool_result_past_user_message() {
        // Speculative mode: user sent a new message while tool was running.
        // Tool result ended up after the user message.
        let mut msgs = vec![
            sys("prompt"),
            assistant_with_tools(&["tc1"]),
            user("new question"),       // overflow user msg
            tool_result_msg("tc1"),     // scattered result
        ];
        repair_message_order(&mut msgs);
        // After repair: tool result gathered next to assistant.
        // User message stays where it is (not displaced).
        assert_eq!(msgs[0].role, MessageRole::System);
        assert_eq!(msgs[1].role, MessageRole::Assistant);
        assert_eq!(msgs[2].role, MessageRole::Tool);        // gathered
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("tc1"));
        assert_eq!(msgs[3].role, MessageRole::User);        // stays in place
        assert_eq!(msgs[3].content, "new question");
    }

    #[test]
    fn should_gather_scattered_tool_results_past_system_message() {
        let mut msgs = vec![
            assistant_with_tools(&["tc1", "tc2"]),
            tool_result_msg("tc1"),
            sys("background task result"),  // injected mid-execution
            tool_result_msg("tc2"),         // scattered
        ];
        repair_message_order(&mut msgs);
        // Both tool results gathered next to assistant, system stays after
        assert_eq!(msgs[0].role, MessageRole::Assistant);
        assert_eq!(msgs[1].role, MessageRole::Tool);
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("tc1"));
        assert_eq!(msgs[2].role, MessageRole::Tool);
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("tc2"));
        assert_eq!(msgs[3].role, MessageRole::System);
    }

    #[test]
    fn should_handle_concurrent_tool_call_threads() {
        // Two concurrent threads: primary (tc1) and overflow (tc2).
        // Messages are interleaved by timestamp.
        let mut msgs = vec![
            user("make slides"),
            assistant_with_tools(&["tc1"]),          // primary tool call
            user("what time is it"),                 // overflow user
            assistant_with_tools(&["tc2"]),          // overflow tool call
            tool_result_msg("tc2"),                  // overflow result (fast)
            tool_result_msg("tc1"),                  // primary result (slow)
        ];
        repair_message_order(&mut msgs);
        // tc1 result should be gathered next to its parent assistant
        assert_eq!(msgs[0].role, MessageRole::User);
        assert_eq!(msgs[1].role, MessageRole::Assistant); // tc1 parent
        assert_eq!(msgs[2].role, MessageRole::Tool);
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("tc1")); // gathered
        assert_eq!(msgs[3].role, MessageRole::User);      // overflow stays
        assert_eq!(msgs[4].role, MessageRole::Assistant);  // tc2 parent
        assert_eq!(msgs[5].role, MessageRole::Tool);
        assert_eq!(msgs[5].tool_call_id.as_deref(), Some("tc2")); // stays
    }

    #[test]
    fn should_not_modify_valid_message_order() {
        let mut msgs = vec![
            sys("prompt"),
            assistant_with_tools(&["tc1"]),
            tool_result_msg("tc1"),
            user("thanks"),
        ];
        let original_len = msgs.len();
        repair_message_order(&mut msgs);
        assert_eq!(msgs.len(), original_len);
        assert_eq!(msgs[3].content, "thanks");
    }

    #[test]
    fn should_gather_backward_stranded_tool_result() {
        // Bug scenario: overflow saved a tool result BEFORE its parent assistant.
        // The same tool_call_id appears both before and after the assistant.
        let mut msgs = vec![
            sys("prompt"),
            user("tts"),
            tool_result_msg("tc1"),               // backward-stranded duplicate
            assistant_with_tools(&["tc1"]),
            tool_result_msg("tc1"),               // correct result
            user("next question"),
        ];
        repair_message_order(&mut msgs);
        // Duplicate removed, one result gathered after assistant
        assert_eq!(msgs[0].role, MessageRole::System);
        assert_eq!(msgs[1].role, MessageRole::User);
        assert_eq!(msgs[1].content, "tts");
        assert_eq!(msgs[2].role, MessageRole::Assistant);
        assert_eq!(msgs[3].role, MessageRole::Tool);
        assert_eq!(msgs[3].tool_call_id.as_deref(), Some("tc1"));
        assert_eq!(msgs[4].role, MessageRole::User);
        assert_eq!(msgs[4].content, "next question");
        assert_eq!(msgs.len(), 5); // duplicate removed
    }

    #[test]
    fn should_remove_tool_result_with_no_tool_call_id() {
        let mut msgs = vec![
            sys("prompt"),
            assistant_with_tools(&["tc1"]),
            tool_result_msg("tc1"),
            Message {
                role: MessageRole::Tool,
                content: "Tool task panicked".to_string(),
                media: vec![],
                tool_calls: None,
                tool_call_id: None, // no tool_call_id
                reasoning_content: None,
                timestamp: chrono::Utc::now(),
            },
            user("thanks"),
        ];
        repair_tool_pairs(&mut msgs);
        assert_eq!(msgs.len(), 4); // sys, assistant, tool(tc1), user
    }

    // ---------- sanitize_tool_call_id ----------

    #[test]
    fn should_sanitize_colons_in_tool_call_id() {
        assert_eq!(
            sanitize_tool_call_id("admin_view_sessions:11"),
            "admin_view_sessions_11"
        );
    }

    #[test]
    fn should_preserve_valid_tool_call_id() {
        assert_eq!(sanitize_tool_call_id("call_0_shell"), "call_0_shell");
        assert_eq!(sanitize_tool_call_id("toolu_01A-bC"), "toolu_01A-bC");
    }

    #[test]
    fn should_sanitize_special_chars_in_tool_call_id() {
        assert_eq!(
            sanitize_tool_call_id("id.with.dots:and:colons"),
            "id_with_dots_and_colons"
        );
    }

    // ---------- Agent::is_retryable_stream_error ----------

    #[test]
    fn is_retryable_stream_error_transient_errors() {
        for keyword in ["overloaded", "429", "503", "rate limit"] {
            let err = eyre::eyre!("Server error: {}", keyword);
            assert!(
                Agent::is_retryable_stream_error(&err),
                "expected retryable for: {keyword}"
            );
        }
    }

    #[test]
    fn is_retryable_stream_error_non_retryable() {
        let err = eyre::eyre!("invalid json");
        assert!(!Agent::is_retryable_stream_error(&err));
    }

    // ---------- ConversationResponse derives ----------

    #[test]
    fn conversation_response_clone_and_debug() {
        let resp = ConversationResponse {
            content: "test".into(),
            token_usage: crew_core::TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                ..Default::default()
            },
            files_modified: vec![],
            streamed: false,
            messages: vec![],
        };
        let cloned = resp.clone();
        assert_eq!(cloned.content, "test");
        assert_eq!(cloned.token_usage.input_tokens, 10);

        // Debug trait works
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("ConversationResponse"));
    }
}
