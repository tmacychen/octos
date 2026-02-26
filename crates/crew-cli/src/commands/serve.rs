//! Serve command: start the REST API server.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;
use colored::Colorize;
use crew_agent::{Agent, AgentConfig, HookExecutor, ToolRegistry};
use crew_bus::SessionManager;
use crew_core::AgentId;
use crew_llm::{LlmProvider, RetryProvider};
use crew_memory::EpisodeStore;
use eyre::{Result, WrapErr};

use super::Executable;
use super::chat::create_provider;
use crate::api::{AppState, SseBroadcaster, build_router, init_metrics};
use crate::config::Config;

/// Start the REST API server.
#[derive(Debug, Args)]
pub struct ServeCommand {
    /// Port to listen on.
    #[arg(short, long, default_value = "8080")]
    pub port: u16,

    /// Host address to bind to. Defaults to localhost for security.
    /// Use 0.0.0.0 to accept connections from all interfaces.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

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

    /// Auth token for API access (overrides config).
    #[arg(long)]
    pub auth_token: Option<String>,

    /// Disable automatic retry on transient errors.
    #[arg(long)]
    pub no_retry: bool,
}

impl Executable for ServeCommand {
    fn execute(self) -> Result<()> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .wrap_err("failed to create tokio runtime")?
            .block_on(self.run_async())
    }
}

impl ServeCommand {
    async fn run_async(self) -> Result<()> {
        let cwd = match &self.cwd {
            Some(p) => p.clone(),
            None => std::env::current_dir().wrap_err("failed to get current directory")?,
        };

        let config = if let Some(config_path) = &self.config {
            Config::from_file(config_path)?
        } else {
            Config::load(&cwd)?
        };

        // Resolve data directory (--data-dir > $CREW_HOME > ~/.crew)
        let data_dir = super::resolve_data_dir(self.data_dir.clone())?;

        // Try to create the LLM provider + agent, but don't fail if no API key.
        // The admin dashboard works without it.
        let agent_and_sessions = self.try_create_agent(&config, &cwd, &data_dir).await;

        let (agent, sessions) = match agent_and_sessions {
            Ok((a, s)) => (Some(Arc::new(a)), Some(s)),
            Err(e) => {
                tracing::warn!("LLM agent not available: {e}");
                tracing::info!("Admin dashboard will still work. Configure profiles via /admin/");
                (None, None)
            }
        };

        let broadcaster = Arc::new(SseBroadcaster::new(256));
        let metrics_handle = Some(init_metrics());

        // Security: warn if binding to non-localhost without auth token
        let auth_token = if self.auth_token.is_some() {
            self.auth_token
        } else if self.host != "127.0.0.1" && self.host != "localhost" && self.host != "::1" {
            tracing::warn!(
                "Binding to {} without --auth-token is dangerous! \
                 Generating a random token for this session.",
                self.host
            );
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            std::time::SystemTime::now().hash(&mut h);
            std::process::id().hash(&mut h);
            let token = format!(
                "{:016x}{:016x}",
                h.finish(),
                h.finish().wrapping_mul(6364136223846793005)
            );
            println!(
                "{}: {} (auto-generated, pass --auth-token to set your own)",
                "Auth token".yellow(),
                token
            );
            Some(token)
        } else {
            None
        };

        // Initialize profile store and process manager for admin dashboard
        let profile_store = Arc::new(
            crate::profiles::ProfileStore::open(&data_dir)
                .wrap_err("failed to open profile store")?,
        );
        let process_manager = Arc::new(crate::process_manager::ProcessManager::new(
            profile_store.clone(),
        ));

        let state = Arc::new(AppState {
            agent,
            sessions,
            broadcaster,
            started_at: chrono::Utc::now(),
            auth_token,
            metrics_handle,
            profile_store: Some(profile_store.clone()),
            process_manager: Some(process_manager.clone()),
        });

        // Auto-start enabled profiles
        let profiles = profile_store.list().unwrap_or_default();
        let enabled_count = profiles.iter().filter(|p| p.enabled).count();
        if enabled_count > 0 {
            for p in &profiles {
                if p.enabled {
                    if let Err(e) = process_manager.start(p).await {
                        tracing::warn!(profile = %p.id, error = %e, "failed to auto-start gateway");
                    }
                }
            }
        }

        let app = build_router(state);
        let addr = format!("{}:{}", self.host, self.port);

        println!("{}", "crew-rs API server".cyan().bold());
        println!("{}: http://{}", "Listening".green(), addr);
        println!("{}: http://{}/admin/", "Dashboard".green(), addr);
        if enabled_count > 0 {
            println!(
                "{}: {} profiles auto-started",
                "Gateways".green(),
                enabled_count
            );
        }
        println!();

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }

    /// Try to create an Agent + SessionManager. Returns Err if API key is missing etc.
    async fn try_create_agent(
        &self,
        config: &Config,
        cwd: &std::path::Path,
        data_dir: &std::path::Path,
    ) -> Result<(Agent, Arc<tokio::sync::Mutex<SessionManager>>)> {
        let model = self.model.clone().or(config.model.clone());
        let base_url = config.base_url.clone();
        let provider_name = self
            .provider
            .clone()
            .or(config.provider.clone())
            .or_else(|| {
                model
                    .as_deref()
                    .and_then(crate::config::detect_provider)
                    .map(String::from)
            })
            .unwrap_or_else(|| "anthropic".to_string());

        let base_provider: Arc<dyn LlmProvider> =
            create_provider(&provider_name, config, model, base_url)?;

        let llm: Arc<dyn LlmProvider> = if self.no_retry {
            base_provider
        } else {
            Arc::new(RetryProvider::new(base_provider))
        };

        let memory = Arc::new(
            EpisodeStore::open(data_dir)
                .await
                .wrap_err("failed to open episode store")?,
        );

        let sandbox = crew_agent::create_sandbox(&config.sandbox);
        let mut tools = ToolRegistry::with_builtins_and_sandbox(cwd, sandbox);
        tools.register(crew_agent::DeepSearchTool::new(data_dir.join("research")));

        // MCP tools
        if !config.mcp_servers.is_empty() {
            match crew_agent::McpClient::start(&config.mcp_servers).await {
                Ok(client) => client.register_tools(&mut tools),
                Err(e) => tracing::warn!("MCP initialization failed: {e}"),
            }
        }

        // Plugins
        let plugin_dirs = Config::plugin_dirs(cwd);
        if !plugin_dirs.is_empty() {
            let _ = crew_agent::PluginLoader::load_into(&mut tools, &plugin_dirs);
        }

        let mut agent =
            Agent::new(AgentId::new("api"), llm, tools, memory).with_config(AgentConfig {
                max_iterations: 20,
                save_episodes: false,
                ..Default::default()
            });

        if !config.hooks.is_empty() {
            agent = agent.with_hooks(Arc::new(HookExecutor::new(config.hooks.clone())));
        }

        let sessions = Arc::new(tokio::sync::Mutex::new(
            SessionManager::open(data_dir).wrap_err("failed to open session manager")?,
        ));

        Ok((agent, sessions))
    }
}
