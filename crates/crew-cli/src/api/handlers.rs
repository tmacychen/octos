//! API request handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use crew_core::{Message, MessageRole, SessionKey};
use serde::{Deserialize, Serialize};

use super::AppState;

/// POST /api/chat -- send a message, get a response.
#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub content: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Maximum message length (1MB).
const MAX_MESSAGE_LEN: usize = 1_048_576;

pub async fn chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, String)> {
    if req.message.len() > MAX_MESSAGE_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("message exceeds {}KB limit", MAX_MESSAGE_LEN / 1024),
        ));
    }

    let session_key = SessionKey::new(
        "api",
        req.session_id.as_deref().unwrap_or("default"),
    );

    let history: Vec<Message> = {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.get_or_create(&session_key);
        session.get_history(50).to_vec()
    };

    let response = state
        .agent
        .process_message(&req.message, &history, vec![])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Save to session
    {
        let mut sessions = state.sessions.lock().await;
        let user_msg = Message {
            role: MessageRole::User,
            content: req.message,
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            timestamp: chrono::Utc::now(),
        };
        let _ = sessions.add_message(&session_key, user_msg);
        let assistant_msg = Message {
            role: MessageRole::Assistant,
            content: response.content.clone(),
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            timestamp: chrono::Utc::now(),
        };
        let _ = sessions.add_message(&session_key, assistant_msg);
    }

    Ok(Json(ChatResponse {
        content: response.content,
        input_tokens: response.token_usage.input_tokens,
        output_tokens: response.token_usage.output_tokens,
    }))
}

/// GET /api/chat/stream -- SSE stream of progress events.
pub async fn chat_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.broadcaster.subscribe();

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(data) => {
                    let event: Result<Event, std::convert::Infallible> =
                        Ok(Event::default().data(data));
                    return Some((event, rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// GET /api/sessions -- list sessions.
#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub message_count: usize,
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<SessionInfo>> {
    let sessions = state.sessions.lock().await;
    let list = sessions
        .list_sessions()
        .into_iter()
        .map(|(id, count)| SessionInfo {
            id,
            message_count: count,
        })
        .collect();
    Json(list)
}

/// GET /api/sessions/:id/messages -- get session history.
///
/// Query params: `?limit=100&offset=0`
#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_page_limit() -> usize {
    100
}

pub async fn session_messages(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Json<Vec<MessageInfo>> {
    let limit = params.limit.min(500);
    let fetch_count = params.offset.saturating_add(limit);
    let key = SessionKey::new("api", &id);
    let mut sessions = state.sessions.lock().await;
    let session = sessions.get_or_create(&key);
    let messages = session
        .get_history(fetch_count)
        .iter()
        .skip(params.offset)
        .take(limit)
        .map(|m| MessageInfo {
            role: format!("{:?}", m.role),
            content: m.content.clone(),
            timestamp: m.timestamp.to_rfc3339(),
        })
        .collect();
    Json(messages)
}

#[derive(Serialize)]
pub struct MessageInfo {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// GET /api/status -- server status.
#[derive(Serialize)]
pub struct StatusResponse {
    pub version: String,
    pub model: String,
    pub provider: String,
    pub uptime_secs: i64,
}

pub async fn status(
    State(state): State<Arc<AppState>>,
) -> Json<StatusResponse> {
    let uptime = chrono::Utc::now() - state.started_at;
    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        model: state.agent.model_id().to_string(),
        provider: state.agent.provider_name().to_string(),
        uptime_secs: uptime.num_seconds(),
    })
}
