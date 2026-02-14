//! Browser automation tool using Chrome DevTools Protocol.
//!
//! Launches headless Chrome on first use, communicates via CDP over WebSocket.
//! Feature-gated behind `browser`.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine;
use eyre::{Result, WrapErr, bail};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::{Tool, ToolResult};
use crate::sandbox::BLOCKED_ENV_VARS;

const CHROME_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_OUTPUT_CHARS: usize = 50_000;

// --- SSRF protection (mirrored from web_fetch.rs) ---

fn is_private_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower == "localhost." {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_private_ip(&ip);
    }
    false
}

fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || matches!(v6.segments()[0], 0xfc00..=0xfdff)
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || (v6.segments()[0] & 0xffc0) == 0xfec0
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified())
                || v6
                    .to_ipv4()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified())
        }
    }
}

async fn check_ssrf(url: &str) -> Option<String> {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return Some("Invalid URL".to_string()),
    };
    let host = match parsed.host_str() {
        Some(h) => h,
        None => return Some("URL has no host".to_string()),
    };
    if is_private_host(host) {
        return Some("Requests to private/internal hosts are not allowed".to_string());
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    if let Ok(addrs) = tokio::net::lookup_host(format!("{host}:{port}")).await {
        for addr in addrs {
            if is_private_ip(&addr.ip()) {
                return Some(
                    "Requests to private/internal hosts are not allowed (DNS resolved to private IP)"
                        .to_string(),
                );
            }
        }
    }
    None
}

// --- CDP client ---

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>;
type WsStream = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

struct CdpClient {
    sink: WsSink,
    stream: WsStream,
    next_id: u32,
}

impl CdpClient {
    async fn connect(ws_url: &str) -> Result<Self> {
        let (ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .wrap_err("failed to connect to Chrome DevTools WebSocket")?;
        let (sink, stream) = ws.split();
        Ok(Self {
            sink,
            stream,
            next_id: 1,
        })
    }

    async fn send(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "id": id, "method": method, "params": params });
        self.sink
            .send(WsMessage::Text(msg.to_string().into()))
            .await
            .wrap_err("failed to send CDP message")?;

        let deadline = Instant::now() + ACTION_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("CDP response timeout for {method}");
            }
            let frame = tokio::time::timeout(remaining, self.stream.next())
                .await
                .map_err(|_| eyre::eyre!("CDP response timeout for {method}"))?
                .ok_or_else(|| eyre::eyre!("WebSocket closed"))??;

            let text = match frame {
                WsMessage::Text(t) => t.to_string(),
                WsMessage::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                WsMessage::Close(_) => bail!("WebSocket closed by Chrome"),
                _ => continue,
            };
            let resp: Value = serde_json::from_str(&text)?;
            // Skip events (no "id" field)
            if resp.get("id").and_then(|v| v.as_u64()) == Some(u64::from(id)) {
                if let Some(err) = resp.get("error") {
                    bail!("CDP error: {err}");
                }
                return Ok(resp.get("result").cloned().unwrap_or(json!({})));
            }
        }
    }

    /// Wait for a specific event (e.g. "Page.loadEventFired").
    async fn wait_event(&mut self, event_name: &str, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timeout waiting for {event_name}");
            }
            let frame = tokio::time::timeout(remaining, self.stream.next())
                .await
                .map_err(|_| eyre::eyre!("timeout waiting for {event_name}"))?
                .ok_or_else(|| eyre::eyre!("WebSocket closed"))??;

            let text = match frame {
                WsMessage::Text(t) => t.to_string(),
                WsMessage::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                WsMessage::Close(_) => bail!("WebSocket closed while waiting for {event_name}"),
                _ => continue,
            };
            let msg: Value = serde_json::from_str(&text)?;
            if msg.get("method").and_then(|m| m.as_str()) == Some(event_name) {
                return Ok(msg);
            }
        }
    }
}

// --- Browser session ---

struct BrowserSession {
    process: Child,
    client: CdpClient,
    last_used: Instant,
    _temp_dir: tempfile::TempDir,
}

