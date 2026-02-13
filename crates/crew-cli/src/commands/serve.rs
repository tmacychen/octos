//! Serve command: start the REST API server.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;
use colored::Colorize;
use crew_agent::{Agent, AgentConfig, ToolRegistry};
use crew_bus::SessionManager;
use crew_core::AgentId;
use crew_llm::{LlmProvider, RetryProvider};
use crew_memory::EpisodeStore;
use eyre::{Result, WrapErr};

use super::Executable;
use super::chat::create_provider;
use crate::api::{AppState, SseBroadcaster, build_router};
use crate::config::Config;

/// Start the REST API server.
#[derive(Debug, Args)]
pub struct ServeCommand {
    /// Port to listen on.
    #[arg(short, long, default_value = "8080")]
    pub port: u16,

    /// Working directory (defaults to current directory).
    #[arg(short, long)]
    pub cwd: Option<PathBuf>,

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
        let cwd = self.cwd.unwrap_or_else(|| std::env::current_dir().unwrap());

        let config = if let Some(config_path) = &self.config {
            Config::from_file(config_path)?
        } else {
            Config::load(&cwd)?
        };

        let model = self.model.or(config.model.clone());
        let base_url = config.base_url.clone();
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

        let base_provider: Arc<dyn LlmProvider> =
            create_provider(&provider_name, &config, model, base_url)?;

        let llm: Arc<dyn LlmProvider> = if self.no_retry {
            base_provider
        } else {
            Arc::new(RetryProvider::new(base_provider))
        };

        let data_dir = cwd.join(".crew");
        let memory = Arc::new(
            EpisodeStore::open(&data_dir)
                .await
                .wrap_err("failed to open episode store")?,
        );

        let sandbox = crew_agent::create_sandbox(&config.sandbox);
        let mut tools = ToolRegistry::with_builtins_and_sandbox(&cwd, sandbox);

        // MCP tools
        if !config.mcp_servers.is_empty() {
            match crew_agent::McpClient::start(&config.mcp_servers).await {
                Ok(client) => client.register_tools(&mut tools),
                Err(e) => tracing::warn!("MCP initialization failed: {e}"),
            }
        }

        // Plugins
        let plugin_dirs = Config::plugin_dirs(&cwd);
        if !plugin_dirs.is_empty() {
            let _ = crew_agent::PluginLoader::load_into(&mut tools, &plugin_dirs);
        }

        let broadcaster = Arc::new(SseBroadcaster::new(256));
        let agent = Agent::new(AgentId::new("api"), llm, tools, memory)
            .with_config(AgentConfig {
                max_iterations: 20,
                save_episodes: false,
                ..Default::default()
            })
            .with_reporter(broadcaster.clone());

        let sessions = Arc::new(tokio::sync::Mutex::new(
            SessionManager::open(&data_dir).wrap_err("failed to open session manager")?,
        ));

        let auth_token = self.auth_token;

        let state = Arc::new(AppState {
            agent: Arc::new(agent),
            sessions,
            broadcaster,
            started_at: chrono::Utc::now(),
            auth_token,
        });

        let app = build_router(state);
        let addr = format!("0.0.0.0:{}", self.port);

        println!("{}", "crew-rs API server".cyan().bold());
        println!("{}: http://{}", "Listening".green(), addr);
        println!();

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}
