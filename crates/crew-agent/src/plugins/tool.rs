//! Plugin tool: wraps a plugin executable as a Tool.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use eyre::{Result, WrapErr};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::tools::{Tool, ToolResult};

use super::manifest::PluginToolDef;

const PLUGIN_TIMEOUT: Duration = Duration::from_secs(30);

/// A tool backed by a plugin executable.
///
/// Protocol: write JSON args to stdin, read JSON result from stdout.
/// Expected output: `{ "output": "...", "success": true/false }`
pub struct PluginTool {
    plugin_name: String,
    tool_def: PluginToolDef,
    executable: PathBuf,
}

impl PluginTool {
    pub fn new(plugin_name: String, tool_def: PluginToolDef, executable: PathBuf) -> Self {
        Self {
            plugin_name,
            tool_def,
            executable,
        }
    }
}

#[async_trait]
impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.tool_def.name
    }

    fn description(&self) -> &str {
        &self.tool_def.description
    }

    fn input_schema(&self) -> serde_json::Value {
        self.tool_def.input_schema.clone()
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let mut child = Command::new(&self.executable)
            .arg(&self.tool_def.name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .wrap_err_with(|| {
                format!(
                    "failed to spawn plugin '{}' executable: {}",
                    self.plugin_name,
                    self.executable.display()
                )
            })?;

        // Write args to stdin
        if let Some(mut stdin) = child.stdin.take() {
            let data = serde_json::to_vec(args)?;
            stdin.write_all(&data).await?;
            // Drop stdin to signal EOF
        }

        let result = tokio::time::timeout(PLUGIN_TIMEOUT, child.wait_with_output())
            .await
            .wrap_err_with(|| {
                format!(
                    "plugin '{}' tool '{}' timed out after {}s",
                    self.plugin_name,
                    self.tool_def.name,
                    PLUGIN_TIMEOUT.as_secs()
                )
            })?
            .wrap_err_with(|| {
                format!(
                    "plugin '{}' tool '{}' execution failed",
                    self.plugin_name, self.tool_def.name
                )
            })?;

        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);

        // Try to parse structured output
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let output = parsed
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or(&stdout)
                .to_string();
            let success = parsed
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(result.status.success());
            return Ok(ToolResult {
                output,
                success,
                ..Default::default()
            });
        }

        // Fallback: raw stdout + stderr
        let mut output = stdout.to_string();
        if !stderr.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&stderr);
        }

        Ok(ToolResult {
            output,
            success: result.status.success(),
            ..Default::default()
        })
    }
}
