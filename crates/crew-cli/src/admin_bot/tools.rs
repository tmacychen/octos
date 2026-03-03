//! Admin tools implementing the `crew_agent::Tool` trait.
//!
//! Each tool has `Arc<AdminContext>` for shared access to ProfileStore,
//! ProcessManager, and watchdog/alert flags.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crew_agent::tools::{Tool, ToolResult};
use eyre::Result;
use serde::Deserialize;

use crate::process_manager::ProcessManager;
use crate::profiles::ProfileStore;

/// Shared context for all admin tools.
pub struct AdminContext {
    pub profile_store: Arc<ProfileStore>,
    pub process_manager: Arc<ProcessManager>,
    pub watchdog_enabled: Arc<AtomicBool>,
    pub alerts_enabled: Arc<AtomicBool>,
    pub server_started_at: DateTime<Utc>,
}

// ── admin_list_profiles ────────────────────────────────────────────────

pub struct ListProfilesTool {
    ctx: Arc<AdminContext>,
}

impl ListProfilesTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[derive(Deserialize, Default)]
struct ListProfilesInput {
    filter: Option<String>,
}

#[async_trait]
impl Tool for ListProfilesTool {
    fn name(&self) -> &str {
        "admin_list_profiles"
    }
    fn description(&self) -> &str {
        "List all profiles with their status (running/stopped, PID, uptime). Optional filter: running, stopped, enabled, disabled."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "enum": ["running", "stopped", "enabled", "disabled"],
                    "description": "Filter profiles by status"
                }
            }
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: ListProfilesInput = serde_json::from_value(args.clone()).unwrap_or_default();
        let profiles = self.ctx.profile_store.list().unwrap_or_default();
        let statuses = self.ctx.process_manager.all_statuses().await;

        let mut lines = Vec::new();
        for p in &profiles {
            let status = statuses.get(&p.id);
            let running = status.is_some_and(|s| s.running);

            // Apply filter
            match input.filter.as_deref() {
                Some("running") if !running => continue,
                Some("stopped") if running => continue,
                Some("enabled") if !p.enabled => continue,
                Some("disabled") if p.enabled => continue,
                _ => {}
            }

            let state = if running { "RUNNING" } else { "STOPPED" };
            let pid = status
                .and_then(|s| s.pid)
                .map(|p| format!("PID {p}"))
                .unwrap_or_default();
            let uptime = status
                .and_then(|s| s.uptime_secs)
                .map(format_duration)
                .unwrap_or_default();
            let enabled = if p.enabled { "enabled" } else { "disabled" };
            let provider = p.config.provider.as_deref().unwrap_or("-");
            let model = p.config.model.as_deref().unwrap_or("-");
            let channels: Vec<&str> = p.config.channels.iter().map(channel_type_name).collect();

            lines.push(format!(
                "- **{}** [{state}] {pid} {uptime} ({enabled}) provider={provider} model={model} channels=[{}]",
                p.name,
                channels.join(", ")
            ));
        }

        if lines.is_empty() {
            Ok(ToolResult {
                output: "No profiles match the filter.".into(),
                success: true,
                ..Default::default()
            })
        } else {
            Ok(ToolResult {
                output: format!("{} profiles:\n{}", lines.len(), lines.join("\n")),
                success: true,
                ..Default::default()
            })
        }
    }
}

// ── admin_profile_status ───────────────────────────────────────────────

pub struct ProfileStatusTool {
    ctx: Arc<AdminContext>,
}

impl ProfileStatusTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[derive(Deserialize)]
struct ProfileIdInput {
    profile_id: String,
}

#[async_trait]
impl Tool for ProfileStatusTool {
    fn name(&self) -> &str {
        "admin_profile_status"
    }
    fn description(&self) -> &str {
        "Get detailed status for a single profile: PID, uptime, channels, provider, model, enabled."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "profile_id": { "type": "string", "description": "Profile ID" }
            },
            "required": ["profile_id"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: ProfileIdInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;

        let profile = match self.ctx.profile_store.get(&input.profile_id)? {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    output: format!("Profile '{}' not found.", input.profile_id),
                    success: false,
                    ..Default::default()
                });
            }
        };

        let status = self.ctx.process_manager.status(&profile.id).await;
        let channels: Vec<&str> = profile
            .config
            .channels
            .iter()
            .map(channel_type_name)
            .collect();

        let mut out = format!("Profile: {} ({})\n", profile.name, profile.id);
        out.push_str(&format!("  Enabled: {}\n", profile.enabled));
        out.push_str(&format!("  Running: {}\n", status.running));
        if let Some(pid) = status.pid {
            out.push_str(&format!("  PID: {pid}\n"));
        }
        if let Some(uptime) = status.uptime_secs {
            out.push_str(&format!("  Uptime: {}\n", format_duration(uptime)));
        }
        out.push_str(&format!(
            "  Provider: {}\n",
            profile.config.provider.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!(
            "  Model: {}\n",
            profile.config.model.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!("  Channels: [{}]\n", channels.join(", ")));
        if let Some(ref parent) = profile.parent_id {
            out.push_str(&format!("  Parent: {parent}\n"));
        }

        Ok(ToolResult {
            output: out,
            success: true,
            ..Default::default()
        })
    }
}

