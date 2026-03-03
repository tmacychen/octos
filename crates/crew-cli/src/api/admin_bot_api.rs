//! Admin bot config read/write API handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::AppState;

/// Admin bot config payload (mirrors `AdminBotConfig` fields).
#[derive(Debug, Serialize, Deserialize)]
pub struct AdminBotConfigPayload {
    #[serde(default)]
    pub telegram_token_env: Option<String>,
    #[serde(default)]
    pub feishu_app_id_env: Option<String>,
    #[serde(default)]
    pub feishu_app_secret_env: Option<String>,
    #[serde(default)]
    pub admin_chat_ids: Vec<i64>,
    #[serde(default)]
    pub admin_feishu_ids: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_true")]
    pub alerts_enabled: bool,
    #[serde(default = "default_true")]
    pub watchdog_enabled: bool,
    #[serde(default = "default_health_interval")]
    pub health_check_interval_secs: u64,
    #[serde(default = "default_max_restart")]
    pub max_restart_attempts: u32,
}

fn default_true() -> bool {
    true
}
fn default_health_interval() -> u64 {
    60
}
fn default_max_restart() -> u32 {
    3
}

impl Default for AdminBotConfigPayload {
    fn default() -> Self {
        Self {
            telegram_token_env: None,
            feishu_app_id_env: None,
            feishu_app_secret_env: None,
            admin_chat_ids: Vec::new(),
            admin_feishu_ids: Vec::new(),
            provider: None,
            model: None,
            base_url: None,
            api_key_env: None,
            alerts_enabled: true,
            watchdog_enabled: true,
            health_check_interval_secs: 60,
            max_restart_attempts: 3,
        }
    }
}

/// GET /api/admin/admin-bot — read current admin bot config.
pub async fn get_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AdminBotConfigPayload>, StatusCode> {
    let config_path = state.config_path.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    let content = std::fs::read_to_string(config_path).map_err(|e| {
        tracing::warn!(error = %e, "failed to read config file");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let root: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        tracing::warn!(error = %e, "failed to parse config JSON");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let payload = if let Some(ab) = root.get("admin_bot") {
        serde_json::from_value::<AdminBotConfigPayload>(ab.clone()).unwrap_or_default()
    } else {
        AdminBotConfigPayload::default()
    };

    Ok(Json(payload))
}

/// PUT /api/admin/admin-bot — update admin bot config.
pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AdminBotConfigPayload>,
) -> Result<Json<AdminBotConfigPayload>, StatusCode> {
    let config_path = state.config_path.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    // Read current config as raw JSON value
    let content = std::fs::read_to_string(config_path).map_err(|e| {
        tracing::warn!(error = %e, "failed to read config file");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut root: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        tracing::warn!(error = %e, "failed to parse config JSON");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Merge payload into admin_bot key
    let admin_bot_value = serde_json::to_value(&payload).map_err(|e| {
        tracing::warn!(error = %e, "failed to serialize admin bot config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    root.as_object_mut()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .insert("admin_bot".to_string(), admin_bot_value);

    // Atomic write: temp file + rename
    let tmp_path = config_path.with_extension("json.tmp");
    let serialized = serde_json::to_string_pretty(&root).map_err(|e| {
        tracing::warn!(error = %e, "failed to serialize config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    std::fs::write(&tmp_path, &serialized).map_err(|e| {
        tracing::warn!(error = %e, "failed to write temp config file");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    std::fs::rename(&tmp_path, config_path).map_err(|e| {
        tracing::warn!(error = %e, "failed to rename temp config file");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!("admin bot config updated");

    Ok(Json(payload))
}
