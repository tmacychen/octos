//! API router construction.

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::routing::{delete, get, post, put};
use tower_http::cors::CorsLayer;

use super::AppState;
use super::admin;
use super::handlers;
use super::metrics;
use super::static_files;

/// Build the axum router with all API routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    // Restrict CORS to localhost origins (override in reverse proxy for production)
    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost:3000".parse::<HeaderValue>().unwrap(),
            "http://localhost:5173".parse::<HeaderValue>().unwrap(),
            "http://localhost:8080".parse::<HeaderValue>().unwrap(),
            "http://127.0.0.1:3000".parse::<HeaderValue>().unwrap(),
            "http://127.0.0.1:5173".parse::<HeaderValue>().unwrap(),
            "http://127.0.0.1:8080".parse::<HeaderValue>().unwrap(),
        ])
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    // TODO: add rate limiting middleware for production deployments

    let api = Router::new()
        .route("/api/chat", post(handlers::chat))
        .route("/api/chat/stream", get(handlers::chat_stream))
        .route("/api/sessions", get(handlers::list_sessions))
        .route(
            "/api/sessions/{id}/messages",
            get(handlers::session_messages),
        )
        .route("/api/status", get(handlers::status));

    // Admin API routes (1MB body limit)
    let admin_api = Router::new()
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .route("/api/admin/overview", get(admin::overview))
        .route("/api/admin/profiles", get(admin::list_profiles))
        .route("/api/admin/profiles", post(admin::create_profile))
        .route("/api/admin/profiles/{id}", get(admin::get_profile))
        .route("/api/admin/profiles/{id}", put(admin::update_profile))
        .route("/api/admin/profiles/{id}", delete(admin::delete_profile))
        .route("/api/admin/profiles/{id}/start", post(admin::start_gateway))
        .route("/api/admin/profiles/{id}/stop", post(admin::stop_gateway))
        .route(
            "/api/admin/profiles/{id}/restart",
            post(admin::restart_gateway),
        )
        .route(
            "/api/admin/profiles/{id}/status",
            get(admin::gateway_status),
        )
        .route("/api/admin/profiles/{id}/logs", get(admin::gateway_logs))
        .route("/api/admin/start-all", post(admin::start_all))
        .route("/api/admin/stop-all", post(admin::stop_all));

    // Wrap with auth middleware if token is configured
    let api = if state.auth_token.is_some() {
        api.merge(admin_api).layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
    } else {
        api.merge(admin_api)
    };

    // Unauthenticated routes (metrics + static files)
    let public = Router::new().route("/metrics", get(metrics::metrics_handler));

    public
        .merge(api)
        .fallback(static_files::static_handler)
        .layer(cors)
        .with_state(state)
}

/// Constant-time byte comparison to prevent timing attacks on auth tokens.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
    }

    #[test]
    fn test_constant_time_eq_not_equal() {
        assert!(!constant_time_eq(b"secret-token", b"wrong-token!"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer-string"));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_constant_time_eq_single_bit_diff() {
        assert!(!constant_time_eq(b"\x00", b"\x01"));
    }
}

/// Simple bearer token auth middleware.
/// Accepts token via `Authorization: Bearer <token>` header or `?token=<token>` query param.
/// The query param fallback is needed for SSE (EventSource) which cannot send custom headers.
async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    if let Some(expected) = &state.auth_token {
        // Try Authorization header first
        let header_token = req
            .headers()
            .get("authorization")
            .and_then(|v: &HeaderValue| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .unwrap_or("");

        // Fall back to ?token= query param (for SSE / EventSource)
        let query_token = req
            .uri()
            .query()
            .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token=")))
            .unwrap_or("");

        let token = if !header_token.is_empty() {
            header_token
        } else {
            query_token
        };

        if !constant_time_eq(token.as_bytes(), expected.as_bytes()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(req).await)
}
