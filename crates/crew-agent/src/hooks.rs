//! Hook/lifecycle system for running shell commands at agent lifecycle points.
//!
//! Supports 4 events: before/after tool call and before/after LLM call.
//! Before-hooks can deny operations (exit code 1). Circuit breaker auto-disables
//! hooks after consecutive failures.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::sandbox::BLOCKED_ENV_VARS;

/// Lifecycle events that can trigger hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    BeforeToolCall,
    AfterToolCall,
    BeforeLlmCall,
    AfterLlmCall,
}

/// Configuration for a single hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookConfig {
    /// Which lifecycle event triggers this hook.
    pub event: HookEvent,
    /// Command as argv array (no shell interpretation).
    pub command: Vec<String>,
    /// Timeout in milliseconds (default 5000).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Only trigger for these tool names (tool events only). Empty = all tools.
    #[serde(default)]
    pub tool_filter: Vec<String>,
}

fn default_timeout_ms() -> u64 {
    5000
}

/// Payload sent to hook process as JSON on stdin.
#[derive(Debug, Clone, Serialize)]
pub struct HookPayload {
    pub event: HookEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
}

/// Result of running hooks for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookResult {
    /// All hooks passed (or no hooks matched).
    Allow,
    /// A before-hook denied the operation.
    Deny(String),
    /// A hook encountered an error (does not block).
    Error(String),
}

/// Executes hooks with circuit breaker protection.
pub struct HookExecutor {
    hooks: Vec<HookConfig>,
    /// Per-hook consecutive failure count.
    failures: Vec<AtomicU32>,
    failure_threshold: u32,
}

impl HookExecutor {
    pub fn new(hooks: Vec<HookConfig>) -> Self {
        let failures = (0..hooks.len()).map(|_| AtomicU32::new(0)).collect();
        Self {
            hooks,
            failures,
            failure_threshold: 3,
        }
    }

    /// Run all matching hooks for the given event sequentially.
    /// Returns `Deny` on the first before-hook that exits with 1.
    pub async fn run(&self, event: HookEvent, payload: &HookPayload) -> HookResult {
        let payload_json = match serde_json::to_string(payload) {
            Ok(j) => j,
            Err(e) => return HookResult::Error(format!("failed to serialize payload: {e}")),
        };

        let mut last_error = None;

        for (i, hook) in self.hooks.iter().enumerate() {
            if hook.event != event {
                continue;
            }

            // Apply tool_filter for tool events
            if matches!(event, HookEvent::BeforeToolCall | HookEvent::AfterToolCall)
                && !hook.tool_filter.is_empty()
            {
                let tool_name = payload.tool_name.as_deref().unwrap_or("");
                if !hook.tool_filter.iter().any(|f| f == tool_name) {
                    continue;
                }
            }

            // Circuit breaker: skip if too many failures
            let fail_count = self.failures[i].load(Ordering::Relaxed);
            if fail_count >= self.failure_threshold {
                if fail_count == self.failure_threshold {
                    warn!(
                        hook_command = ?hook.command,
                        "hook disabled after {} consecutive failures",
                        self.failure_threshold
                    );
                    // Increment past threshold so warning only fires once
                    self.failures[i].store(self.failure_threshold + 1, Ordering::Relaxed);
                }
                continue;
            }

            match self.execute_hook(hook, &payload_json).await {
                Ok((0, _stdout)) => {
                    self.failures[i].store(0, Ordering::Relaxed);
                }
                Ok((1, stdout)) => {
                    self.failures[i].store(0, Ordering::Relaxed);
                    if matches!(event, HookEvent::BeforeToolCall | HookEvent::BeforeLlmCall) {
                        return HookResult::Deny(stdout);
                    }
                }
                Ok((code, _stdout)) => {
                    let new_count = self.failures[i].fetch_add(1, Ordering::Relaxed) + 1;
                    let msg = format!(
                        "hook {:?} exited with code {} ({}/{})",
                        hook.command, code, new_count, self.failure_threshold
                    );
                    warn!("{}", msg);
                    last_error = Some(msg);
                }
                Err(e) => {
                    let new_count = self.failures[i].fetch_add(1, Ordering::Relaxed) + 1;
                    let msg = format!(
                        "hook {:?} failed: {} ({}/{})",
                        hook.command, e, new_count, self.failure_threshold
                    );
                    warn!("{}", msg);
                    last_error = Some(msg);
                }
            }
        }

        if let Some(err) = last_error {
            HookResult::Error(err)
        } else {
            HookResult::Allow
        }
    }

