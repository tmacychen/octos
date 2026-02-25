//! Recall memory tool: load full entity pages from the memory bank.

use std::sync::Arc;

use async_trait::async_trait;
use crew_memory::MemoryStore;
use eyre::{Result, WrapErr};
use serde::Deserialize;

use super::{Tool, ToolResult};

/// Tool that loads full entity pages from the memory bank.
pub struct RecallMemoryTool {
    store: Arc<MemoryStore>,
}

impl RecallMemoryTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct Input {
    name: String,
}

#[async_trait]
impl Tool for RecallMemoryTool {
    fn name(&self) -> &str {
        "recall_memory"
    }

    fn description(&self) -> &str {
        "Load the full content of a memory bank entity by name. \
         Use the entity names shown in the Memory Bank section of the system prompt."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Entity name (e.g. 'crew-rs', 'yuechen')"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<ToolResult> {
        let input: Input =
            serde_json::from_value(args.clone()).wrap_err("invalid recall_memory input")?;

        let slug = input.name.trim().to_lowercase().replace(' ', "-");

        match self.store.read_entity(&slug).await? {
            Some(content) => Ok(ToolResult {
                output: content,
                success: true,
                ..Default::default()
            }),
            None => {
                let entities = self.store.list_entities().await.unwrap_or_default();
                let available: Vec<_> = entities.iter().map(|(n, _)| n.as_str()).collect();
                Ok(ToolResult {
                    output: format!(
                        "Entity '{}' not found. Available: {}",
                        slug,
                        if available.is_empty() {
                            "(none)".to_string()
                        } else {
                            available.join(", ")
                        }
                    ),
                    success: false,
                    ..Default::default()
                })
            }
        }
    }
}
