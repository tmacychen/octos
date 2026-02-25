//! WhatsApp channel via Node.js bridge (Baileys) over WebSocket.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use crew_core::{InboundMessage, OutboundMessage};
use eyre::Result;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::channel::Channel;

/// Default bridge URL for the Node.js WhatsApp bridge.
const DEFAULT_BRIDGE_URL: &str = "ws://localhost:3001";

pub struct WhatsAppChannel {
    bridge_url: String,
    allowed_senders: HashSet<String>,
    shutdown: Arc<AtomicBool>,
    /// Write half of the WebSocket for sending messages.
    ws_tx: Arc<tokio::sync::Mutex<Option<WsSink>>>,
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;

impl WhatsAppChannel {
    pub fn new(bridge_url: &str, allowed_senders: Vec<String>, shutdown: Arc<AtomicBool>) -> Self {
        let url = if bridge_url.is_empty() {
            DEFAULT_BRIDGE_URL.to_string()
        } else {
            bridge_url.to_string()
        };
        Self {
            bridge_url: url,
            allowed_senders: allowed_senders.into_iter().collect(),
            shutdown,
            ws_tx: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    fn check_allowed(&self, sender_id: &str) -> bool {
        self.allowed_senders.is_empty() || self.allowed_senders.contains(sender_id)
    }

    /// Extract a clean sender ID from a WhatsApp JID (e.g. "1234567890@s.whatsapp.net" -> "1234567890").
    fn clean_sender(jid: &str) -> &str {
        jid.split('@').next().unwrap_or(jid)
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn name(&self) -> &str {
        "whatsapp"
    }

    async fn start(&self, inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        info!(url = %self.bridge_url, "Starting WhatsApp channel");

        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            let (ws_stream, _) = match connect_async(&self.bridge_url).await {
                Ok(conn) => conn,
                Err(e) => {
                    error!("Failed to connect to WhatsApp bridge: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            info!("WhatsApp bridge connected");
            let (ws_tx, mut ws_rx) = ws_stream.split();

            // Store write half for send()
            *self.ws_tx.lock().await = Some(ws_tx);

            while let Some(frame) = ws_rx.next().await {
                if self.shutdown.load(Ordering::Relaxed) {
                    break;
                }

                let data = match frame {
                    Ok(WsMessage::Text(text)) => text,
                    Ok(WsMessage::Close(_)) => {
                        info!("WhatsApp bridge closed connection");
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        warn!("WhatsApp WebSocket error: {e}");
                        break;
                    }
                };

                let msg: serde_json::Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("Failed to parse WhatsApp bridge message: {e}");
                        continue;
                    }
                };

                let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

                match msg_type {
                    "message" => {
                        let sender = msg.get("sender").and_then(|v| v.as_str()).unwrap_or("");
                        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let is_group = msg
                            .get("isGroup")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        if sender.is_empty() || content.is_empty() {
                            continue;
                        }

                        let clean_id = Self::clean_sender(sender);

                        if !self.check_allowed(clean_id) {
                            continue;
                        }

                        // Use the full JID as chat_id for groups, clean number for DMs
                        let chat_id = if is_group {
                            sender.to_string()
                        } else {
                            clean_id.to_string()
                        };

                        let inbound = InboundMessage {
                            channel: "whatsapp".into(),
                            sender_id: clean_id.to_string(),
                            chat_id,
                            content: content.to_string(),
                            timestamp: Utc::now(),
                            media: vec![],
                            metadata: serde_json::json!({
                                "whatsapp": {
                                    "is_group": is_group,
                                    "jid": sender,
                                }
                            }),
                        };

                        if inbound_tx.send(inbound).await.is_err() {
                            return Ok(());
                        }
                    }
                    "status" => {
                        let status = msg.get("status").and_then(|v| v.as_str()).unwrap_or("");
                        info!("WhatsApp status: {status}");
                    }
                    "qr" => {
                        info!("WhatsApp QR code received — scan with your phone");
                    }
                    "error" => {
                        let err = msg.get("error").and_then(|v| v.as_str()).unwrap_or("");
                        warn!("WhatsApp bridge error: {err}");
                    }
                    _ => {
                        debug!("WhatsApp unknown message type: {msg_type}");
                    }
                }
            }

            // Clear write half on disconnect
            *self.ws_tx.lock().await = None;

            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            warn!("WhatsApp bridge disconnected, reconnecting in 5s...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }

        info!("WhatsApp channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<()> {
        let mut lock = self.ws_tx.lock().await;
        let Some(ref mut tx) = *lock else {
            warn!("WhatsApp bridge not connected, cannot send");
            return Ok(());
        };

        if !msg.media.is_empty() {
            // Send each media file (bridge handles image/video/audio/document detection)
            for (i, path) in msg.media.iter().enumerate() {
                let caption = if i == 0 { &msg.content } else { "" };
                let payload = serde_json::json!({
                    "type": "send",
                    "to": msg.chat_id,
                    "text": caption,
                    "media": path,
                });
                tx.send(WsMessage::Text(payload.to_string().into()))
                    .await
                    .map_err(|e| eyre::eyre!("failed to send WhatsApp media: {e}"))?;
            }
        } else {
            let payload = serde_json::json!({
                "type": "send",
                "to": msg.chat_id,
                "text": msg.content,
            });
            tx.send(WsMessage::Text(payload.to_string().into()))
                .await
                .map_err(|e| eyre::eyre!("failed to send WhatsApp message: {e}"))?;
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

    fn make_channel(allowed: Vec<&str>) -> WhatsAppChannel {
        WhatsAppChannel {
            bridge_url: DEFAULT_BRIDGE_URL.into(),
            allowed_senders: allowed.into_iter().map(String::from).collect(),
            shutdown: Arc::new(AtomicBool::new(false)),
            ws_tx: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    #[test]
    fn test_is_allowed_empty_list() {
        let ch = make_channel(vec![]);
        assert!(ch.is_allowed("anyone"));
    }

    #[test]
    fn test_is_allowed_matching() {
        let ch = make_channel(vec!["1234567890", "9876543210"]);
        assert!(ch.is_allowed("1234567890"));
        assert!(!ch.is_allowed("5555555555"));
    }

    #[test]
    fn test_clean_sender() {
        assert_eq!(
            WhatsAppChannel::clean_sender("1234567890@s.whatsapp.net"),
            "1234567890"
        );
        assert_eq!(
            WhatsAppChannel::clean_sender("plain_number"),
            "plain_number"
        );
    }

    #[test]
    fn test_bridge_message_parsing() {
        let json = r#"{
            "type": "message",
            "id": "3EB0123456789",
            "sender": "1234567890@s.whatsapp.net",
            "pn": "",
            "content": "Hello",
            "timestamp": 1699564800,
            "isGroup": false
        }"#;
        let msg: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(msg["type"], "message");
        assert_eq!(msg["sender"], "1234567890@s.whatsapp.net");
        assert_eq!(msg["content"], "Hello");
        assert_eq!(msg["isGroup"], false);
    }
}
