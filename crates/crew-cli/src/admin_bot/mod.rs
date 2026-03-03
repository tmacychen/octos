//! Admin bot: in-process LLM-powered administration via Telegram and Feishu.
//!
//! Runs inside `crew serve` with direct `Arc` access to `ProfileStore`,
//! `ProcessManager`, and `UserStore`. Provides natural language admin
//! via 10 custom tools, plus a watchdog that auto-restarts crashed gateways.

pub mod monitor;
pub mod tools;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use crew_agent::{Agent, AgentConfig, SilentReporter, ToolRegistry};
use crew_core::{AgentId, Message, MessageRole};
use crew_llm::{LlmProvider, RetryProvider};
use crew_memory::EpisodeStore;
use eyre::{Result, WrapErr};
use tokio::sync::{Mutex, mpsc};

use crate::config::AdminBotConfig;
use crate::process_manager::ProcessManager;
use crate::profiles::ProfileStore;

use self::monitor::{AdminAlert, FeishuAlertSender, Monitor, TelegramAlertSender};
use self::tools::{AdminContext, register_admin_tools};

const ADMIN_SYSTEM_PROMPT: &str = "\
You are the admin assistant for crew.rs, an AI agent platform.
You help the admin manage profiles (AI agent instances), monitor system health,
and handle operational tasks.

You have tools to:
- List profiles and their status (running/stopped, uptime, PID)
- Start, stop, restart profiles
- Enable/disable profiles for auto-start
- View real-time logs from any profile
- Check system-wide health and provider metrics
- Manage the watchdog (auto-restart crashed gateways)

Be concise. Format status information clearly. When showing profile lists,
use a compact format. Proactively suggest actions when you spot issues.
When a user first messages you without being authorized, tell them their ID \
so they can add it to the config.";

/// Incoming message from any channel, unified for the agent loop.
struct IncomingMessage {
    sender_id: String,
    text: String,
    reply: ReplyTarget,
}

/// How to reply to a message.
#[allow(dead_code)]
enum ReplyTarget {
    Telegram {
        bot: teloxide::Bot,
        chat_id: teloxide::types::ChatId,
    },
    Feishu {
        http: reqwest::Client,
        base_url: String,
        token: String,
        chat_id: String,
    },
}

impl ReplyTarget {
    async fn send(&self, text: &str) {
        match self {
            Self::Telegram { bot, chat_id } => {
                use teloxide::requests::Requester;
                // Telegram has a 4096 char limit; split if needed
                for chunk in split_message(text, 4000) {
                    if let Err(e) = bot.send_message(*chat_id, &chunk).await {
                        tracing::warn!(error = %e, "failed to send Telegram reply");
                    }
                }
            }
            Self::Feishu {
                http,
                base_url,
                token,
                chat_id,
            } => {
                let url = format!("{base_url}/open-apis/im/v1/messages");
                let body = serde_json::json!({
                    "receive_id": chat_id,
                    "msg_type": "text",
                    "content": serde_json::json!({"text": text}).to_string(),
                });
                if let Err(e) = http
                    .post(&url)
                    .query(&[("receive_id_type", "chat_id")])
                    .bearer_auth(token)
                    .json(&body)
                    .send()
                    .await
                {
                    tracing::warn!(error = %e, "failed to send Feishu reply");
                }
            }
        }
    }
}

pub struct AdminBot {
    agent: Agent,
    telegram_bot: Option<teloxide::Bot>,
    feishu_config: Option<FeishuConfig>,
    admin_chat_ids: HashSet<i64>,
    admin_feishu_ids: HashSet<String>,
    monitor_config: MonitorConfig,
    profile_store: Arc<ProfileStore>,
    process_manager: Arc<ProcessManager>,
    sessions: Arc<Mutex<HashMap<String, Vec<Message>>>>,
    shutdown: Arc<AtomicBool>,
    #[allow(dead_code)]
    admin_ctx: Arc<AdminContext>,
}

struct FeishuConfig {
    app_id: String,
    app_secret: String,
    region: String,
}

struct MonitorConfig {
    watchdog_enabled: Arc<AtomicBool>,
    alerts_enabled: Arc<AtomicBool>,
    max_restart_attempts: u32,
    health_check_interval: Duration,
}

