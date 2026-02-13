//! Tool output sanitization.
//!
//! Strips base64 data URIs and long hex strings from tool results
//! before feeding them back to the LLM, reducing context waste
//! and preventing accidental secret leakage.

use std::sync::LazyLock;

use regex::Regex;

/// Base64 data URIs: `data:...;base64,<payload>`
static DATA_URI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"data:[^;]{1,100};base64,[A-Za-z0-9+/=]{64,}").unwrap());

/// Long hex strings (64+ contiguous hex chars, e.g. SHA-256, SHA-512, raw keys).
static HEX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9a-fA-F]{64,}").unwrap());

/// Sanitize tool output by stripping base64 data URIs and long hex strings.
pub fn sanitize_tool_output(input: &str) -> String {
    let after_data_uri = DATA_URI_RE.replace_all(input, "[base64-data-redacted]");
    let after_hex = HEX_RE.replace_all(&after_data_uri, "[hex-redacted]");
    after_hex.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strips_data_uri() {
        let input = "img: data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg== done";
        let result = sanitize_tool_output(input);
        assert_eq!(result, "img: [base64-data-redacted] done");
    }

    #[test]
    fn test_strips_long_hex() {
        let hex = "a".repeat(64);
        let input = format!("key: {} end", hex);
        let result = sanitize_tool_output(&input);
        assert_eq!(result, "key: [hex-redacted] end");
    }

    #[test]
    fn test_preserves_short_hex() {
        let input = "commit abc123def456 is good";
        let result = sanitize_tool_output(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_preserves_normal_text() {
        let input = "Hello world, this is normal output.";
        let result = sanitize_tool_output(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_multiple_replacements() {
        let hex = "f".repeat(64);
        let input = format!("data:text/plain;base64,{} and {} end", "A".repeat(100), hex);
        let result = sanitize_tool_output(&input);
        assert!(result.contains("[base64-data-redacted]"));
        assert!(result.contains("[hex-redacted]"));
        assert!(!result.contains(&hex));
    }
}
