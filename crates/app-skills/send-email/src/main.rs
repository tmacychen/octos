use std::io::Read;
use std::process;

use serde::Deserialize;
use serde_json::json;

// ---------------------------------------------------------------------------
// Input schema
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Input {
    to: String,
    subject: String,
    body: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    html: Option<bool>,
}

// ---------------------------------------------------------------------------
// SMTP sender (lettre, blocking)
// ---------------------------------------------------------------------------

fn send_smtp(input: &Input) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::message::{MultiPart, SinglePart};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};

    let host = env_required("SMTP_HOST")?;
    let port: u16 = env_required("SMTP_PORT")?
        .parse()
        .map_err(|e| format!("SMTP_PORT is not a valid port number: {e}"))?;
    let username = env_required("SMTP_USERNAME")?;
    let password = env_required("SMTP_PASSWORD")?;
    let from_address = env_required("SMTP_FROM")?;

    let from = from_address
        .parse()
        .map_err(|e| format!("invalid SMTP_FROM address: {e}"))?;
    let to_mailbox = input
        .to
        .parse()
        .map_err(|e| format!("invalid 'to' address '{}': {e}", input.to))?;

    let builder = Message::builder()
        .from(from)
        .to(to_mailbox)
        .subject(&input.subject);

    let is_html = input.html.unwrap_or(false);

    let message = if is_html {
        builder
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(strip_html_tags(&input.body)),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(input.body.clone()),
                    ),
            )
            .map_err(|e| format!("failed to build email: {e}"))?
    } else {
        builder
            .header(ContentType::TEXT_PLAIN)
            .body(input.body.clone())
            .map_err(|e| format!("failed to build email: {e}"))?
    };

    let creds = Credentials::new(username, password);

    let mailer = if port == 465 {
        SmtpTransport::relay(&host)
            .map_err(|e| format!("SMTP relay error: {e}"))?
            .credentials(creds)
            .port(port)
            .build()
    } else {
        SmtpTransport::starttls_relay(&host)
            .map_err(|e| format!("SMTP STARTTLS error: {e}"))?
            .credentials(creds)
            .port(port)
            .build()
    };

    mailer
        .send(&message)
        .map_err(|e| format!("failed to send email: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Feishu/Lark Mail sender (reqwest blocking)
// ---------------------------------------------------------------------------

fn send_feishu(input: &Input) -> Result<(), String> {
    let app_id = env_required("LARK_APP_ID")?;
    let app_secret = env_required("LARK_APP_SECRET")?;
    let from_address = env_required("LARK_FROM_ADDRESS")?;

    let region = std::env::var("LARK_REGION").unwrap_or_default();
    let base_url = match region.as_str() {
        "global" | "lark" => "https://open.larksuite.com/open-apis",
        _ => "https://open.feishu.cn/open-apis",
    };

    let client = reqwest::blocking::Client::new();

    // Step 1: Get tenant access token
    let token_resp: serde_json::Value = client
        .post(format!("{base_url}/auth/v3/tenant_access_token/internal"))
        .json(&json!({
            "app_id": app_id,
            "app_secret": app_secret,
        }))
        .send()
        .map_err(|e| format!("failed to request Feishu tenant token: {e}"))?
        .json()
        .map_err(|e| format!("failed to parse Feishu token response: {e}"))?;

    let token = token_resp
        .get("tenant_access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            let msg = token_resp
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Feishu token error: {msg}")
        })?
        .to_string();

    // Step 2: Build email body
    let is_html = input.html.unwrap_or(false);
    let content = if is_html {
        input.body.clone()
    } else {
        format!("<p>{}</p>", html_escape(&input.body))
    };

    let email_body = json!({
        "subject": input.subject,
        "to": [{"mail_address": input.to}],
        "cc": [],
        "bcc": [],
        "body": {
            "content": content
        },
        "head_from": {
            "mail_address": from_address
        }
    });

    // Step 3: Send email
    let url = format!("{base_url}/mail/v1/user_mailboxes/{from_address}/messages/send");

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&email_body)
        .send()
        .map_err(|e| format!("Feishu mail API request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!("Feishu mail API error (HTTP {status}): {text}"));
    }

    let result: serde_json::Value = resp.json().unwrap_or_default();
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = result
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(format!("Feishu mail API error (code {code}): {msg}"));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn env_required(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("environment variable '{name}' is not set"))
}

/// Minimal HTML escaping for plain text embedded in Feishu mail body.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Very basic HTML tag stripping for the plain-text fallback part.
fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    result
}

/// Detect provider from input or environment variables.
fn detect_provider(input: &Input) -> Result<String, String> {
    if let Some(ref p) = input.provider {
        let lower = p.to_lowercase();
        match lower.as_str() {
            "smtp" | "feishu" | "lark" => Ok(if lower == "lark" {
                "feishu".to_string()
            } else {
                lower
            }),
            _ => Err(format!(
                "unknown provider '{p}'. Supported: \"smtp\", \"feishu\""
            )),
        }
    } else if std::env::var("SMTP_HOST").is_ok() {
        Ok("smtp".to_string())
    } else if std::env::var("LARK_APP_ID").is_ok() {
        Ok("feishu".to_string())
    } else {
        Err(
            "no provider specified and cannot auto-detect: set SMTP_HOST (for SMTP) or LARK_APP_ID (for Feishu) environment variables"
                .to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    // Read JSON input from stdin
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        let output = json!({
            "output": format!("Failed to read stdin: {e}"),
            "success": false
        });
        println!("{}", output);
        process::exit(1);
    }

    let input: Input = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(e) => {
            let output = json!({
                "output": format!("Invalid JSON input: {e}"),
                "success": false
            });
            println!("{}", output);
            process::exit(1);
        }
    };

    // Validate recipient
    if input.to.trim().is_empty() {
        let output = json!({
            "output": "Error: 'to' must contain a non-empty email address.",
            "success": false
        });
        println!("{}", output);
        process::exit(1);
    }

    // Detect provider
    let provider = match detect_provider(&input) {
        Ok(p) => p,
        Err(e) => {
            let output = json!({
                "output": format!("Provider detection failed: {e}"),
                "success": false
            });
            println!("{}", output);
            process::exit(1);
        }
    };

    // Send email
    let result = match provider.as_str() {
        "smtp" => send_smtp(&input),
        "feishu" => send_feishu(&input),
        _ => unreachable!(),
    };

    match result {
        Ok(()) => {
            let output = json!({
                "output": format!("Email sent to {}", input.to),
                "success": true
            });
            println!("{}", output);
        }
        Err(e) => {
            let output = json!({
                "output": format!("Failed to send email: {e}"),
                "success": false
            });
            println!("{}", output);
            process::exit(1);
        }
    }
}
