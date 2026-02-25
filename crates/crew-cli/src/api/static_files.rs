//! Embedded static file serving for the built-in Web UI.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

use super::AppState;

#[derive(Embed)]
#[folder = "static/"]
struct Assets;

/// Fallback handler: serves embedded static files, falls back to index.html for SPA routing.
/// Routes under `/admin/` fall back to `admin/index.html` for the admin dashboard SPA.
pub async fn static_handler(State(_state): State<Arc<AppState>>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // Check if this is an admin route
    let is_admin = path.starts_with("admin");

    match Assets::get(path) {
        Some(file) => serve_file(path, &file.data),
        None => {
            // SPA fallback: serve the appropriate index.html
            let fallback = if is_admin {
                "admin/index.html"
            } else {
                "index.html"
            };
            match Assets::get(fallback) {
                Some(file) => serve_file(fallback, &file.data),
                None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
            }
        }
    }
}

fn serve_file(path: &str, data: &[u8]) -> Response {
    let mime = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime)],
        data.to_vec(),
    )
        .into_response()
}