// ── admin_start_profile ────────────────────────────────────────────────

pub struct StartProfileTool {
    ctx: Arc<AdminContext>,
}

impl StartProfileTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for StartProfileTool {
    fn name(&self) -> &str {
        "admin_start_profile"
    }
    fn description(&self) -> &str {
        "Start a gateway for the given profile."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "profile_id": { "type": "string", "description": "Profile ID to start" }
            },
            "required": ["profile_id"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: ProfileIdInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;

        let profile = match self.ctx.profile_store.get(&input.profile_id)? {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    output: format!("Profile '{}' not found.", input.profile_id),
                    success: false,
                    ..Default::default()
                });
            }
        };

        match self.ctx.process_manager.start(&profile).await {
            Ok(()) => Ok(ToolResult {
                output: format!("Gateway '{}' started.", profile.name),
                success: true,
                ..Default::default()
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to start '{}': {e}", profile.name),
                success: false,
                ..Default::default()
            }),
        }
    }
}

// ── admin_stop_profile ─────────────────────────────────────────────────

pub struct StopProfileTool {
    ctx: Arc<AdminContext>,
}

impl StopProfileTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for StopProfileTool {
    fn name(&self) -> &str {
        "admin_stop_profile"
    }
    fn description(&self) -> &str {
        "Stop a running gateway for the given profile."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "profile_id": { "type": "string", "description": "Profile ID to stop" }
            },
            "required": ["profile_id"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: ProfileIdInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;

        match self.ctx.process_manager.stop(&input.profile_id).await {
            Ok(true) => Ok(ToolResult {
                output: format!("Gateway '{}' stopped.", input.profile_id),
                success: true,
                ..Default::default()
            }),
            Ok(false) => Ok(ToolResult {
                output: format!("Gateway '{}' was not running.", input.profile_id),
                success: true,
                ..Default::default()
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to stop '{}': {e}", input.profile_id),
                success: false,
                ..Default::default()
            }),
        }
    }
}

// ── admin_restart_profile ──────────────────────────────────────────────

pub struct RestartProfileTool {
    ctx: Arc<AdminContext>,
}

impl RestartProfileTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for RestartProfileTool {
    fn name(&self) -> &str {
        "admin_restart_profile"
    }
    fn description(&self) -> &str {
        "Restart a gateway (stop then start)."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "profile_id": { "type": "string", "description": "Profile ID to restart" }
            },
            "required": ["profile_id"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: ProfileIdInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;

        let profile = match self.ctx.profile_store.get(&input.profile_id)? {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    output: format!("Profile '{}' not found.", input.profile_id),
                    success: false,
                    ..Default::default()
                });
            }
        };

        match self.ctx.process_manager.restart(&profile).await {
            Ok(()) => Ok(ToolResult {
                output: format!("Gateway '{}' restarted.", profile.name),
                success: true,
                ..Default::default()
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to restart '{}': {e}", profile.name),
                success: false,
                ..Default::default()
            }),
        }
    }
}

// ── admin_enable_profile ───────────────────────────────────────────────

pub struct EnableProfileTool {
    ctx: Arc<AdminContext>,
}

impl EnableProfileTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[derive(Deserialize)]
struct EnableInput {
    profile_id: String,
    enabled: bool,
}

