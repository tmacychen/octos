//! Voice platform skill binary (ASR/TTS/model management) via ominix-api.
//!
//! Protocol: `./main <tool_name>` with JSON on stdin, JSON on stdout.
//! Requires OMINIX_API_URL environment variable (default: http://localhost:8080).

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

// ── Input types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TranscribeInput {
    audio_path: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize)]
struct SynthesizeInput {
    text: String,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    speaker: Option<String>,
}

// ── Helpers ──────────────────────────────────────────────────────────

fn api_base_url() -> String {
    // Priority: env var > discovery file > default
    if let Ok(url) = std::env::var("OMINIX_API_URL") {
        return url.trim_end_matches('/').to_string();
    }
    // Read URL written by ominix-api on startup
    if let Some(home) = std::env::var_os("HOME") {
        let discovery = Path::new(&home).join(".ominix").join("api_url");
        if let Ok(url) = std::fs::read_to_string(&discovery) {
            let url = url.trim();
            if !url.is_empty() {
                return url.trim_end_matches('/').to_string();
            }
        }
    }
    "http://localhost:8080".to_string()
}

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build HTTP client")
}

fn check_health(client: &reqwest::blocking::Client, base_url: &str) -> Result<(), String> {
    match client
        .get(format!("{base_url}/health"))
        .timeout(Duration::from_secs(5))
        .send()
    {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => Err(format!(
            "ominix-api returned HTTP {} — is it running on {base_url}?",
            resp.status()
        )),
        Err(e) => Err(format!(
            "Cannot reach ominix-api at {base_url}: {e}. \
             Start it with: ominix-api --port 8081"
        )),
    }
}

fn fail(msg: &str) -> ! {
    let out = json!({"output": msg, "success": false});
    println!("{out}");
    std::process::exit(1);
}

fn succeed(msg: &str) -> ! {
    let out = json!({"output": msg, "success": true});
    println!("{out}");
    std::process::exit(0);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end: String = s.chars().take(max).collect();
        format!("{end}...")
    }
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── voice_transcribe ─────────────────────────────────────────────────

fn handle_transcribe(input_json: &str) {
    let input: TranscribeInput = match serde_json::from_str(input_json) {
        Ok(v) => v,
        Err(e) => fail(&format!("Invalid input: {e}")),
    };

    let path = Path::new(&input.audio_path);
    if !path.exists() {
        fail(&format!("Audio file not found: {}", input.audio_path));
    }
    if !path.is_file() {
        fail(&format!("Not a file: {}", input.audio_path));
    }
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() == 0 {
            fail("Audio file is empty (0 bytes)");
        }
        if meta.len() > 100_000_000 {
            fail("Audio file too large (>100MB)");
        }
    }

    let base_url = api_base_url();
    let client = http_client();
    if let Err(e) = check_health(&client, &base_url) {
        fail(&e);
    }

    let language = input.language.unwrap_or_else(|| "Chinese".to_string());

    // ominix-api accepts file paths (starting with '/') or base64 in the "file" field
    let body = json!({
        "file": input.audio_path,
        "language": language,
        "response_format": "verbose_json"
    });

    let resp = match client
        .post(format!("{base_url}/v1/audio/transcriptions"))
        .json(&body)
        .send()
    {
        Ok(r) => r,
        Err(e) => fail(&format!("ASR request failed: {e}")),
    };

    let status = resp.status();
    let resp_text = resp.text().unwrap_or_default();

    if !status.is_success() {
        fail(&format!(
            "ASR error (HTTP {status}): {}",
            truncate(&resp_text, 200)
        ));
    }

    let result: serde_json::Value = match serde_json::from_str(&resp_text) {
        Ok(v) => v,
        Err(e) => fail(&format!("Failed to parse ASR response: {e}")),
    };

    let text = result["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        fail("ASR returned empty transcription (silence or unsupported format)");
    }

    let mut output = text.to_string();
    if let Some(duration) = result["duration"].as_f64() {
        output = format!("{text}\n\n[Audio duration: {duration:.1}s]");
    }

    succeed(&output);
}

// ── voice_synthesize ─────────────────────────────────────────────────

