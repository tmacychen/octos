//! Save memory tool: write/update entity pages in the memory bank.

use std::sync::Arc;

use async_trait::async_trait;
use crew_memory::MemoryStore;
use eyre::{Result, WrapErr};
use serde::Deserialize;

use super::{Tool, ToolResult};

/// Tool that saves or updates entity pages in the memory bank.
pub struct SaveMemoryTool {
    store: Arc<MemoryStore>,
}

impl SaveMemoryTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct Input {
    name: String,
    content: String,
}

#[async_trait]
impl Tool for SaveMemoryTool {
    fn name(&self) -> &str {
        "save_memory"
    }

    fn description(&self) -> &str {
        "Save or update a memory bank entity. Always start content with a heading \
         (# Name) followed by a one-line plain-text summary — this becomes the abstract \
         shown in the memory bank index. Example: '# Daniu\\nUBC CS student in Vancouver, \
         likes music and movies.\\n\\n## Details\\n...'. \
         IMPORTANT: When updating an existing entity, first use `recall_memory` to \
         load the current content, then MERGE new information into it before saving. \
         Never discard existing facts — add to or update them."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Entity name slug (e.g. 'crew-rs', 'yuechen')"
                },
                "content": {
                    "type": "string",
                    "description": "Full markdown content for the entity page"
                }
            },
            "required": ["name", "content"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: Input =
            serde_json::from_value(args.clone()).wrap_err("invalid save_memory input")?;

        let slug = input.name.trim().to_lowercase().replace(' ', "-");

        if slug.is_empty()
            || !slug
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Ok(ToolResult {
                output: format!(
                    "Invalid entity name: '{slug}'. Use alphanumeric chars and hyphens."
                ),
                success: false,
                ..Default::default()
            });
        }

        // Read existing content before overwriting, so we can warn about lost info
        let existing = self.store.read_entity(&slug).await.unwrap_or(None);

        self.store.write_entity(&slug, &input.content).await?;

        let output = if let Some(old) = existing {
            format!(
                "Memory entity '{slug}' updated. Previous content was:\n\
                 ---\n{old}\n---\n\
                 If important information was lost, save again with merged content."
            )
        } else {
            format!("Memory entity '{slug}' created.")
        };

        Ok(ToolResult {
            output,
            success: true,
            ..Default::default()
        })
    }
}
