//! LLM provider trait.

use async_trait::async_trait;
use crew_core::Message;
use eyre::Result;

use crate::config::ChatConfig;
use crate::context;
use crate::types::{ChatResponse, ChatStream, StreamEvent, ToolSpec};

/// Trait for LLM providers.
///
/// This is intentionally minimal to reduce abstraction overhead.
/// Each provider implements the specifics of its API.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request.
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatResponse>;

    /// Stream a chat completion response.
    /// Default: falls back to non-streaming chat() and emits events.
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatStream> {
        let response = self.chat(messages, tools, config).await?;
        let mut events: Vec<StreamEvent> = Vec::new();
        if let Some(text) = response.content.clone() {
            events.push(StreamEvent::TextDelta(text));
        }
        for (i, tc) in response.tool_calls.iter().enumerate() {
            events.push(StreamEvent::ToolCallDelta {
                index: i,
                id: Some(tc.id.clone()),
                name: Some(tc.name.clone()),
                arguments_delta: tc.arguments.to_string(),
            });
        }
        events.push(StreamEvent::Usage(response.usage));
        events.push(StreamEvent::Done(response.stop_reason));
        Ok(Box::pin(futures::stream::iter(events)))
    }

    /// Get the context window size in tokens for this model.
    fn context_window(&self) -> u32 {
        context::context_window_tokens(self.model_id())
    }

    /// Get the model identifier.
    fn model_id(&self) -> &str;

    /// Get the provider name (e.g., "anthropic", "openai").
    fn provider_name(&self) -> &str;

    /// Export provider QoS metrics as JSON (for adaptive routers).
    /// Returns `None` for simple providers; overridden by `AdaptiveRouter`.
    fn export_metrics(&self) -> Option<serde_json::Value> {
        None
    }
}

/// Truncate an API error body to avoid leaking verbose internal details.
/// Keeps the first 200 chars which typically contain the error message/code.
pub(crate) fn truncate_error_body(body: &str) -> String {
    if body.len() <= 200 {
        body.to_string()
    } else {
        format!("{}... ({} bytes total)", &body[..200], body.len())
    }
}

/// Default LLM request timeout in seconds.
pub const DEFAULT_LLM_TIMEOUT_SECS: u64 = 120;
/// Default LLM connect timeout in seconds.
pub const DEFAULT_LLM_CONNECT_TIMEOUT_SECS: u64 = 30;
/// Default embedding request timeout in seconds.
pub const DEFAULT_EMBEDDING_TIMEOUT_SECS: u64 = 60;
/// Default embedding connect timeout in seconds.
pub const DEFAULT_EMBEDDING_CONNECT_TIMEOUT_SECS: u64 = 15;

/// Build a `reqwest::Client` with the given timeout values (in seconds).
pub fn build_http_client(timeout_secs: u64, connect_timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(connect_timeout_secs))
        .build()
        .expect("failed to build HTTP client")
}