fn synthesize_segment(
    client: &reqwest::blocking::Client,
    base_url: &str,
    text: &str,
    voice: &str,
    language: &str,
    output_path: &Path,
) -> Result<(usize, f64), String> {
    let body = json!({
        "input": text,
        "voice": voice,
        "language": language
    });

    let resp = client
        .post(format!("{base_url}/v1/audio/speech"))
        .json(&body)
        .send()
        .map_err(|e| format!("TTS request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let resp_text = resp.text().unwrap_or_default();
        return Err(format!(
            "TTS error (HTTP {status}): {}",
            truncate(&resp_text, 200)
        ));
    }

    let wav_bytes = resp
        .bytes()
        .map_err(|e| format!("Failed to read TTS response: {e}"))?;

    if wav_bytes.len() < 44 {
        return Err("TTS returned invalid WAV data (too small)".to_string());
    }

    std::fs::write(output_path, &wav_bytes)
        .map_err(|e| format!("Failed to write {}: {e}", output_path.display()))?;

    // 24kHz 16-bit mono = 48000 bytes/sec
    let duration_secs = wav_bytes.len().saturating_sub(44) as f64 / 48000.0;
    Ok((wav_bytes.len(), duration_secs))
}

fn handle_synthesize(input_json: &str) {
    let input: SynthesizeInput = match serde_json::from_str(input_json) {
        Ok(v) => v,
        Err(e) => fail(&format!("Invalid input: {e}")),
    };

    if input.text.trim().is_empty() {
        fail("'text' must not be empty");
    }

    let base_url = api_base_url();
    let client = http_client();
    if let Err(e) = check_health(&client, &base_url) {
        fail(&e);
    }

    let output_path = input
        .output_path
        .unwrap_or_else(|| format!("/tmp/crew_tts_{}.wav", timestamp()));

    if let Some(parent) = Path::new(&output_path).parent() {
        if !parent.exists() {
            fail(&format!(
                "Output directory does not exist: {}",
                parent.display()
            ));
        }
    }

    let language = input.language.unwrap_or_else(|| "chinese".to_string());
    let speaker = input.speaker.unwrap_or_else(|| "vivian".to_string());

    match synthesize_segment(
        &client,
        &base_url,
        &input.text,
        &speaker,
        &language,
        Path::new(&output_path),
    ) {
        Ok((size, duration_secs)) => {
            succeed(&format!(
                "Generated audio: {output_path} ({duration_secs:.1}s, {size} bytes). Use send_file to deliver it to the user."
            ));
        }
        Err(e) => fail(&e),
    }
}

// ── list_models ──────────────────────────────────────────────────────

fn handle_list_models(_input_json: &str) {
    let base_url = api_base_url();
    let client = http_client();
    if let Err(e) = check_health(&client, &base_url) {
        fail(&e);
    }

    // Get loaded models
    let loaded = match client.get(format!("{base_url}/v1/models")).send() {
        Ok(r) if r.status().is_success() => r.text().unwrap_or_default(),
        Ok(r) => fail(&format!("Failed to list models: HTTP {}", r.status())),
        Err(e) => fail(&format!("Failed to list models: {e}")),
    };
    let loaded: serde_json::Value = serde_json::from_str(&loaded).unwrap_or(json!({}));

    // Get catalog (available for download)
    let catalog = match client.get(format!("{base_url}/v1/models/catalog")).send() {
        Ok(r) if r.status().is_success() => r.text().unwrap_or_default(),
        _ => "{}".to_string(),
    };
    let catalog: serde_json::Value = serde_json::from_str(&catalog).unwrap_or(json!({}));

    let mut output = String::from("## Loaded Models\n\n");
    if let Some(models) = loaded.get("data").and_then(|d| d.as_array()) {
        if models.is_empty() {
            output.push_str("No models loaded.\n");
        }
        for m in models {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let mtype = m.get("type").and_then(|v| v.as_str()).unwrap_or("?");
            output.push_str(&format!("- {id} ({mtype})\n"));
        }
    }

    output.push_str("\n## Available Models (catalog)\n\n");
    if let Some(models) = catalog.get("models").and_then(|d| d.as_array()) {
        for m in models {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let mtype = m.get("type").and_then(|v| v.as_str()).unwrap_or("?");
            let downloaded = m
                .get("downloaded")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let status = if downloaded {
                "downloaded"
            } else {
                "not downloaded"
            };
            output.push_str(&format!("- {id} ({mtype}) [{status}]\n"));
        }
    }

    succeed(&output);
}

// ── download_model ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct DownloadModelInput {
    model_id: String,
}

