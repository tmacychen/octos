//! API router construction.

use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::routing::{get, post};
use tower_http::cors::{Any, CorsLayer};

use super::AppState;
use super::handlers;

/// Build the axum router with all API routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
        .route("/api/chat", post(handlers::chat))
        .route("/api/chat/stream", get(handlers::chat_stream))
        .route("/api/sessions", get(handlers::list_sessions))
        .route("/api/sessions/{id}/messages", get(handlers::session_messages))
        .route("/api/status", get(handlers::status));

    // Wrap with auth middleware if token is configured
    let api = if state.auth_token.is_some() {
        api.layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
    } else {
        api
    };

    api.layer(cors).with_state(state)
}

/// Simple bearer token auth middleware.
async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    if let Some(expected) = &state.auth_token {
        let auth_header = req
            .headers()
            .get("authorization")
            .and_then(|v: &HeaderValue| v.to_str().ok())
            .unwrap_or("");

        let token = auth_header.strip_prefix("Bearer ").unwrap_or("");
        if token != expected {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(req).await)
}
