//! Plugin manifest parsing.

use serde::Deserialize;

/// A plugin manifest (manifest.json).
#[derive(Debug, Deserialize)]
pub struct PluginManifest {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Tools provided by this plugin.
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
}

/// A tool definition within a plugin manifest.
#[derive(Debug, Deserialize)]
pub struct PluginToolDef {
    /// Tool name (must be unique across all plugins).
    pub name: String,
    /// Description for the LLM.
    pub description: String,
    /// JSON Schema for input parameters.
    #[serde(default = "default_schema")]
    pub input_schema: serde_json::Value,
}

fn default_schema() -> serde_json::Value {
    serde_json::json!({"type": "object"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest() {
        let json = r#"{
            "name": "test-plugin",
            "version": "0.1.0",
            "tools": [
                {
                    "name": "hello",
                    "description": "Say hello",
                    "input_schema": {"type": "object", "properties": {"name": {"type": "string"}}}
                }
            ]
        }"#;
        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.tools[0].name, "hello");
    }

    #[test]
    fn test_default_schema() {
        let json = r#"{
            "name": "minimal",
            "version": "1.0.0",
            "tools": [{"name": "t", "description": "d"}]
        }"#;
        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.tools[0].input_schema, serde_json::json!({"type": "object"}));
    }
}
