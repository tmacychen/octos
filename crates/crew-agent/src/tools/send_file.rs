//! Send file tool for delivering files to chat channels.

use std::path::Path;

use async_trait::async_trait;
use crew_core::OutboundMessage;
use eyre::{Result, WrapErr};
use serde::Deserialize;
use tokio::sync::mpsc;

use super::{Tool, ToolResult};

/// Tool that sends a file to the current chat channel as a document attachment.
pub struct SendFileTool {
    out_tx: mpsc::Sender<OutboundMessage>,
    default_channel: std::sync::Mutex<String>,
    default_chat_id: std::sync::Mutex<String>,
}

impl SendFileTool {
    pub fn new(out_tx: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            out_tx,
            default_channel: std::sync::Mutex::new(String::new()),
            default_chat_id: std::sync::Mutex::new(String::new()),
        }
    }

    /// Create a new SendFileTool with context pre-set (for per-session instances).
    pub fn with_context(
        out_tx: mpsc::Sender<OutboundMessage>,
        channel: impl Into<String>,
        chat_id: impl Into<String>,
    ) -> Self {
        Self {
            out_tx,
            default_channel: std::sync::Mutex::new(channel.into()),
            default_chat_id: std::sync::Mutex::new(chat_id.into()),
        }
    }

    /// Update the default channel/chat_id context (called per inbound message).
    /// WARNING: This mutates shared state. See MessageTool::set_context() for details.
    pub fn set_context(&self, channel: &str, chat_id: &str) {
        *self
            .default_channel
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = channel.to_string();
        *self
            .default_chat_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = chat_id.to_string();
    }
}

#[derive(Deserialize)]
struct Input {
    file_path: String,
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
}

#[async_trait]
impl Tool for SendFileTool {
    fn name(&self) -> &str {
        "send_file"
    }

    fn description(&self) -> &str {
        "Send a file to the user as a document attachment. Use this to deliver files \
         (reports, code, data, etc.) directly to the chat. The file is sent as-is, \
         not rendered as text."
    }

    fn tags(&self) -> &[&str] {
        &["gateway"]
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to send"
                },
                "caption": {
                    "type": "string",
                    "description": "Optional caption/description for the file"
                },
                "channel": {
                    "type": "string",
                    "description": "Target channel. Defaults to current."
                },
                "chat_id": {
                    "type": "string",
                    "description": "Target chat/user ID. Defaults to current."
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: Input =
            serde_json::from_value(args.clone()).wrap_err("invalid send_file tool input")?;

        // Validate file exists
        let path = Path::new(&input.file_path);
        if !path.exists() {
            return Ok(ToolResult {
                output: format!("Error: File not found: {}", input.file_path),
                success: false,
                ..Default::default()
            });
        }
        if !path.is_file() {
            return Ok(ToolResult {
                output: format!("Error: Not a file: {}", input.file_path),
                success: false,
                ..Default::default()
            });
        }

        let channel = input.channel.unwrap_or_else(|| {
            self.default_channel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        });
        let chat_id = input.chat_id.unwrap_or_else(|| {
            self.default_chat_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        });

        if channel.is_empty() || chat_id.is_empty() {
            return Ok(ToolResult {
                output: "Error: No target channel/chat specified.".into(),
                success: false,
                ..Default::default()
            });
        }

        let msg = OutboundMessage {
            channel: channel.clone(),
            chat_id: chat_id.clone(),
            content: input.caption.unwrap_or_default(),
            reply_to: None,
            media: vec![input.file_path.clone()],
            metadata: serde_json::json!({}),
        };

        self.out_tx
            .send(msg)
            .await
            .map_err(|e| eyre::eyre!("failed to send file message: {e}"))?;

        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| input.file_path.clone());

        Ok(ToolResult {
            output: format!("File '{filename}' sent to {channel}:{chat_id}"),
            success: true,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_send_file() {
        let (tx, mut rx) = mpsc::channel(16);
        let tool = SendFileTool::new(tx);
        tool.set_context("telegram", "12345");

        // Create a temp file
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "hello world").unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let result = tool
            .execute(&serde_json::json!({
                "file_path": path,
                "caption": "Here is the file"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("sent to telegram:12345"));

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.channel, "telegram");
        assert_eq!(msg.chat_id, "12345");
        assert_eq!(msg.content, "Here is the file");
        assert_eq!(msg.media.len(), 1);
        assert_eq!(msg.media[0], path);
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let (tx, _rx) = mpsc::channel(16);
        let tool = SendFileTool::new(tx);
        tool.set_context("telegram", "12345");

        let result = tool
            .execute(&serde_json::json!({
                "file_path": "/nonexistent/file.txt"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn test_no_target() {
        let (tx, _rx) = mpsc::channel(16);
        let tool = SendFileTool::new(tx);
        // No context set

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "data").unwrap();

        let result = tool
            .execute(&serde_json::json!({
                "file_path": tmp.path().to_string_lossy().to_string()
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.output.contains("No target"));
    }
}
