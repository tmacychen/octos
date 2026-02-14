//! Write file tool for creating new files.

use std::path::PathBuf;

use async_trait::async_trait;
use eyre::{Result, WrapErr};
use serde::Deserialize;

use super::{Tool, ToolResult};

/// Tool for writing/creating files.
pub struct WriteFileTool {
    /// Base directory for resolving relative paths.
    base_dir: PathBuf,
}

impl WriteFileTool {
    /// Create a new write file tool.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WriteFileInput {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, or overwrites if it does."
    }

    fn tags(&self) -> &[&str] {
        &["fs", "code"]
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: WriteFileInput =
            serde_json::from_value(args.clone()).wrap_err("invalid write_file tool input")?;

        // Resolve path (with traversal protection)
        let path = match super::resolve_path(&self.base_dir, &input.path) {
            Ok(p) => p,
            Err(_) => {
                return Ok(ToolResult {
                    output: format!("Path outside working directory: {}", input.path),
                    success: false,
                    ..Default::default()
                });
            }
        };

        if let Some(r) = super::reject_symlink(&path).await {
            return Ok(r);
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .wrap_err_with(|| format!("failed to create directories: {}", parent.display()))?;
        }

        // Write file
        tokio::fs::write(&path, &input.content)
            .await
            .wrap_err_with(|| format!("failed to write file: {}", path.display()))?;

        let line_count = input.content.lines().count();
        Ok(ToolResult {
            output: format!("Successfully wrote {} lines to {}", line_count, input.path),
            success: true,
            file_modified: Some(path),
            ..Default::default()
        })
    }
}
