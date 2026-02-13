//! Feishu/Lark channel using WebSocket long connection + REST API.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use crew_core::{InboundMessage, OutboundMessage};
use eyre::{Result, WrapErr};
use futures::StreamExt;
use reqwest::Client;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::channel::Channel;

const FEISHU_BASE: &str = "https://open.feishu.cn/open-apis";
/// Token refresh interval (slightly under 2 hours).
const TOKEN_TTL_SECS: u64 = 7000;
/// Maximum message IDs to track for dedup.
const MAX_SEEN_IDS: usize = 1000;

pub struct FeishuChannel {
    app_id: String,
    app_secret: String,
    allowed_senders: HashSet<String>,
    shutdown: Arc<AtomicBool>,
    http: Client,
    token_cache: Arc<tokio::sync::Mutex<Option<(String, Instant)>>>,
    seen_ids: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl FeishuChannel {
    pub fn new(
        app_id: &str,
        app_secret: &str,
        allowed_senders: Vec<String>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            app_id: app_id.to_string(),
            app_secret: app_secret.to_string(),
            allowed_senders: allowed_senders.into_iter().collect(),
            shutdown,
            http: Client::new(),
            token_cache: Arc::new(tokio::sync::Mutex::new(None)),
            seen_ids: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }
    }

    fn check_allowed(&self, sender_id: &str) -> bool {
        self.allowed_senders.is_empty() || self.allowed_senders.contains(sender_id)
    }