impl AdminBot {
    /// Create a new admin bot from config and shared infrastructure.
    pub async fn new(
        config: &AdminBotConfig,
        profile_store: Arc<ProfileStore>,
        process_manager: Arc<ProcessManager>,
        shutdown: Arc<AtomicBool>,
        data_dir: &std::path::Path,
    ) -> Result<Self> {
        // Build LLM provider for admin bot
        let provider_name = config.provider.as_deref().unwrap_or("openai");
        let model = config.model.clone();
        let base_url = config.base_url.clone();

        // Build a minimal Config with the admin bot's API key env
        let llm_config = crate::config::Config {
            api_key_env: config.api_key_env.clone(),
            ..Default::default()
        };

        let base_provider: Arc<dyn LlmProvider> =
            crate::commands::chat::create_provider(provider_name, &llm_config, model, base_url)?;
        let llm: Arc<dyn LlmProvider> = Arc::new(RetryProvider::new(base_provider));

        // Build episode store
        let memory = Arc::new(
            EpisodeStore::open(data_dir)
                .await
                .wrap_err("failed to open episode store for admin bot")?,
        );

        // Build admin tools
        let watchdog_enabled = Arc::new(AtomicBool::new(config.watchdog_enabled));
        let alerts_enabled = Arc::new(AtomicBool::new(config.alerts_enabled));

        let admin_ctx = Arc::new(AdminContext {
            profile_store: profile_store.clone(),
            process_manager: process_manager.clone(),
            watchdog_enabled: watchdog_enabled.clone(),
            alerts_enabled: alerts_enabled.clone(),
            server_started_at: Utc::now(),
        });

        let mut tools = ToolRegistry::new();
        register_admin_tools(&mut tools, admin_ctx.clone());

        let reporter: Arc<dyn crew_agent::ProgressReporter> = Arc::new(SilentReporter);
        let agent = Agent::new(AgentId::new("admin-bot"), llm, tools, memory)
            .with_config(AgentConfig {
                max_iterations: 10,
                save_episodes: false,
                max_timeout: Some(Duration::from_secs(120)),
                ..Default::default()
            })
            .with_reporter(reporter)
            .with_system_prompt(ADMIN_SYSTEM_PROMPT.to_string())
            .with_shutdown(shutdown.clone());

        // Set up Telegram bot if token env is configured
        let telegram_bot = config
            .telegram_token_env
            .as_ref()
            .and_then(|env_name| std::env::var(env_name).ok().map(teloxide::Bot::new));

        // Set up Feishu config if app ID env is configured
        let feishu_config = config.feishu_app_id_env.as_ref().and_then(|id_env| {
            let app_id = std::env::var(id_env).ok()?;
            let secret_env = config
                .feishu_app_secret_env
                .as_deref()
                .unwrap_or("ADMIN_FEISHU_APP_SECRET");
            let app_secret = std::env::var(secret_env).ok()?;
            Some(FeishuConfig {
                app_id,
                app_secret,
                region: "cn".to_string(),
            })
        });

        let monitor_config = MonitorConfig {
            watchdog_enabled,
            alerts_enabled,
            max_restart_attempts: config.max_restart_attempts,
            health_check_interval: Duration::from_secs(config.health_check_interval_secs),
        };

        Ok(Self {
            agent,
            telegram_bot,
            feishu_config,
            admin_chat_ids: config.admin_chat_ids.iter().copied().collect(),
            admin_feishu_ids: config.admin_feishu_ids.iter().cloned().collect(),
            monitor_config,
            profile_store,
            process_manager,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            shutdown,
            admin_ctx,
        })
    }