#[async_trait]
impl Tool for EnableProfileTool {
    fn name(&self) -> &str {
        "admin_enable_profile"
    }
    fn description(&self) -> &str {
        "Enable or disable auto-start for a profile."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "profile_id": { "type": "string", "description": "Profile ID" },
                "enabled": { "type": "boolean", "description": "true to enable auto-start, false to disable" }
            },
            "required": ["profile_id", "enabled"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: EnableInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;

        let mut profile = match self.ctx.profile_store.get(&input.profile_id)? {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    output: format!("Profile '{}' not found.", input.profile_id),
                    success: false,
                    ..Default::default()
                });
            }
        };

        profile.enabled = input.enabled;
        profile.updated_at = Utc::now();
        self.ctx.profile_store.save(&profile)?;

        let state = if input.enabled { "enabled" } else { "disabled" };
        Ok(ToolResult {
            output: format!("Profile '{}' auto-start {}.", profile.name, state),
            success: true,
            ..Default::default()
        })
    }
}

// ── admin_view_logs ────────────────────────────────────────────────────

pub struct ViewLogsTool {
    ctx: Arc<AdminContext>,
}

impl ViewLogsTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[derive(Deserialize)]
struct ViewLogsInput {
    profile_id: String,
    #[serde(default = "default_log_lines")]
    lines: usize,
}
fn default_log_lines() -> usize {
    30
}

#[async_trait]
impl Tool for ViewLogsTool {
    fn name(&self) -> &str {
        "admin_view_logs"
    }
    fn description(&self) -> &str {
        "View recent log output from a running gateway. Returns up to N lines (default 30, max 100)."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "profile_id": { "type": "string", "description": "Profile ID" },
                "lines": { "type": "integer", "description": "Number of log lines to collect (default 30, max 100)" }
            },
            "required": ["profile_id"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: ViewLogsInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;
        let max_lines = input.lines.min(100);

        let mut rx = match self
            .ctx
            .process_manager
            .subscribe_logs(&input.profile_id)
            .await
        {
            Some(rx) => rx,
            None => {
                return Ok(ToolResult {
                    output: format!("Gateway '{}' is not running.", input.profile_id),
                    success: false,
                    ..Default::default()
                });
            }
        };

        let mut collected = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if collected.len() >= max_lines {
                break;
            }
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(line) => collected.push(line),
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }

        if collected.is_empty() {
            Ok(ToolResult {
                output: format!(
                    "No log output from '{}' in the last 3 seconds.",
                    input.profile_id
                ),
                success: true,
                ..Default::default()
            })
        } else {
            Ok(ToolResult {
                output: format!(
                    "{} log lines from '{}':\n{}",
                    collected.len(),
                    input.profile_id,
                    collected.join("\n")
                ),
                success: true,
                ..Default::default()
            })
        }
    }
}

// ── admin_system_health ────────────────────────────────────────────────

pub struct SystemHealthTool {
    ctx: Arc<AdminContext>,
}

impl SystemHealthTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for SystemHealthTool {
    fn name(&self) -> &str {
        "admin_system_health"
    }
    fn description(&self) -> &str {
        "Get system-wide health: total profiles, running/stopped counts, server uptime, watchdog/alerts status."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn execute(&self, _args: &serde_json::Value) -> Result<ToolResult> {
        let profiles = self.ctx.profile_store.list().unwrap_or_default();
        let statuses = self.ctx.process_manager.all_statuses().await;

        let total = profiles.len();
        let enabled = profiles.iter().filter(|p| p.enabled).count();
        let running = statuses.len();
        let stopped_enabled = profiles
            .iter()
            .filter(|p| p.enabled && !statuses.contains_key(&p.id))
            .count();

        let uptime = Utc::now() - self.ctx.server_started_at;
        let watchdog = if self.ctx.watchdog_enabled.load(Ordering::Relaxed) {
            "ON"
        } else {
            "OFF"
        };
        let alerts = if self.ctx.alerts_enabled.load(Ordering::Relaxed) {
            "ON"
        } else {
            "OFF"
        };

        let mut out = String::from("System Health:\n");
        out.push_str(&format!(
            "  Server uptime: {}\n",
            format_duration(uptime.num_seconds())
        ));
        out.push_str(&format!("  Profiles: {total} total, {enabled} enabled\n"));
        out.push_str(&format!(
            "  Running: {running}, Stopped (enabled): {stopped_enabled}\n"
        ));
        out.push_str(&format!("  Watchdog: {watchdog}, Alerts: {alerts}\n"));

        if stopped_enabled > 0 {
            out.push_str("\n  WARNING: Some enabled profiles are not running!\n");
            for p in &profiles {
                if p.enabled && !statuses.contains_key(&p.id) {
                    out.push_str(&format!("    - {} ({})\n", p.name, p.id));
                }
            }
        }

        Ok(ToolResult {
            output: out,
            success: true,
            ..Default::default()
        })
    }
}

// ── admin_provider_metrics ─────────────────────────────────────────────

pub struct ProviderMetricsTool {
    ctx: Arc<AdminContext>,
}

impl ProviderMetricsTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for ProviderMetricsTool {
    fn name(&self) -> &str {
        "admin_provider_metrics"
    }
    fn description(&self) -> &str {
        "Read provider QoS metrics (latency, error rate, token usage) for a profile."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "profile_id": { "type": "string", "description": "Profile ID" }
            },
            "required": ["profile_id"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: ProfileIdInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;

        match self
            .ctx
            .process_manager
            .read_metrics(&input.profile_id)
            .await
        {
            Some(metrics) => Ok(ToolResult {
                output: serde_json::to_string_pretty(&metrics).unwrap_or_else(|_| "{}".into()),
                success: true,
                ..Default::default()
            }),
            None => Ok(ToolResult {
                output: format!("No metrics available for '{}'.", input.profile_id),
                success: true,
                ..Default::default()
            }),
        }
    }
}

// ── admin_manage_watchdog ──────────────────────────────────────────────

pub struct ManageWatchdogTool {
    ctx: Arc<AdminContext>,
}

impl ManageWatchdogTool {
    pub fn new(ctx: Arc<AdminContext>) -> Self {
        Self { ctx }
    }
}

#[derive(Deserialize)]
struct WatchdogInput {
    action: String,
}

#[async_trait]
impl Tool for ManageWatchdogTool {
    fn name(&self) -> &str {
        "admin_manage_watchdog"
    }
    fn description(&self) -> &str {
        "Check or toggle watchdog auto-restart and proactive alerts. Actions: status, enable, disable."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "enable", "disable"],
                    "description": "Action to perform"
                }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: WatchdogInput =
            serde_json::from_value(args.clone()).map_err(|e| eyre::eyre!("invalid input: {e}"))?;