fn handle_download_model(input_json: &str) {
    let input: DownloadModelInput = match serde_json::from_str(input_json) {
        Ok(v) => v,
        Err(e) => fail(&format!("Invalid input: {e}")),
    };

    let base_url = api_base_url();
    let client = http_client();
    if let Err(e) = check_health(&client, &base_url) {
        fail(&e);
    }

    let resp = match client
        .post(format!("{base_url}/v1/models/download"))
        .json(&json!({"model_id": input.model_id}))
        .timeout(Duration::from_secs(600))
        .send()
    {
        Ok(r) => r,
        Err(e) => fail(&format!("Download request failed: {e}")),
    };

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        fail(&format!(
            "Download failed (HTTP {status}): {}",
            truncate(&text, 200)
        ));
    }

    succeed(&format!(
        "Download started for model: {}. Use list_models to check status.",
        input.model_id
    ));
}

// ── load_model / unload_model ────────────────────────────────────────

#[derive(Deserialize)]
struct LoadModelInput {
    model: String,
    #[serde(default = "default_model_type")]
    model_type: String,
}

fn default_model_type() -> String {
    "llm".to_string()
}

fn handle_load_model(input_json: &str) {
    let input: LoadModelInput = match serde_json::from_str(input_json) {
        Ok(v) => v,
        Err(e) => fail(&format!("Invalid input: {e}")),
    };

    let base_url = api_base_url();
    let client = http_client();
    if let Err(e) = check_health(&client, &base_url) {
        fail(&e);
    }

    let resp = match client
        .post(format!("{base_url}/v1/models/load"))
        .json(&json!({"model": input.model, "model_type": input.model_type}))
        .timeout(Duration::from_secs(120))
        .send()
    {
        Ok(r) => r,
        Err(e) => fail(&format!("Load request failed: {e}")),
    };

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        fail(&format!(
            "Load failed (HTTP {status}): {}",
            truncate(&text, 200)
        ));
    }

    succeed(&format!(
        "Model loaded: {} (type: {})",
        input.model, input.model_type
    ));
}

#[derive(Deserialize)]
struct UnloadModelInput {
    model_type: String,
}

fn handle_unload_model(input_json: &str) {
    let input: UnloadModelInput = match serde_json::from_str(input_json) {
        Ok(v) => v,
        Err(e) => fail(&format!("Invalid input: {e}")),
    };

    let base_url = api_base_url();
    let client = http_client();
    if let Err(e) = check_health(&client, &base_url) {
        fail(&e);
    }

    let resp = match client
        .post(format!("{base_url}/v1/models/unload"))
        .json(&json!({"model_type": input.model_type}))
        .send()
    {
        Ok(r) => r,
        Err(e) => fail(&format!("Unload request failed: {e}")),
    };

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        fail(&format!(
            "Unload failed (HTTP {status}): {}",
            truncate(&text, 200)
        ));
    }

    succeed(&format!("Model unloaded: {}", input.model_type));
}

// ── Main ─────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tool_name = args.get(1).map(|s| s.as_str()).unwrap_or("unknown");

    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        fail(&format!("Failed to read stdin: {e}"));
    }

    match tool_name {
        "voice_transcribe" => handle_transcribe(&buf),
        "voice_synthesize" => handle_synthesize(&buf),
        "list_models" => handle_list_models(&buf),
        "download_model" => handle_download_model(&buf),
        "load_model" => handle_load_model(&buf),
        "unload_model" => handle_unload_model(&buf),
        _ => fail(&format!(
            "Unknown tool '{tool_name}'. Expected: voice_transcribe, voice_synthesize, list_models, download_model, load_model, unload_model"
        )),
    }
}