    /// Run the admin bot. Consumes self. Returns when shutdown is signaled.
    pub async fn run(self) -> Result<()> {
        let (msg_tx, mut msg_rx) = mpsc::channel::<IncomingMessage>(64);
        let (alert_tx, alert_rx) = mpsc::channel::<AdminAlert>(256);

        // Wire alert sender into process manager
        self.process_manager.set_alert_sender(alert_tx.clone());

        // Start monitor (watchdog + health checks + alerts)
        let mut monitor = Monitor::new(
            self.profile_store.clone(),
            self.process_manager.clone(),
            alert_rx,
            self.monitor_config.watchdog_enabled.clone(),
            self.monitor_config.alerts_enabled.clone(),
            self.monitor_config.max_restart_attempts,
            self.monitor_config.health_check_interval,
            self.shutdown.clone(),
        );

        // Add alert senders
        if let Some(ref bot) = self.telegram_bot {
            if !self.admin_chat_ids.is_empty() {
                let chat_ids: Vec<i64> = self.admin_chat_ids.iter().copied().collect();
                monitor.add_sender(Box::new(TelegramAlertSender::new(bot.clone(), chat_ids)));
            }
        }
        if let Some(ref feishu) = self.feishu_config {
            if !self.admin_feishu_ids.is_empty() {
                monitor.add_sender(Box::new(FeishuAlertSender::new(
                    feishu.app_id.clone(),
                    feishu.app_secret.clone(),
                    self.admin_feishu_ids.iter().cloned().collect(),
                    &feishu.region,
                )));
            }
        }

        tokio::spawn(async move { monitor.run().await });

        // Spawn Telegram polling task
        if let Some(bot) = self.telegram_bot.clone() {
            let tx = msg_tx.clone();
            let shutdown = self.shutdown.clone();
            tokio::spawn(async move {
                Self::telegram_polling(bot, tx, shutdown).await;
            });
        }

        // Spawn Feishu WebSocket listener task
        // (Feishu admin channel is a future enhancement — for now only Telegram is supported)

        // Main message loop
        let sessions = self.sessions;
        let agent = self.agent;
        let admin_chat_ids = self.admin_chat_ids;
        let shutdown = self.shutdown;

        tracing::info!("admin bot started");

        while let Some(incoming) = msg_rx.recv().await {
            if shutdown.load(Ordering::Acquire) {
                break;
            }

            // Auth check for Telegram
            if let ReplyTarget::Telegram { chat_id, .. } = &incoming.reply {
                if !admin_chat_ids.contains(&chat_id.0) {
                    incoming.reply.send(&format!(
                        "Unauthorized. Your chat ID is `{}`. Add it to `admin_bot.admin_chat_ids` in config.",
                        chat_id.0
                    )).await;
                    continue;
                }
            }

            // Get/create conversation history
            let mut sessions_lock = sessions.lock().await;
            let history = sessions_lock.entry(incoming.sender_id.clone()).or_default();

            // Process message through agent
            match agent.process_message(&incoming.text, history, vec![]).await {
                Ok(response) => {
                    // Append to history
                    history.push(Message {
                        role: MessageRole::User,
                        content: incoming.text,
                        media: vec![],
                        tool_calls: None,
                        tool_call_id: None,
                        reasoning_content: None,
                        timestamp: Utc::now(),
                    });
                    history.push(Message {
                        role: MessageRole::Assistant,
                        content: response.content.clone(),
                        media: vec![],
                        tool_calls: None,
                        tool_call_id: None,
                        reasoning_content: None,
                        timestamp: Utc::now(),
                    });

                    // Trim history to last 20 messages
                    if history.len() > 20 {
                        let drain_count = history.len() - 20;
                        history.drain(..drain_count);
                    }

                    incoming.reply.send(&response.content).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "admin bot agent error");
                    incoming.reply.send(&format!("Error: {e}")).await;
                }
            }
        }

        tracing::info!("admin bot stopped");
        Ok(())
    }

    /// Telegram long polling loop.
    async fn telegram_polling(
        bot: teloxide::Bot,
        tx: mpsc::Sender<IncomingMessage>,
        shutdown: Arc<AtomicBool>,
    ) {
        use teloxide::payloads::GetUpdatesSetters;
        use teloxide::requests::Requester;
        use teloxide::types::UpdateKind;

        let mut offset: i32 = 0;

        loop {
            if shutdown.load(Ordering::Acquire) {
                break;
            }

            // Manual long polling with timeout
            let updates = match bot.get_updates().offset(offset).timeout(30).await {
                Ok(updates) => updates,
                Err(e) => {
                    tracing::warn!(error = %e, "admin bot Telegram polling error");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            for update in updates {
                offset = update.id.as_offset();

                if let UpdateKind::Message(msg) = update.kind {
                    let text = match msg.text() {
                        Some(t) => t.to_string(),
                        None => continue,
                    };
                    let chat_id = msg.chat.id;
                    let sender_id = format!("tg:{}", chat_id.0);

                    let _ = tx
                        .send(IncomingMessage {
                            sender_id,
                            text,
                            reply: ReplyTarget::Telegram {
                                bot: bot.clone(),
                                chat_id,
                            },
                        })
                        .await;
                }
            }
        }
    }
}

/// Split a message into chunks for channel limits.
fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }
        // Find a good split point (newline or space)
        let split_at = remaining[..max_len]
            .rfind('\n')
            .or_else(|| remaining[..max_len].rfind(' '))
            .unwrap_or(max_len);
        chunks.push(remaining[..split_at].to_string());
        remaining = remaining[split_at..].trim_start();
    }
    chunks
}