impl BrowserSession {
    async fn launch() -> Result<Self> {
        let binary = find_chrome_binary()?;
        let temp_dir = tempfile::Builder::new()
            .prefix("crew-browser-")
            .tempdir()
            .wrap_err("failed to create temp dir for Chrome")?;

        let mut cmd = Command::new(&binary);
        cmd.args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--disable-extensions",
            "--disable-background-networking",
        ]);
        cmd.arg(format!(
            "--user-data-dir={}",
            temp_dir.path().display()
        ));
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        // Sanitize environment
        for var in BLOCKED_ENV_VARS {
            cmd.env_remove(var);
        }

        let mut child = cmd.spawn().wrap_err_with(|| {
            format!("failed to launch Chrome at {}", binary.display())
        })?;

        // Parse DevTools WS URL from stderr
        let stderr = child.stderr.take().ok_or_else(|| eyre::eyre!("no stderr"))?;
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();

        let ws_url = tokio::time::timeout(CHROME_STARTUP_TIMEOUT, async {
            while let Some(line) = lines.next_line().await? {
                // Line looks like: "DevTools listening on ws://127.0.0.1:PORT/devtools/browser/UUID"
                if let Some(pos) = line.find("ws://") {
                    return Ok::<String, eyre::Report>(line[pos..].trim().to_string());
                }
            }
            bail!("Chrome exited without printing DevTools URL")
        })
        .await
        .map_err(|_| eyre::eyre!("Chrome startup timeout ({}s)", CHROME_STARTUP_TIMEOUT.as_secs()))??;

        // Drain stderr in background to prevent pipe buffer fill blocking Chrome
        let stderr_inner = lines.into_inner().into_inner();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            use tokio::io::AsyncReadExt;
            let mut reader = stderr_inner;
            while reader.read(&mut buf).await.unwrap_or(0) > 0 {}
        });

        // The stderr URL is the *browser* endpoint. We need a *page* endpoint.
        // Extract port and query /json to find the page target's WS URL.
        let port = ws_url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split(':').nth(1))
            .and_then(|port_and_path| port_and_path.split('/').next())
            .ok_or_else(|| eyre::eyre!("cannot parse port from WS URL: {ws_url}"))?;

        let json_url = format!("http://127.0.0.1:{port}/json");
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        let targets: Vec<Value> = http_client
            .get(&json_url)
            .send()
            .await
            .wrap_err("failed to query Chrome /json endpoint")?
            .json()
            .await
            .wrap_err("failed to parse /json response")?;

        let page_ws_url = targets
            .iter()
            .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
            .and_then(|t| t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()))
            .ok_or_else(|| eyre::eyre!("no page target found in Chrome /json"))?
            .to_string();

        let mut client = CdpClient::connect(&page_ws_url).await?;
        client.send("Page.enable", json!({})).await?;

        Ok(Self {
            process: child,
            client,
            last_used: Instant::now(),
            _temp_dir: temp_dir,
        })
    }

    fn touch(&mut self) {
        self.last_used = Instant::now();
    }

    fn is_idle(&self) -> bool {
        self.last_used.elapsed() > IDLE_TIMEOUT
    }

    async fn shutdown(&mut self) {
        let _ = self.client.send("Browser.close", json!({})).await;
        let _ = self.process.kill().await;
        let _ = self.process.wait().await; // Reap to prevent zombie
    }
}

// --- Chrome binary discovery ---

