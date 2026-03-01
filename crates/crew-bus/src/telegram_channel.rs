//! Telegram channel using teloxide long polling.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use crew_core::{InboundMessage, OutboundMessage};
use eyre::{Result, WrapErr};
use reqwest::Client;
use teloxide::prelude::*;
use teloxide::types::{ChatId, FileId, InputFile, ParseMode, UpdateKind};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::channel::Channel;
use crate::markdown_html::markdown_to_telegram_html;
use crate::media::download_media;

pub struct TelegramChannel {
    bot: Bot,
    allowed_senders: HashSet<String>,
    shutdown: Arc<AtomicBool>,
    media_dir: PathBuf,
    http: Client,
}

impl TelegramChannel {
    pub fn new(
        token: &str,
        allowed_senders: Vec<String>,
        shutdown: Arc<AtomicBool>,
        media_dir: PathBuf,
    ) -> Self {
        Self {
            bot: Bot::new(token),
            allowed_senders: allowed_senders.into_iter().collect(),
            shutdown,
            media_dir,
            http: Client::new(),
        }
    }

    fn check_allowed(&self, sender_id: &str) -> bool {
        if self.allowed_senders.is_empty() {
            return true;
        }
        if self.allowed_senders.contains(sender_id) {
            return true;
        }
        sender_id
            .split('|')
            .any(|part| self.allowed_senders.contains(part))
    }

    /// Download a file from Telegram by file_id.
    async fn download_telegram_file(&self, file_id: &FileId, ext: &str) -> Result<PathBuf> {
        let file = self
            .bot
            .get_file(file_id.clone())
            .await
            .wrap_err("failed to get file info from Telegram")?;

        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.bot.token(),
            file.path
        );

        let filename = format!("{}{}", file.meta.unique_id, ext);
        download_media(&self.http, &url, &[], &self.media_dir, &filename).await
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    fn max_message_length(&self) -> usize {
        4000
    }

