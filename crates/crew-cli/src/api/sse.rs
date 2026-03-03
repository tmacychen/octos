//! SSE broadcaster for streaming progress events.

use crew_agent::{ProgressEvent, ProgressReporter};
use tokio::sync::broadcast;
use tracing::debug;

/// Broadcasts progress events to SSE subscribers.
pub struct SseBroadcaster {
    tx: broadcast::Sender<String>,
}

impl SseBroadcaster {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}

impl ProgressReporter for SseBroadcaster {
    fn report(&self, event: ProgressEvent) {
        let json = match serde_json::to_string(&event_to_json(&event)) {
            Ok(j) => j,
            Err(_) => return,
        };
        // Ignore send errors (no subscribers)
        let _ = self.tx.send(json);
    }
}

fn event_to_json(event: &ProgressEvent) -> serde_json::Value {
    match event {
        ProgressEvent::ToolStarted { name, .. } => {
            serde_json::json!({"type": "tool_start", "tool": name})
        }
        ProgressEvent::ToolCompleted { name, success, .. } => {
            serde_json::json!({"type": "tool_end", "tool": name, "success": success})
        }
        ProgressEvent::StreamChunk { text, .. } => {
            serde_json::json!({"type": "token", "text": text})
        }
        ProgressEvent::StreamDone { .. } => {
            serde_json::json!({"type": "stream_end"})
        }
        ProgressEvent::CostUpdate {
            session_input_tokens,
            session_output_tokens,
            session_cost,
            ..
        } => {
            serde_json::json!({
                "type": "cost_update",
                "input_tokens": session_input_tokens,
                "output_tokens": session_output_tokens,
                "session_cost": session_cost,
            })
        }
        ProgressEvent::Thinking { iteration } => {
            serde_json::json!({"type": "thinking", "iteration": iteration})
        }
        ProgressEvent::Response { iteration, .. } => {
            serde_json::json!({"type": "response", "iteration": iteration})
        }
        other => {
            debug!("unmapped SSE progress event: {other:?}");
            serde_json::json!({"type": "other"})
        }
    }
}