fn find_chrome_binary() -> Result<std::path::PathBuf> {
    // Try well-known names in PATH
    let names = [
        "google-chrome-stable",
        "google-chrome",
        "chromium-browser",
        "chromium",
        "chrome",
    ];
    for name in &names {
        if let Ok(path) = which::which(name) {
            return Ok(path);
        }
    }

    // Platform-specific paths
    #[cfg(target_os = "macos")]
    {
        let paths = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ];
        for p in &paths {
            let path = std::path::PathBuf::from(p);
            if path.exists() {
                return Ok(path);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let paths = [
            "/usr/bin/google-chrome-stable",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium-browser",
            "/usr/bin/chromium",
            "/snap/bin/chromium",
        ];
        for p in &paths {
            let path = std::path::PathBuf::from(p);
            if path.exists() {
                return Ok(path);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let paths = [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ];
        for p in &paths {
            let path = std::path::PathBuf::from(p);
            if path.exists() {
                return Ok(path);
            }
        }
    }

    bail!("Chrome/Chromium not found. Install Chrome or set it in PATH.")
}

// --- Tool input ---

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    expression: Option<String>,
}

// --- BrowserTool ---

pub struct BrowserTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserTool {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
        }
    }

    async fn ensure_session(
        guard: &mut tokio::sync::MutexGuard<'_, Option<BrowserSession>>,
    ) -> Result<()> {
        let needs_launch = match guard.as_ref() {
            None => true,
            Some(s) => s.is_idle(),
        };
        if needs_launch {
            if let Some(mut old) = guard.take() {
                old.shutdown().await;
            }
            **guard = Some(BrowserSession::launch().await?);
        }
        Ok(())
    }
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Interact with web pages using a headless browser. Supports navigation, \
         text/HTML extraction, clicking, typing, screenshots, and JS evaluation."
    }

    fn tags(&self) -> &[&str] {
        &["web", "interactive"]
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "get_text", "get_html", "click", "type", "screenshot", "evaluate", "close"],
                    "description": "Action to perform"
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to (for 'navigate' action)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector (for 'click' and 'type' actions)"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type (for 'type' action)"
                },
                "expression": {
                    "type": "string",
                    "description": "JavaScript expression (for 'evaluate' action)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: Input =
            serde_json::from_value(args.clone()).wrap_err("invalid browser tool input")?;

        // Validate action before launching Chrome
        const VALID_ACTIONS: &[&str] = &[
            "navigate", "get_text", "get_html", "click", "type", "screenshot", "evaluate", "close",
        ];
        if !VALID_ACTIONS.contains(&input.action.as_str()) {
            return Ok(ToolResult {
                output: format!(
                    "Unknown action: {}. Valid: {}",
                    input.action,
                    VALID_ACTIONS.join(", ")
                ),
                success: false,
                ..Default::default()
            });
        }

        // Close action doesn't need a session
        if input.action == "close" {
            let mut guard = self.session.lock().await;
            if let Some(mut session) = guard.take() {
                session.shutdown().await;
            }
            return Ok(ToolResult {
                output: "Browser session closed".to_string(),
                success: true,
                ..Default::default()
            });
        }

        let mut guard = self.session.lock().await;
        if let Err(e) = Self::ensure_session(&mut guard).await {
            return Ok(ToolResult {
                output: format!("Failed to launch browser: {e}"),
                success: false,
                ..Default::default()
            });
        }
        let session = guard.as_mut().unwrap();
        session.touch();

        match input.action.as_str() {
            "navigate" => {
                let url = match &input.url {
                    Some(u) => u,
                    None => {
                        return Ok(ToolResult {
                            output: "'url' parameter is required for navigate".to_string(),
                            success: false,
                            ..Default::default()
                        });
                    }
                };
                // Reject non-http(s) schemes (file://, data:, javascript:, etc.)
                let parsed_url = match reqwest::Url::parse(url) {
                    Ok(u) => u,
                    Err(_) => {
                        return Ok(ToolResult {
                            output: "Invalid URL".to_string(),
                            success: false,
                            ..Default::default()
                        });
                    }
                };
                let scheme = parsed_url.scheme();
                if scheme != "http" && scheme != "https" {
                    return Ok(ToolResult {
                        output: format!("Only http:// and https:// URLs are allowed, got {scheme}://"),
                        success: false,
                        ..Default::default()
                    });
                }
                if let Some(msg) = check_ssrf(url).await {
                    return Ok(ToolResult {
                        output: msg,
                        success: false,
                        ..Default::default()
                    });
                }
                session
                    .client
                    .send("Page.navigate", json!({ "url": url }))
                    .await?;
                // Wait for page load (best-effort, 30s timeout)
                let _ = session
                    .client
                    .wait_event("Page.loadEventFired", ACTION_TIMEOUT)
                    .await;
                Ok(ToolResult {
                    output: format!("Navigated to {url}"),
                    success: true,
                    ..Default::default()
                })
            }
            "get_text" => {
                let result = session
                    .client
                    .send(
                        "Runtime.evaluate",
                        json!({ "expression": "document.body.innerText", "returnByValue": true, "timeout": 10000 }),
                    )
                    .await?;
                let mut text = result
                    .pointer("/result/value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                crew_core::truncate_utf8(&mut text, MAX_OUTPUT_CHARS, "\n\n... (truncated)");
                Ok(ToolResult {
                    output: text,
                    success: true,
                    ..Default::default()
                })
            }
            "get_html" => {
                let result = session
                    .client
                    .send(
                        "Runtime.evaluate",
                        json!({ "expression": "document.documentElement.outerHTML", "returnByValue": true, "timeout": 10000 }),
                    )
                    .await?;
                let mut html = result
                    .pointer("/result/value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                crew_core::truncate_utf8(&mut html, MAX_OUTPUT_CHARS, "\n\n... (truncated)");
                Ok(ToolResult {
                    output: html,
                    success: true,
                    ..Default::default()
                })
            }
            "click" => {
                let selector = match &input.selector {
                    Some(s) => s,
                    None => {
                        return Ok(ToolResult {
                            output: "'selector' parameter is required for click".to_string(),
                            success: false,
                            ..Default::default()
                        });
                    }
                };
                let js = format!(
                    r#"(() => {{ const el = document.querySelector({sel}); if (!el) return 'ERROR: element not found'; el.click(); return 'clicked'; }})()"#,
                    sel = serde_json::to_string(selector)?
                );
                let result = session
                    .client
                    .send(
                        "Runtime.evaluate",
                        json!({ "expression": js, "returnByValue": true, "timeout": 10000 }),
                    )
                    .await?;
                let val = result
                    .pointer("/result/value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("done");
                let success = !val.starts_with("ERROR:");
                Ok(ToolResult {
                    output: val.to_string(),
                    success,
                    ..Default::default()
                })
            }
            "type" => {
                let selector = match &input.selector {
                    Some(s) => s,
                    None => {
                        return Ok(ToolResult {
                            output: "'selector' parameter is required for type".to_string(),
                            success: false,
                            ..Default::default()
                        });
                    }
                };
                let text = match &input.text {
                    Some(t) => t,
                    None => {
                        return Ok(ToolResult {
                            output: "'text' parameter is required for type".to_string(),
                            success: false,
                            ..Default::default()
                        });
                    }
                };
                let js = format!(
                    r#"(() => {{ const el = document.querySelector({sel}); if (!el) return 'ERROR: element not found'; el.focus(); el.value = {val}; el.dispatchEvent(new Event('input', {{bubbles: true}})); return 'typed'; }})()"#,
                    sel = serde_json::to_string(selector)?,
                    val = serde_json::to_string(text)?
                );
                let result = session
                    .client
                    .send(
                        "Runtime.evaluate",
                        json!({ "expression": js, "returnByValue": true, "timeout": 10000 }),
                    )
                    .await?;
                let val = result
                    .pointer("/result/value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("done");
                let success = !val.starts_with("ERROR:");
                Ok(ToolResult {
                    output: val.to_string(),
                    success,
                    ..Default::default()
                })
            }
            "screenshot" => {
                let result = session
                    .client
                    .send("Page.captureScreenshot", json!({ "format": "png" }))
                    .await?;
                let data_b64 = result
                    .get("data")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if data_b64.is_empty() {
                    return Ok(ToolResult {
                        output: "Failed to capture screenshot".to_string(),
                        success: false,
                        ..Default::default()
                    });
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data_b64)
                    .wrap_err("invalid base64 from screenshot")?;
                let tmp = tempfile::Builder::new()
                    .prefix("crew-screenshot-")
                    .suffix(".png")
                    .tempfile()
                    .wrap_err("failed to create screenshot temp file")?;
                let path = tmp.path().to_path_buf();
                tokio::fs::write(&path, &bytes).await.wrap_err("failed to write screenshot")?;
                // Keep the file on disk (don't auto-delete on drop)
                tmp.keep().map_err(|e| eyre::eyre!("failed to persist screenshot: {}", e.error))?;
                Ok(ToolResult {
                    output: format!("Screenshot saved to {}", path.display()),
                    success: true,
                    ..Default::default()
                })
            }
            "evaluate" => {
                let expr = match &input.expression {
                    Some(e) => e,
                    None => {
                        return Ok(ToolResult {
                            output: "'expression' parameter is required for evaluate".to_string(),
                            success: false,
                            ..Default::default()
                        });
                    }
                };
                let result = session
                    .client
                    .send(
                        "Runtime.evaluate",
                        json!({ "expression": expr, "returnByValue": true, "timeout": 10000 }),
                    )
                    .await?;
                let exception = result.get("exceptionDetails");
                if exception.is_some() {
                    let msg = result
                        .pointer("/exceptionDetails/exception/description")
                        .or_else(|| result.pointer("/exceptionDetails/text"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("JavaScript exception");
                    return Ok(ToolResult {
                        output: msg.to_string(),
                        success: false,
                        ..Default::default()
                    });
                }
                let mut output = match result.pointer("/result/value") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => {
                        // No value returned (undefined, etc.)
                        result
                            .pointer("/result/description")
                            .or_else(|| result.pointer("/result/type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("undefined")
                            .to_string()
                    }
                };
                crew_core::truncate_utf8(&mut output, MAX_OUTPUT_CHARS, "\n\n... (truncated)");
                Ok(ToolResult {
                    output,
                    success: true,
                    ..Default::default()
                })
            }
            // VALID_ACTIONS check above ensures we never reach here
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_chrome_binary() {
        // Just verify it doesn't panic; may or may not find Chrome in CI
        let _ = find_chrome_binary();
    }

    #[test]
    fn test_ssrf_private_hosts() {
        assert!(is_private_host("localhost"));
        assert!(is_private_host("127.0.0.1"));
        assert!(is_private_host("10.0.0.1"));
        assert!(is_private_host("192.168.1.1"));
        assert!(is_private_host("::1"));
        assert!(!is_private_host("example.com"));
        assert!(!is_private_host("8.8.8.8"));
    }

    #[test]
    fn test_input_deserialization() {
        let v = json!({ "action": "navigate", "url": "https://example.com" });
        let input: Input = serde_json::from_value(v).unwrap();
        assert_eq!(input.action, "navigate");
        assert_eq!(input.url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn test_input_minimal() {
        let v = json!({ "action": "get_text" });
        let input: Input = serde_json::from_value(v).unwrap();
        assert_eq!(input.action, "get_text");
        assert!(input.url.is_none());
        assert!(input.selector.is_none());
    }

    #[tokio::test]
    async fn test_close_without_session() {
        let tool = BrowserTool::new();
        let result = tool
            .execute(&json!({ "action": "close" }))
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_navigate_ssrf_blocked() {
        let tool = BrowserTool::new();
        // This should fail at SSRF check before needing Chrome
        let result = tool
            .execute(&json!({ "action": "navigate", "url": "http://127.0.0.1:9222" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("private"));
    }

    #[tokio::test]
    async fn test_navigate_missing_url() {
        // This will try to launch Chrome which may fail in CI, but we test the param validation
        let tool = BrowserTool::new();
        let result = tool
            .execute(&json!({ "action": "navigate" }))
            .await
            .unwrap();
        // Either fails at missing URL or at Chrome launch - both are acceptable
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_navigate_rejects_file_scheme() {
        let tool = BrowserTool::new();
        let result = tool
            .execute(&json!({ "action": "navigate", "url": "file:///etc/passwd" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Only http://"));
    }

    #[tokio::test]
    async fn test_navigate_rejects_javascript_scheme() {
        let tool = BrowserTool::new();
        let result = tool
            .execute(&json!({ "action": "navigate", "url": "javascript:alert(1)" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Only http://"));
    }

    #[tokio::test]
    async fn test_unknown_action_rejected_early() {
        let tool = BrowserTool::new();
        let result = tool
            .execute(&json!({ "action": "invalid_action" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("Unknown action"));
    }

    #[test]
    fn test_unknown_action_deserializes() {
        let v = json!({ "action": "invalid_action" });
        let input: Input = serde_json::from_value(v).unwrap();
        assert_eq!(input.action, "invalid_action");
    }
}
