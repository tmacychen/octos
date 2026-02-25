//! REST API and SSE streaming for crew-rs.
//!
//! Feature-gated behind `api`. Start with `crew serve [--port 8080]`.

pub mod admin;
mod handlers;
pub mod metrics;
mod router;
mod sse;
mod static_files;

pub use metrics::init_metrics;
pub use router::build_router;
pub use sse::SseBroadcaster;

use std::sync::Arc;

use crate::process_manager::ProcessManager;
use crate::profiles::ProfileStore;

/// Shared application state for API handlers.
pub struct AppState {
    /// Agent for processing messages (None if no LLM provider configured).
    pub agent: Option<Arc<crew_agent::Agent>>,
    /// Session manager for history.
    pub sessions: Option<Arc<tokio::sync::Mutex<crew_bus::SessionManager>>>,
    /// SSE broadcaster for streaming events.
    pub broadcaster: Arc<SseBroadcaster>,
    /// Server start time.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Auth token (if configured).
    pub auth_token: Option<String>,
    /// Prometheus metrics handle.
    pub metrics_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
    /// Profile store for admin dashboard.
    pub profile_store: Option<Arc<ProfileStore>>,
    /// Process manager for gateway lifecycle.
    pub process_manager: Option<Arc<ProcessManager>>,
}