    /// Get or refresh tenant access token.
    async fn get_token(&self) -> Result<String> {
        let mut cache = self.token_cache.lock().await;
        if let Some((ref token, ref created)) = *cache {
            if created.elapsed().as_secs() < TOKEN_TTL_SECS {
                return Ok(token.clone());
            }
        }

        let resp: serde_json::Value = self
            .http
            .post(format!(
                "{FEISHU_BASE}/auth/v3/tenant_access_token/internal"
            ))
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "app_secret": self.app_secret,
            }))
            .send()
            .await
            .wrap_err("failed to get tenant token")?
            .json()
            .await?;

        let token = resp
            .get("tenant_access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                let msg = resp
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                eyre::eyre!("Feishu token error: {msg}")
            })?
            .to_string();

        *cache = Some((token.clone(), Instant::now()));
        Ok(token)
    }

    /// Get WebSocket gateway URL from Feishu bot gateway endpoint.
    async fn get_ws_url(&self) -> Result<String> {
        let token = self.get_token().await?;
        let resp: serde_json::Value = self
            .http
            .post(format!("{FEISHU_BASE}/callback/ws/endpoint"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .wrap_err("failed to get Feishu WS endpoint")?
            .json()
            .await?;

        let data = resp.get("data").ok_or_else(|| {
            let msg = resp
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            eyre::eyre!("Feishu WS endpoint error: {msg}")
        })?;

        let url = data
            .get("URL")
            .or_else(|| data.get("url"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("no URL in Feishu WS endpoint response"))?;

        Ok(url.to_string())
    }

    /// Check if a message ID has been seen; add if not. Trims when over capacity.
    fn dedup_check(&self, msg_id: &str) -> bool {
        let mut seen = self.seen_ids.lock().unwrap();
        if seen.contains(msg_id) {
            return true;
        }
        if seen.len() >= MAX_SEEN_IDS {
            seen.clear();
        }
        seen.insert(msg_id.to_string());
        false
    }

    /// Determine receive_id_type from chat_id prefix.
    fn receive_id_type(chat_id: &str) -> &'static str {
        if chat_id.starts_with("oc_") {
            "chat_id"
        } else {
            "open_id"
        }
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn name(&self) -> &str {
        "feishu"
    }

    async fn start(&self, inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        info!("Starting Feishu channel");

        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            let ws_url = match self.get_ws_url().await {
                Ok(url) => url,
                Err(e) => {
                    error!("Failed to get Feishu WS URL: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            let (ws_stream, _) = match connect_async(&ws_url).await {
                Ok(conn) => conn,
                Err(e) => {
                    error!("Failed to connect Feishu WebSocket: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            info!("Feishu WebSocket connected");
            let (_ws_tx, mut ws_rx) = ws_stream.split();

            while let Some(frame) = ws_rx.next().await {
                if self.shutdown.load(Ordering::Relaxed) {
                    break;
                }

                let data = match frame {
                    Ok(WsMessage::Text(text)) => text,
                    Ok(WsMessage::Close(_)) => {
                        info!("Feishu WebSocket closed by server");
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        warn!("Feishu WebSocket error: {e}");
                        break;
                    }
                };

                let envelope: serde_json::Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("Failed to parse Feishu envelope: {e}");
                        continue;
                    }
                };

                // Feishu WS events have a header.event_type field
                let event_type = envelope
                    .get("header")
                    .and_then(|h| h.get("event_type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if event_type != "im.message.receive_v1" {
                    continue;
                }

                let event = match envelope.get("event") {
                    Some(e) => e,
                    None => continue,
                };

                // Extract message_id for dedup
                let message = match event.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                let message_id = message
                    .get("message_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if message_id.is_empty() || self.dedup_check(message_id) {
                    continue;
                }

                // Extract sender
                let sender_id = event
                    .get("sender")
                    .and_then(|s| s.get("sender_id"))
                    .and_then(|s| s.get("open_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Extract chat_id
                let chat_id = message
                    .get("chat_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if sender_id.is_empty() || chat_id.is_empty() {
                    continue;
                }

                if !self.check_allowed(sender_id) {
                    continue;
                }

                // Extract text content from message.content JSON
                let msg_type = message
                    .get("message_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let content = if msg_type == "text" {
                    message
                        .get("content")
                        .and_then(|v| v.as_str())
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
                        .unwrap_or_default()
                } else {
                    format!("[{msg_type} message]")
                };

                if content.is_empty() {
                    continue;
                }

                let inbound = InboundMessage {
                    channel: "feishu".into(),
                    sender_id: sender_id.to_string(),
                    chat_id: chat_id.to_string(),
                    content,
                    timestamp: Utc::now(),
                    media: vec![],
                    metadata: serde_json::json!({
                        "feishu": {
                            "message_id": message_id,
                            "message_type": msg_type,
                        }
                    }),
                };

                if inbound_tx.send(inbound).await.is_err() {
                    return Ok(());
                }
            }

            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            warn!("Feishu WebSocket disconnected, reconnecting in 2s...");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        info!("Feishu channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<()> {
        let token = self.get_token().await?;
        let id_type = Self::receive_id_type(&msg.chat_id);

        let body = serde_json::json!({
            "receive_id": msg.chat_id,
            "msg_type": "text",
            "content": serde_json::json!({"text": msg.content}).to_string(),
        });

        let resp: serde_json::Value = self
            .http
            .post(format!(
                "{FEISHU_BASE}/im/v1/messages?receive_id_type={id_type}"
            ))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .wrap_err("failed to send Feishu message")?
            .json()
            .await?;

        let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let err_msg = resp
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            warn!("Feishu send error: {err_msg}");
        }

        Ok(())
    }

    fn is_allowed(&self, sender_id: &str) -> bool {
        self.check_allowed(sender_id)
    }

    async fn stop(&self) -> Result<()> {
        self.shutdown.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel(allowed: Vec<&str>) -> FeishuChannel {
        FeishuChannel {
            app_id: "test_id".into(),
            app_secret: "test_secret".into(),
            allowed_senders: allowed.into_iter().map(String::from).collect(),
            shutdown: Arc::new(AtomicBool::new(false)),
            http: Client::new(),
            token_cache: Arc::new(tokio::sync::Mutex::new(None)),
            seen_ids: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }
    }

    #[test]
    fn test_is_allowed_empty_list() {
        let ch = make_channel(vec![]);
        assert!(ch.is_allowed("anyone"));
    }

    #[test]
    fn test_is_allowed_matching() {
        let ch = make_channel(vec!["ou_123", "ou_456"]);
        assert!(ch.is_allowed("ou_123"));
        assert!(!ch.is_allowed("ou_789"));
    }

    #[test]
    fn test_receive_id_type() {
        assert_eq!(FeishuChannel::receive_id_type("oc_abc123"), "chat_id");
        assert_eq!(FeishuChannel::receive_id_type("ou_xyz789"), "open_id");
        assert_eq!(FeishuChannel::receive_id_type("other"), "open_id");
    }

    #[test]
    fn test_dedup() {
        let ch = make_channel(vec![]);
        assert!(!ch.dedup_check("msg1"));
        assert!(ch.dedup_check("msg1")); // duplicate
        assert!(!ch.dedup_check("msg2"));
    }

    #[test]
    fn test_dedup_overflow_clears() {
        let ch = make_channel(vec![]);
        for i in 0..MAX_SEEN_IDS {
            ch.dedup_check(&format!("msg_{i}"));
        }
        // At capacity — next insert triggers clear
        assert!(!ch.dedup_check("new_msg"));
        // Old ones should now be gone (cleared on overflow)
        assert!(!ch.dedup_check("msg_0"));
    }
}
