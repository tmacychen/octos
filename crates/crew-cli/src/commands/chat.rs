//! Chat command: interactive multi-turn conversation with an agent.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;
use colored::Colorize;
use crew_agent::{Agent, AgentConfig, ConsoleReporter, HookExecutor, ToolRegistry};
use crew_core::{AgentId, Message, MessageRole};
use crew_llm::{
    EmbeddingProvider, LlmProvider, OpenAIEmbedder, ProviderChain, RetryProvider,
    anthropic::AnthropicProvider, gemini::GeminiProvider, openai::OpenAIProvider,
    openrouter::OpenRouterProvider,
};
use crew_memory::{EpisodeStore, MemoryStore};
use eyre::{Result, WrapErr};
use rustyline::DefaultEditor;

use super::Executable;
use crate::config::Config;

/// Interactive multi-turn chat with an agent.
#[derive(Debug, Args)]
pub struct ChatCommand {
    /// Working directory (defaults to current directory).
    #[arg(short, long)]
    pub cwd: Option<PathBuf>,

    /// Data directory for episodes, memory, sessions (defaults to $CREW_HOME or ~/.crew).
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Path to config file.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// LLM provider to use (overrides config).
    #[arg(long)]
    pub provider: Option<String>,

    /// Model to use (overrides config).
    #[arg(long)]
    pub model: Option<String>,

    /// Custom base URL for the API endpoint (overrides config).
    #[arg(long)]
    pub base_url: Option<String>,

    /// Maximum tool-call iterations per message (default: 20).
    #[arg(long, default_value = "20")]
    pub max_iterations: u32,

    /// Verbose output (show tool outputs).
    #[arg(short, long)]
    pub verbose: bool,

    /// Disable automatic retry on transient errors.
    #[arg(long)]
    pub no_retry: bool,

    /// Send a single message and exit (non-interactive mode).
    #[arg(short, long)]
    pub message: Option<String>,
}

/// Exit commands.
const EXIT_COMMANDS: &[&str] = &["exit", "quit", "/exit", "/quit", ":q"];

impl Executable for ChatCommand {
    fn execute(self) -> Result<()> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(8 * 1024 * 1024) // 8MB stack for deep agent futures
            .build()
            .wrap_err("failed to create tokio runtime")?
            .block_on(self.run_async())
    }
}

