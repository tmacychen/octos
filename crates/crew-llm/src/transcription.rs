//! Voice transcription via Groq Whisper API.

use std::path::Path;

use eyre::{Result, WrapErr};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::debug;

/// Transcribes audio files using the Groq Whisper API.
pub struct GroqTranscriber {
    client: Client,
    api_key: SecretString,
    model: String,
}

impl GroqTranscriber {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: SecretString::from(api_key.into()),
            model: "whisper-large-v3".to_string(),
        }
    }

    /// Transcribe an audio file to text.
    pub async fn transcribe(&self, audio_path: &Path) -> Result<String> {
        let bytes = tokio::fs::read(audio_path)
            .await
            .wrap_err_with(|| format!("failed to read audio: {}", audio_path.display()))?;

        let filename = audio_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.ogg")
            .to_string();

        let ext = audio_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("ogg");

        let mime = match ext {
            "ogg" | "oga" | "opus" => "audio/ogg",
            "mp3" => "audio/mpeg",
            "m4a" => "audio/mp4",
            "wav" => "audio/wav",
            _ => "audio/ogg",
        };

        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str(mime)
            .wrap_err("invalid MIME type")?;

        let form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .part("file", part);

        let resp = self
            .client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .multipart(form)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .wrap_err("failed to call Groq transcription API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eyre::bail!("Groq transcription failed: {status} - {body}");
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .wrap_err("invalid transcription response")?;

        let text = json
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("no text field in transcription response"))?;

        debug!(chars = text.len(), "audio transcribed");
        Ok(text.to_string())
    }
}
