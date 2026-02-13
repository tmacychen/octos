//! Vision support: base64 image encoding for LLM providers.

use std::path::Path;

use base64::Engine;
use eyre::{Result, WrapErr};

/// Encode an image file as base64 and return (mime_type, base64_data).
pub fn encode_image(path: &str) -> Result<(String, String)> {
    let bytes = std::fs::read(path).wrap_err_with(|| format!("failed to read image: {path}"))?;

    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg")
        .to_lowercase();

    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/jpeg",
    };

    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((mime.to_string(), encoded))
}

/// Check if a file path looks like an image file.
pub fn is_image(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
}
