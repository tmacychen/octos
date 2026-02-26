//! Admin API handlers for profile and gateway management.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::profiles::{ProfileConfig, UserProfile, mask_secrets};

// ── Request / Response types ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateProfileRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub data_dir: Option<String>,
    #[serde(default)]
    pub config: ProfileConfig,
}

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub data_dir: Option<Option<String>>,
    #[serde(default)]
    pub config: Option<ProfileConfig>,
}

#[derive(Serialize)]
pub struct ProfileResponse {
    #[serde(flatten)]
    pub profile: UserProfile,
    pub status: crate::process_manager::ProcessStatus,
}

#[derive(Serialize)]
pub struct OverviewResponse {
    pub total_profiles: usize,
    pub running: usize,
    pub stopped: usize,
    pub profiles: Vec<ProfileResponse>,
}

#[derive(Serialize)]
pub struct ActionResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Serialize)]
pub struct BulkActionResponse {
    pub ok: bool,
    pub count: usize,
}

// ── Handlers ──────────────────────────────────────────────────────────

/// GET /api/admin/overview
pub async fn overview(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OverviewResponse>, (StatusCode, String)> {
    let profiles = state
        .profile_store
        .as_ref()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "admin not configured".into(),
        ))?
        .list()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let mut running = 0;
    let mut items = Vec::with_capacity(profiles.len());
    for p in profiles {
        let status = pm.status(&p.id).await;
        if status.running {
            running += 1;
        }
        items.push(ProfileResponse {
            profile: mask_secrets(&p),
            status,
        });
    }

    let total = items.len();
    Ok(Json(OverviewResponse {
        total_profiles: total,
        running,
        stopped: total - running,
        profiles: items,
    }))
}

/// GET /api/admin/profiles
pub async fn list_profiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ProfileResponse>>, (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let profiles = store
        .list()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut items = Vec::with_capacity(profiles.len());
    for p in profiles {
        let status = pm.status(&p.id).await;
        items.push(ProfileResponse {
            profile: mask_secrets(&p),
            status,
        });
    }
    Ok(Json(items))
}

/// GET /api/admin/profiles/:id
pub async fn get_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ProfileResponse>, (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let profile = store
        .get(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("profile '{id}' not found")))?;

    let status = pm.status(&id).await;
    Ok(Json(ProfileResponse {
        profile: mask_secrets(&profile),
        status,
    }))
}

/// POST /api/admin/profiles
pub async fn create_profile(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateProfileRequest>,
) -> Result<(StatusCode, Json<ProfileResponse>), (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    // Check for duplicates
    if store
        .get(&req.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_some()
    {
        return Err((
            StatusCode::CONFLICT,
            format!("profile '{}' already exists", req.id),
        ));
    }

    let now = Utc::now();
    let profile = UserProfile {
        id: req.id,
        name: req.name,
        enabled: req.enabled,
        data_dir: req.data_dir,
        config: req.config,
        created_at: now,
        updated_at: now,
    };

    store
        .save(&profile)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let status = pm.status(&profile.id).await;
    Ok((
        StatusCode::CREATED,
        Json(ProfileResponse {
            profile: mask_secrets(&profile),
            status,
        }),
    ))
}

/// PUT /api/admin/profiles/:id
pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<Json<ProfileResponse>, (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let mut profile = store
        .get(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("profile '{id}' not found")))?;

    if let Some(name) = req.name {
        profile.name = name;
    }
    if let Some(enabled) = req.enabled {
        profile.enabled = enabled;
    }
    if let Some(data_dir) = req.data_dir {
        profile.data_dir = data_dir;
    }
    if let Some(mut new_config) = req.config {
        // Merge env_vars: preserve existing secrets when the incoming value
        // is masked (contains "***") or empty. This prevents the masked
        // values returned by GET from overwriting the real secrets.
        let old_env_vars = std::mem::take(&mut profile.config.env_vars);
        for (key, old_val) in &old_env_vars {
            match new_config.env_vars.get(key) {
                // Masked or empty value sent back — keep the original secret
                Some(v) if v.contains("***") || v.is_empty() => {
                    new_config.env_vars.insert(key.clone(), old_val.clone());
                }
                // New value provided — use it
                Some(_) => {}
                // Key removed in update — don't re-add
                None => {}
            }
        }
        profile.config = new_config;
    }
    profile.updated_at = Utc::now();

    store
        .save(&profile)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let status = pm.status(&id).await;
    Ok(Json(ProfileResponse {
        profile: mask_secrets(&profile),
        status,
    }))
}

/// DELETE /api/admin/profiles/:id
pub async fn delete_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    // Stop the gateway if running
    let _ = pm.stop(&id).await;

    let deleted = store
        .delete(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, format!("profile '{id}' not found")));
    }

    Ok(Json(ActionResponse {
        ok: true,
        message: Some(format!("profile '{id}' deleted")),
    }))
}

/// POST /api/admin/profiles/:id/start
pub async fn start_gateway(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let profile = store
        .get(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("profile '{id}' not found")))?;

    pm.start(&profile)
        .await
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;

    Ok(Json(ActionResponse {
        ok: true,
        message: Some(format!("gateway '{id}' started")),
    }))
}

/// POST /api/admin/profiles/:id/stop
pub async fn stop_gateway(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let stopped = pm
        .stop(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !stopped {
        return Err((
            StatusCode::NOT_FOUND,
            format!("gateway '{id}' is not running"),
        ));
    }

    Ok(Json(ActionResponse {
        ok: true,
        message: Some(format!("gateway '{id}' stopped")),
    }))
}

/// POST /api/admin/profiles/:id/restart
pub async fn restart_gateway(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let profile = store
        .get(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("profile '{id}' not found")))?;

    pm.restart(&profile)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ActionResponse {
        ok: true,
        message: Some(format!("gateway '{id}' restarted")),
    }))
}

/// GET /api/admin/profiles/:id/status
pub async fn gateway_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<crate::process_manager::ProcessStatus>, (StatusCode, String)> {
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    Ok(Json(pm.status(&id).await))
}

/// GET /api/admin/profiles/:id/logs — SSE log stream.
pub async fn gateway_logs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>,
    (StatusCode, String),
> {
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let rx = pm.subscribe_logs(&id).await.ok_or((
        StatusCode::NOT_FOUND,
        format!("gateway '{id}' is not running"),
    ))?;

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    let event: Result<Event, std::convert::Infallible> =
                        Ok(Event::default().data(line));
                    return Some((event, rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// POST /api/admin/start-all
pub async fn start_all(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BulkActionResponse>, (StatusCode, String)> {
    let store = state.profile_store.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let profiles = store
        .list()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut started = 0;
    for p in &profiles {
        if p.enabled {
            if pm.start(p).await.is_ok() {
                started += 1;
            }
        }
    }

    Ok(Json(BulkActionResponse {
        ok: true,
        count: started,
    }))
}

/// POST /api/admin/stop-all
pub async fn stop_all(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BulkActionResponse>, (StatusCode, String)> {
    let pm = state.process_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "admin not configured".into(),
    ))?;

    let count = pm.stop_all().await;
    Ok(Json(BulkActionResponse { ok: true, count }))
}