impl ChatCommand {
    async fn run_async(self) -> Result<()> {
        let cwd = match self.cwd {
            Some(p) => p,
            None => std::env::current_dir().wrap_err("failed to get current directory")?,
        };

        // Load config
        let config = if let Some(config_path) = &self.config {
            Config::from_file(config_path)?
        } else {
            Config::load(&cwd)?
        };

        let model = self.model.or(config.model.clone());
        let base_url = self.base_url.or(config.base_url.clone());
        let provider_name = self
            .provider
            .or(config.provider.clone())
            .or_else(|| {
                model
                    .as_deref()
                    .and_then(crate::config::detect_provider)
                    .map(String::from)
            })
            .unwrap_or_else(|| "anthropic".to_string());

        // Create LLM provider (with optional failover chain)
        let base_provider: Arc<dyn LlmProvider> =
            create_provider(&provider_name, &config, model, base_url)?;
        let model_id = base_provider.model_id().to_string();

        let llm: Arc<dyn LlmProvider> = if self.no_retry {
            base_provider
        } else if config.fallback_models.is_empty() {
            Arc::new(RetryProvider::new(base_provider))
        } else {
            let mut providers: Vec<Arc<dyn LlmProvider>> =
                vec![Arc::new(RetryProvider::new(base_provider))];
            for fb in &config.fallback_models {
                match create_provider(&fb.provider, &config, fb.model.clone(), fb.base_url.clone())
                {
                    Ok(p) => providers.push(Arc::new(RetryProvider::new(p))),
                    Err(e) => {
                        tracing::warn!(provider = %fb.provider, error = %e, "skipping fallback provider");
                    }
                }
            }
            Arc::new(ProviderChain::new(providers))
        };

        // Resolve data directory (--data-dir > $CREW_HOME > ~/.crew)
        let data_dir = super::resolve_data_dir(self.data_dir)?;
        let memory = Arc::new(
            EpisodeStore::open(&data_dir)
                .await
                .wrap_err("failed to open episode store")?,
        );

        // Create tool registry (with sandbox if configured)
        let sandbox = crew_agent::create_sandbox(&config.sandbox);
        let mut tools = ToolRegistry::with_builtins_and_sandbox(&cwd, sandbox);
        tools.register(crew_agent::DeepSearchTool::new(data_dir.join("research")));

        // Register spawn tool for sync sub-agent support in chat mode.
        // Background mode won't deliver results (dummy channel), but sync mode works fine.
        let (spawn_tx, _spawn_rx) = tokio::sync::mpsc::channel(1);
        tools.register(crew_agent::SpawnTool::new(
            llm.clone(),
            memory.clone(),
            cwd.clone(),
            spawn_tx,
        ));

        // Register deep research tool with background notification channel
        let (research_tx, mut research_rx) =
            tokio::sync::mpsc::channel::<crew_agent::ResearchNotification>(8);
        tools.register(crew_agent::DeepResearchTool::new(
            llm.clone(),
            memory.clone(),
            data_dir.clone(),
            research_tx,
        ));

        // Create memory store and register memory bank tools
        let memory_store = Arc::new(
            MemoryStore::open(&data_dir)
                .await
                .wrap_err("failed to open memory store")?,
        );
        tools.register(crew_agent::RecallMemoryTool::new(memory_store.clone()));
        tools.register(crew_agent::SaveMemoryTool::new(memory_store.clone()));

        // Register MCP tools
        if !config.mcp_servers.is_empty() {
            match crew_agent::McpClient::start(&config.mcp_servers).await {
                Ok(client) => client.register_tools(&mut tools),
                Err(e) => eprintln!("Warning: MCP initialization failed: {e}"),
            }
        }

        // Load plugins
        let plugin_dirs = Config::plugin_dirs(&cwd);
        if !plugin_dirs.is_empty() {
            if let Err(e) = crew_agent::PluginLoader::load_into(&mut tools, &plugin_dirs) {
                eprintln!("Warning: plugin loading failed: {e}");
            }
        }

        // Apply tool policy from config
        if let Some(ref policy) = config.tool_policy {
            tools.apply_policy(policy);
        }

        // Apply context-based tag filter
        if !config.context_filter.is_empty() {
            tools.set_context_filter(config.context_filter.clone());
        }

        // Apply provider-specific tool policy
        if let Some(policy) = resolve_provider_policy(&config, &provider_name, &model_id) {
            tools.set_provider_policy(policy);
        }

        // Set up Ctrl+C handler
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                shutdown_clone.store(true, Ordering::Release);
            }
        });

        // Create agent
        let reporter = Arc::new(ConsoleReporter::new().with_verbose(self.verbose));
        let agent_config = AgentConfig {
            max_iterations: self.max_iterations,
            save_episodes: true,
            ..Default::default()
        };
        let mut agent = Agent::new(AgentId::new("chat"), llm, tools, memory)
            .with_config(agent_config)
            .with_reporter(reporter)
            .with_shutdown(shutdown.clone());

        // Load bootstrap files (AGENTS.md, SOUL.md, etc.) from project .crew/ directory
        let project_dir = cwd.join(".crew");
        let bootstrap = super::load_bootstrap_files(&project_dir);
        if !bootstrap.is_empty() {
            agent.append_system_prompt(&bootstrap);
        }

        // Inject memory context (long-term + daily notes)
        let memory_ctx = memory_store.get_memory_context().await;
        if !memory_ctx.is_empty() {
            agent.append_system_prompt(&memory_ctx);
        }

        // Inject memory bank summary (entity abstracts)
        let bank_summary = memory_store.get_bank_summary().await;
        if !bank_summary.is_empty() {
            agent.append_system_prompt(&bank_summary);
        }

        if !config.hooks.is_empty() {
            agent = agent.with_hooks(Arc::new(HookExecutor::new(config.hooks.clone())));
        }

        if let Some(embedder) = create_embedder(&config) {
            agent = agent.with_embedder(embedder);
        }

        // Single-message mode: send one message and exit
        if let Some(msg) = self.message {
            let response = agent.process_message(&msg, &[], vec![]).await?;
            if !response.streamed {
                println!("{}", response.content);
            }
            return Ok(());
        }

        // Set up readline
        let history_dir = data_dir.join("history");
        std::fs::create_dir_all(&history_dir).ok();
        let history_path = history_dir.join("chat_history");

        let mut rl = DefaultEditor::new().wrap_err("failed to initialize readline")?;
        let _ = rl.load_history(&history_path);

        // Banner
        println!("{}", "crew-rs chat".cyan().bold());
        println!("{}", "(type /exit or Ctrl+C to quit)".dimmed());
        println!();

        // Conversation history
        let mut history: Vec<Message> = Vec::new();

        // Interactive loop using tokio::select! to handle both user input
        // and background research notifications concurrently.
        //
        // readline is blocking, so we run it on a blocking thread and receive
        // the result via a oneshot channel. This lets us print notifications
        // even while waiting for user input.
        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            // Spawn blocking readline on a separate thread
            let (line_tx, line_rx) = tokio::sync::oneshot::channel();
            let mut rl_moved = rl;
            let readline_handle = tokio::task::spawn_blocking(move || {
                let result = rl_moved.readline("you> ");
                let _ = line_tx.send(result);
                rl_moved
            });

            // Wait for either user input or background notification
            let readline_result;
            let mut line_rx = line_rx;
            loop {
                tokio::select! {
                    result = &mut line_rx => {
                        readline_result = result.unwrap_or(Err(
                            rustyline::error::ReadlineError::Eof
                        ));
                        break;
                    }
                    Some(notif) = research_rx.recv() => {
                        // Print notification while user is at the prompt
                        println!();
                        if notif.success {
                            println!(
                                "{} Research complete: {}",
                                "✓".green().bold(),
                                notif.question
                            );
                            println!("  Report saved to: {}", notif.report_path.display());
                        } else {
                            println!(
                                "{} Research failed: {}",
                                "✗".red().bold(),
                                notif.summary
                            );
                        }
                        println!();
                        // Continue waiting for user input
                    }
                }
            }

            // Recover the Editor from the blocking thread
            rl = readline_handle.await.unwrap_or_else(|_| {
                rustyline::DefaultEditor::new().expect("failed to create editor")
            });

            let line = match readline_result {
                Ok(line) => line,
                Err(
                    rustyline::error::ReadlineError::Interrupted
                    | rustyline::error::ReadlineError::Eof,
                ) => {
                    break;
                }
                Err(e) => {
                    eprintln!("Input error: {e}");
                    break;
                }
            };

            let input = line.trim();
            if input.is_empty() {
                continue;
            }

            rl.add_history_entry(input).ok();

            if EXIT_COMMANDS.contains(&input.to_lowercase().as_str()) {
                break;
            }

            // Process message (may return quickly for background research)
            let response = agent.process_message(input, &history, vec![]).await?;

            // Drain any notifications that arrived during processing
            while let Ok(notif) = research_rx.try_recv() {
                println!();
                if notif.success {
                    println!(
                        "{} Research complete: {}",
                        "✓".green().bold(),
                        notif.question
                    );
                    println!("  Report saved to: {}", notif.report_path.display());
                } else {
                    println!(
                        "{} Research failed: {}",
                        "✗".red().bold(),
                        notif.summary
                    );
                }
            }

            // Append to history
            history.push(Message {
                role: MessageRole::User,
                content: input.to_string(),
                media: vec![],
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                timestamp: chrono::Utc::now(),
            });
            history.push(Message {
                role: MessageRole::Assistant,
                content: response.content.clone(),
                media: vec![],
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                timestamp: chrono::Utc::now(),
            });

            // Print response (skip if already streamed to console)
            if !response.streamed {
                println!();
                println!("{}: {}", "assistant".blue().bold(), response.content);
            }
            println!();
        }

        // Save history
        let _ = rl.save_history(&history_path);
        println!("{}", "Goodbye!".dimmed());

        Ok(())
    }
}