        match input.action.as_str() {
            "status" => {
                let wd = self.ctx.watchdog_enabled.load(Ordering::Relaxed);
                let al = self.ctx.alerts_enabled.load(Ordering::Relaxed);
                Ok(ToolResult {
                    output: format!(
                        "Watchdog: {}, Alerts: {}",
                        if wd { "ON" } else { "OFF" },
                        if al { "ON" } else { "OFF" }
                    ),
                    success: true,
                    ..Default::default()
                })
            }
            "enable" => {
                self.ctx.watchdog_enabled.store(true, Ordering::Relaxed);
                self.ctx.alerts_enabled.store(true, Ordering::Relaxed);
                Ok(ToolResult {
                    output: "Watchdog and alerts enabled.".into(),
                    success: true,
                    ..Default::default()
                })
            }
            "disable" => {
                self.ctx.watchdog_enabled.store(false, Ordering::Relaxed);
                self.ctx.alerts_enabled.store(false, Ordering::Relaxed);
                Ok(ToolResult {
                    output: "Watchdog and alerts disabled.".into(),
                    success: true,
                    ..Default::default()
                })
            }
            other => Ok(ToolResult {
                output: format!("Unknown action '{other}'. Use: status, enable, disable."),
                success: false,
                ..Default::default()
            }),
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

use crate::profiles::ChannelCredentials;

fn channel_type_name(ch: &ChannelCredentials) -> &'static str {
    match ch {
        ChannelCredentials::Telegram { .. } => "telegram",
        ChannelCredentials::Discord { .. } => "discord",
        ChannelCredentials::Slack { .. } => "slack",
        ChannelCredentials::WhatsApp { .. } => "whatsapp",
        ChannelCredentials::Feishu { .. } => "feishu",
        ChannelCredentials::Email { .. } => "email",
    }
}

fn format_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

/// Register all admin tools into a ToolRegistry.
pub fn register_admin_tools(registry: &mut crew_agent::ToolRegistry, ctx: Arc<AdminContext>) {
    registry.register(ListProfilesTool::new(ctx.clone()));
    registry.register(ProfileStatusTool::new(ctx.clone()));
    registry.register(StartProfileTool::new(ctx.clone()));
    registry.register(StopProfileTool::new(ctx.clone()));
    registry.register(RestartProfileTool::new(ctx.clone()));
    registry.register(EnableProfileTool::new(ctx.clone()));
    registry.register(ViewLogsTool::new(ctx.clone()));
    registry.register(SystemHealthTool::new(ctx.clone()));
    registry.register(ProviderMetricsTool::new(ctx.clone()));
    registry.register(ManageWatchdogTool::new(ctx));
}