    async fn start(&self, inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        use futures::StreamExt;
        use teloxide::update_listeners::{AsUpdateStream, polling_default};

        info!("Starting Telegram channel (long polling)");

        let mut listener = polling_default(self.bot.clone()).await;
        let stream = listener.as_stream();
        tokio::pin!(stream);

        while let Some(result) = stream.next().await {
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            let update = match result {
                Ok(update) => update,
                Err(e) => {
                    warn!("Telegram polling error: {e}");
                    continue;
                }
            };

            if let UpdateKind::Message(msg) = update.kind {
                // Extract text: plain text or caption (for photos/documents)
                let text = msg.text().or(msg.caption()).unwrap_or("").to_string();

                // Download media attachments
                let mut media = Vec::new();

                if let Some(sizes) = msg.photo() {
                    if let Some(photo) = sizes.last() {
                        match self.download_telegram_file(&photo.file.id, ".jpg").await {
                            Ok(path) => media.push(path.display().to_string()),
                            Err(e) => warn!("failed to download photo: {e}"),
                        }
                    }
                }

                if let Some(voice) = msg.voice() {
                    match self.download_telegram_file(&voice.file.id, ".ogg").await {
                        Ok(path) => media.push(path.display().to_string()),
                        Err(e) => warn!("failed to download voice: {e}"),
                    }
                }

                if let Some(audio) = msg.audio() {
                    let ext = audio
                        .file_name
                        .as_ref()
                        .and_then(|n| std::path::Path::new(n).extension())
                        .map(|e| format!(".{}", e.to_string_lossy()))
                        .unwrap_or_else(|| ".mp3".to_string());
                    match self.download_telegram_file(&audio.file.id, &ext).await {
                        Ok(path) => media.push(path.display().to_string()),
                        Err(e) => warn!("failed to download audio: {e}"),
                    }
                }

                if let Some(doc) = msg.document() {
                    let ext = doc
                        .file_name
                        .as_ref()
                        .and_then(|n| std::path::Path::new(n).extension())
                        .map(|e| format!(".{}", e.to_string_lossy()))
                        .unwrap_or_default();
                    match self.download_telegram_file(&doc.file.id, &ext).await {
                        Ok(path) => media.push(path.display().to_string()),
                        Err(e) => warn!("failed to download document: {e}"),
                    }
                }

                // Skip messages with no text and no media
                if text.is_empty() && media.is_empty() {
                    continue;
                }

                let sender_id = msg
                    .from
                    .as_ref()
                    .map(|u| {
                        let id = u.id.to_string();
                        match &u.username {
                            Some(name) => format!("{id}|{name}"),
                            None => id,
                        }
                    })
                    .unwrap_or_default();

                if !self.check_allowed(&sender_id) {
                    continue;
                }

                let inbound = InboundMessage {
                    channel: "telegram".into(),
                    sender_id,
                    chat_id: msg.chat.id.0.to_string(),
                    content: text,
                    timestamp: Utc::now(),
                    media,
                    metadata: serde_json::json!({}),
                };

                if inbound_tx.send(inbound).await.is_err() {
                    break;
                }
            }
        }

        info!("Telegram channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<()> {
        let chat_id: i64 = msg
            .chat_id
            .parse()
            .wrap_err_with(|| format!("invalid Telegram chat_id: {}", msg.chat_id))?;

        if !msg.media.is_empty() {
            // Send files as documents
            let caption = if msg.content.is_empty() {
                None
            } else {
                Some(markdown_to_telegram_html(&msg.content))
            };

            for (i, path) in msg.media.iter().enumerate() {
                let file = InputFile::file(std::path::PathBuf::from(path));
                let mut req = self.bot.send_document(ChatId(chat_id), file);
                // Only attach caption to the first file
                if i == 0 {
                    if let Some(ref cap) = caption {
                        req = req.caption(cap).parse_mode(ParseMode::Html);
                    }
                }
                req.await
                    .wrap_err_with(|| format!("failed to send document: {path}"))?;
            }
        } else {
            // Normal text message
            let html = markdown_to_telegram_html(&msg.content);

            self.bot
                .send_message(ChatId(chat_id), &html)
                .parse_mode(ParseMode::Html)
                .await
                .wrap_err("failed to send Telegram message")?;
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

    async fn send_typing(&self, chat_id: &str) -> Result<()> {
        let id: i64 = chat_id
            .parse()
            .wrap_err_with(|| format!("invalid Telegram chat_id: {chat_id}"))?;
        self.bot
            .send_chat_action(ChatId(id), teloxide::types::ChatAction::Typing)
            .await
            .wrap_err("failed to send typing action")?;
        Ok(())
    }

    async fn send_with_id(&self, msg: &OutboundMessage) -> Result<Option<String>> {
        let chat_id: i64 = msg
            .chat_id
            .parse()
            .wrap_err_with(|| format!("invalid Telegram chat_id: {}", msg.chat_id))?;

        let html = markdown_to_telegram_html(&msg.content);
        let sent = self
            .bot
            .send_message(ChatId(chat_id), &html)
            .parse_mode(ParseMode::Html)
            .await
            .wrap_err("failed to send Telegram message")?;

        Ok(Some(sent.id.0.to_string()))
    }

    async fn edit_message(&self, chat_id: &str, message_id: &str, new_content: &str) -> Result<()> {
        let cid: i64 = chat_id
            .parse()
            .wrap_err_with(|| format!("invalid Telegram chat_id: {chat_id}"))?;
        let mid: i32 = message_id
            .parse()
            .wrap_err_with(|| format!("invalid Telegram message_id: {message_id}"))?;

        let html = markdown_to_telegram_html(new_content);
        self.bot
            .edit_message_text(ChatId(cid), teloxide::types::MessageId(mid), &html)
            .parse_mode(ParseMode::Html)
            .await
            .wrap_err("failed to edit Telegram message")?;
        Ok(())
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()> {
        let cid: i64 = chat_id
            .parse()
            .wrap_err_with(|| format!("invalid Telegram chat_id: {chat_id}"))?;
        let mid: i32 = message_id
            .parse()
            .wrap_err_with(|| format!("invalid Telegram message_id: {message_id}"))?;

        self.bot
            .delete_message(ChatId(cid), teloxide::types::MessageId(mid))
            .await
            .wrap_err("failed to delete Telegram message")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel(allowed: Vec<&str>) -> TelegramChannel {
        TelegramChannel {
            bot: Bot::new("test:token"),
            allowed_senders: allowed.into_iter().map(String::from).collect(),
            shutdown: Arc::new(AtomicBool::new(false)),
            media_dir: PathBuf::from("/tmp/test-media"),
            http: Client::new(),
        }
    }

    #[test]
    fn test_is_allowed_empty_list() {
        let ch = make_channel(vec![]);
        assert!(ch.is_allowed("anyone"));
        assert!(ch.is_allowed("12345"));
    }

    #[test]
    fn test_is_allowed_matching() {
        let ch = make_channel(vec!["12345", "67890"]);
        assert!(ch.is_allowed("12345"));
        assert!(ch.is_allowed("67890"));
        assert!(!ch.is_allowed("99999"));
    }

    #[test]
    fn test_is_allowed_compound_id() {
        let ch = make_channel(vec!["12345", "johndoe"]);
        assert!(ch.is_allowed("12345|johndoe"));
        assert!(ch.is_allowed("12345|other"));
        assert!(ch.is_allowed("99999|johndoe"));
        assert!(!ch.is_allowed("99999|other"));
    }
}