/// Find the matching provider-specific tool policy for the active model.
/// Checks model ID first (e.g. "claude-sonnet-4-20250514"), then provider name (e.g. "gemini").
pub(crate) fn resolve_provider_policy(
    config: &Config,
    provider_name: &str,
    model_id: &str,
) -> Option<crew_agent::ToolPolicy> {
    if config.tool_policy_by_provider.is_empty() {
        return None;
    }
    // Exact model ID match first
    if let Some(policy) = config.tool_policy_by_provider.get(model_id) {
        return Some(policy.clone());
    }
    // Provider name match
    if let Some(policy) = config.tool_policy_by_provider.get(provider_name) {
        return Some(policy.clone());
    }
    None
}

/// Create an embedding provider from config, if configured.
pub(crate) fn create_embedder(config: &Config) -> Option<Arc<dyn EmbeddingProvider>> {
    let cfg = config.embedding.as_ref()?;
    let key = config.get_api_key(&cfg.provider).ok()?;
    let mut e = OpenAIEmbedder::new(key);
    if let Some(ref url) = cfg.base_url {
        e = e.with_base_url(url);
    }
    Some(Arc::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_provider_policy_model_id_match() {
        let json = r#"{
            "tool_policy_by_provider": {
                "gemini": {"deny": ["diff_edit"]},
                "claude-sonnet-4-20250514": {"allow": ["shell"]}
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let policy =
            resolve_provider_policy(&config, "anthropic", "claude-sonnet-4-20250514").unwrap();
        assert!(policy.is_allowed("shell"));
        assert!(!policy.is_allowed("read_file"));
    }

    #[test]
    fn test_resolve_provider_policy_provider_fallback() {
        let json = r#"{
            "tool_policy_by_provider": {
                "gemini": {"deny": ["diff_edit"]}
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let policy = resolve_provider_policy(&config, "gemini", "gemini-2.0-flash").unwrap();
        assert!(!policy.is_allowed("diff_edit"));
        assert!(policy.is_allowed("shell"));
    }

    #[test]
    fn test_resolve_provider_policy_none() {
        let config = Config::default();
        assert!(
            resolve_provider_policy(&config, "anthropic", "claude-sonnet-4-20250514").is_none()
        );
    }
}

/// Create an LLM provider from name and config.
pub(crate) fn create_provider(
    name: &str,
    config: &Config,
    model: Option<String>,
    base_url: Option<String>,
) -> Result<Arc<dyn LlmProvider>> {
    let provider: Arc<dyn LlmProvider> = match name {
        "anthropic" => {
            let api_key = config.get_api_key("anthropic")?;
            let model_name = model.unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());
            let mut p = AnthropicProvider::new(&api_key, &model_name);
            if let Some(url) = &base_url {
                p = p.with_base_url(url);
            }
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "openai" => {
            let api_key = config.get_api_key("openai")?;
            let model_name = model.unwrap_or_else(|| "gpt-4o".to_string());
            let mut p = OpenAIProvider::new(&api_key, &model_name);
            if let Some(url) = &base_url {
                p = p.with_base_url(url);
            }
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "gemini" | "google" => {
            let api_key = config.get_api_key("gemini")?;
            let model_name = model.unwrap_or_else(|| "gemini-2.0-flash".to_string());
            let mut p = GeminiProvider::new(&api_key, &model_name);
            if let Some(url) = &base_url {
                p = p.with_base_url(url);
            }
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "openrouter" => {
            let api_key = config.get_api_key("openrouter")?;
            let model_name =
                model.unwrap_or_else(|| "anthropic/claude-sonnet-4-20250514".to_string());
            let mut p = OpenRouterProvider::new(&api_key, &model_name);
            if let Some(url) = &base_url {
                p = p.with_base_url(url);
            }
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "deepseek" => {
            let api_key = config.get_api_key("deepseek")?;
            let model_name = model.unwrap_or_else(|| "deepseek-chat".to_string());
            let p = OpenAIProvider::new(&api_key, &model_name)
                .with_base_url(base_url.as_deref().unwrap_or("https://api.deepseek.com/v1"));
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "groq" => {
            let api_key = config.get_api_key("groq")?;
            let model_name = model.unwrap_or_else(|| "llama-3.3-70b-versatile".to_string());
            let p = OpenAIProvider::new(&api_key, &model_name).with_base_url(
                base_url
                    .as_deref()
                    .unwrap_or("https://api.groq.com/openai/v1"),
            );
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "moonshot" | "kimi" => {
            let api_key = config.get_api_key("moonshot")?;
            let model_name = model.unwrap_or_else(|| "kimi-k2.5".to_string());
            let p = OpenAIProvider::new(&api_key, &model_name)
                .with_base_url(base_url.as_deref().unwrap_or("https://api.moonshot.ai/v1"));
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "dashscope" | "qwen" => {
            let api_key = config.get_api_key("dashscope")?;
            let model_name = model.unwrap_or_else(|| "qwen-max".to_string());
            let p = OpenAIProvider::new(&api_key, &model_name).with_base_url(
                base_url
                    .as_deref()
                    .unwrap_or("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            );
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "minimax" => {
            let api_key = config.get_api_key("minimax")?;
            let model_name = model.unwrap_or_else(|| "MiniMax-Text-01".to_string());
            let p = OpenAIProvider::new(&api_key, &model_name)
                .with_base_url(base_url.as_deref().unwrap_or("https://api.minimax.io/v1"));
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "zhipu" | "glm" => {
            let api_key = config.get_api_key("zhipu")?;
            let model_name = model.unwrap_or_else(|| "glm-4-plus".to_string());
            let p = OpenAIProvider::new(&api_key, &model_name).with_base_url(
                base_url
                    .as_deref()
                    .unwrap_or("https://open.bigmodel.cn/api/paas/v4"),
            );
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "zai" | "z.ai" => {
            let api_key = config.get_api_key("zai")?;
            let model_name = model.unwrap_or_else(|| "glm-5".to_string());
            let mut p = AnthropicProvider::new(&api_key, &model_name);
            p = p.with_base_url(
                base_url
                    .as_deref()
                    .unwrap_or("https://api.z.ai/api/anthropic"),
            );
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "nvidia" | "nim" => {
            let api_key = config.get_api_key("nvidia")?;
            let model_name = model.unwrap_or_else(|| "meta/llama-3.3-70b-instruct".to_string());
            let p = OpenAIProvider::new(&api_key, &model_name).with_base_url(
                base_url
                    .as_deref()
                    .unwrap_or("https://integrate.api.nvidia.com/v1"),
            );
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "ollama" => {
            let model_name = model.unwrap_or_else(|| "llama3.2".to_string());
            let p = OpenAIProvider::new("ollama", &model_name)
                .with_base_url(base_url.as_deref().unwrap_or("http://localhost:11434/v1"));
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        "vllm" => {
            let api_key = config
                .get_api_key("vllm")
                .unwrap_or_else(|_| "token".to_string());
            let model_name = model
                .ok_or_else(|| eyre::eyre!("vllm provider requires --model to be specified"))?;
            let url = base_url
                .ok_or_else(|| eyre::eyre!("vllm provider requires --base-url to be specified"))?;
            let p = OpenAIProvider::new(&api_key, &model_name).with_base_url(&url);
            println!("{}: {}", "Model".green(), p.model_id());
            Arc::new(p)
        }
        other => {
            eyre::bail!(
                "unknown provider: {other}. Valid: anthropic, openai, gemini, openrouter, \
                 deepseek, groq, moonshot, dashscope, minimax, zhipu, zai, nvidia, ollama, vllm"
            );
        }
    };
    Ok(provider)
}