    /// Execute a single hook process. Returns (exit_code, stdout).
    async fn execute_hook(
        &self,
        hook: &HookConfig,
        payload_json: &str,
    ) -> eyre::Result<(i32, String)> {
        let (program, args) = hook
            .command
            .split_first()
            .ok_or_else(|| eyre::eyre!("empty hook command"))?;

        // Expand ~ to home directory
        let program = expand_tilde(program);

        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());

        // Sanitize environment
        for var in BLOCKED_ENV_VARS {
            cmd.env_remove(var);
        }

        let mut child = cmd.spawn()?;

        // Write payload to stdin
        if let Some(mut stdin) = child.stdin.take() {
            let payload = payload_json.to_string();
            tokio::spawn(async move {
                let _ = stdin.write_all(payload.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }

        // Wait with timeout
        let timeout = Duration::from_millis(hook.timeout_ms);
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let code = output.status.code().unwrap_or(2);
                Ok((code, stdout))
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => {
                // Timeout: child was consumed by wait_with_output future which was dropped,
                // so the process will be cleaned up when the future is dropped.
                Err(eyre::eyre!("hook timed out after {}ms", hook.timeout_ms))
            }
        }
    }
}

/// Expand leading `~` or `~/` to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), &path[1..]);
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_config_deserialize() {
        let json = r#"{
            "event": "before_tool_call",
            "command": ["python3", "~/.crew/hooks/audit.py"],
            "timeout_ms": 3000,
            "tool_filter": ["shell", "write_file"]
        }"#;
        let hook: HookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(hook.event, HookEvent::BeforeToolCall);
        assert_eq!(hook.command, vec!["python3", "~/.crew/hooks/audit.py"]);
        assert_eq!(hook.timeout_ms, 3000);
        assert_eq!(hook.tool_filter, vec!["shell", "write_file"]);
    }

    #[test]
    fn test_hook_config_defaults() {
        let json = r#"{
            "event": "after_llm_call",
            "command": ["echo", "done"]
        }"#;
        let hook: HookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(hook.timeout_ms, 5000);
        assert!(hook.tool_filter.is_empty());
    }

    #[test]
    fn test_payload_serialization() {
        let payload = HookPayload {
            event: HookEvent::BeforeToolCall,
            tool_name: Some("shell".into()),
            arguments: Some(serde_json::json!({"command": "ls"})),
            tool_id: Some("call_1".into()),
            result: None,
            success: None,
            duration_ms: None,
            message_count: None,
            model: None,
            iteration: Some(3),
            stop_reason: None,
            has_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"event\":\"before_tool_call\""));
        assert!(json.contains("\"tool_name\":\"shell\""));
        assert!(json.contains("\"iteration\":3"));
        assert!(!json.contains("\"result\""));
        assert!(!json.contains("\"success\""));
    }

    #[test]
    fn test_circuit_breaker_tracking() {
        let executor = HookExecutor::new(vec![HookConfig {
            event: HookEvent::AfterToolCall,
            command: vec!["true".into()],
            timeout_ms: 1000,
            tool_filter: vec![],
        }]);
        executor.failures[0].store(3, Ordering::Relaxed);
        assert!(executor.failures[0].load(Ordering::Relaxed) >= executor.failure_threshold);
    }

    #[test]
    fn test_tool_filter_matching() {
        let hook = HookConfig {
            event: HookEvent::BeforeToolCall,
            command: vec!["check".into()],
            timeout_ms: 1000,
            tool_filter: vec!["shell".into(), "write_file".into()],
        };
        assert!(hook.tool_filter.iter().any(|f| f == "shell"));
        assert!(!hook.tool_filter.iter().any(|f| f == "read_file"));
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/foo/bar");
        assert!(!expanded.starts_with('~'));
        assert!(expanded.ends_with("/foo/bar"));

        // Non-tilde paths unchanged
        assert_eq!(expand_tilde("/usr/bin/foo"), "/usr/bin/foo");
        assert_eq!(expand_tilde("relative/path"), "relative/path");
    }

    #[tokio::test]
    async fn test_executor_no_hooks() {
        let executor = HookExecutor::new(vec![]);
        let payload = HookPayload {
            event: HookEvent::BeforeToolCall,
            tool_name: Some("shell".into()),
            arguments: None,
            tool_id: None,
            result: None,
            success: None,
            duration_ms: None,
            message_count: None,
            model: None,
            iteration: None,
            stop_reason: None,
            has_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
        };
        let result = executor.run(HookEvent::BeforeToolCall, &payload).await;
        assert_eq!(result, HookResult::Allow);
    }

    #[tokio::test]
    async fn test_executor_allow_hook() {
        let executor = HookExecutor::new(vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            command: vec!["true".into()],
            timeout_ms: 5000,
            tool_filter: vec![],
        }]);
        let payload = HookPayload {
            event: HookEvent::BeforeToolCall,
            tool_name: Some("shell".into()),
            arguments: None,
            tool_id: None,
            result: None,
            success: None,
            duration_ms: None,
            message_count: None,
            model: None,
            iteration: None,
            stop_reason: None,
            has_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
        };
        let result = executor.run(HookEvent::BeforeToolCall, &payload).await;
        assert_eq!(result, HookResult::Allow);
    }

    #[tokio::test]
    async fn test_executor_deny_hook() {
        // `false` exits with code 1
        let executor = HookExecutor::new(vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            command: vec!["false".into()],
            timeout_ms: 5000,
            tool_filter: vec![],
        }]);
        let payload = HookPayload {
            event: HookEvent::BeforeToolCall,
            tool_name: Some("shell".into()),
            arguments: None,
            tool_id: None,
            result: None,
            success: None,
            duration_ms: None,
            message_count: None,
            model: None,
            iteration: None,
            stop_reason: None,
            has_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
        };
        let result = executor.run(HookEvent::BeforeToolCall, &payload).await;
        assert!(matches!(result, HookResult::Deny(_)));
    }

    #[tokio::test]
    async fn test_executor_tool_filter_skips() {
        let executor = HookExecutor::new(vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            command: vec!["false".into()],
            timeout_ms: 5000,
            tool_filter: vec!["write_file".into()],
        }]);
        let payload = HookPayload {
            event: HookEvent::BeforeToolCall,
            tool_name: Some("read_file".into()),
            arguments: None,
            tool_id: None,
            result: None,
            success: None,
            duration_ms: None,
            message_count: None,
            model: None,
            iteration: None,
            stop_reason: None,
            has_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
        };
        let result = executor.run(HookEvent::BeforeToolCall, &payload).await;
        assert_eq!(result, HookResult::Allow);
    }

    #[tokio::test]
    async fn test_executor_event_mismatch_skips() {
        let executor = HookExecutor::new(vec![HookConfig {
            event: HookEvent::AfterToolCall,
            command: vec!["false".into()],
            timeout_ms: 5000,
            tool_filter: vec![],
        }]);
        let payload = HookPayload {
            event: HookEvent::BeforeToolCall,
            tool_name: Some("shell".into()),
            arguments: None,
            tool_id: None,
            result: None,
            success: None,
            duration_ms: None,
            message_count: None,
            model: None,
            iteration: None,
            stop_reason: None,
            has_tool_calls: None,
            input_tokens: None,
            output_tokens: None,
        };
        let result = executor.run(HookEvent::BeforeToolCall, &payload).await;
        assert_eq!(result, HookResult::Allow);
    }
}
