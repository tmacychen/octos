//! Configuration file support for crew CLI.

use std::path::{Path, PathBuf};

use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};

/// Current config version.
const CURRENT_CONFIG_VERSION: u32 = 1;

/// LLM provider configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Config {
    /// Config version for migration.
    #[serde(default)]
    pub version: Option<u32>,

    /// LLM provider: "anthropic", "openai", or "gemini".
    #[serde(default)]
    pub provider: Option<String>,

    /// Model name.
    #[serde(default)]
    pub model: Option<String>,

    /// Custom base URL for the API endpoint.
    #[serde(default)]
    pub base_url: Option<String>,

    /// Environment variable name for API key (default: ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY).
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Gateway configuration (optional).
    #[serde(default)]
    pub gateway: Option<GatewayConfig>,

    /// MCP server configurations.
    #[serde(default)]
    pub mcp_servers: Vec<crew_agent::McpServerConfig>,

    /// Sandbox configuration.
    #[serde(default)]
    pub sandbox: crew_agent::SandboxConfig,

    /// Tool access policy (allow/deny lists with group and wildcard support).
    #[serde(default)]
    pub tool_policy: Option<crew_agent::ToolPolicy>,

    /// Per-provider tool policies. Key = model ID or provider name prefix.
    /// Example: `{"gemini": {"deny": ["diff_edit"]}}`.
    #[serde(default)]
    pub tool_policy_by_provider: std::collections::HashMap<String, crew_agent::ToolPolicy>,

    /// Embedding configuration for hybrid memory search.
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,

    /// Fallback models for provider failover chain.
    /// When the primary provider fails with a retriable error, the next model is tried.
    #[serde(default)]
    pub fallback_models: Vec<FallbackModel>,

    /// Lifecycle hooks for agent events.
    #[serde(default)]
    pub hooks: Vec<crew_agent::HookConfig>,
}

/// A fallback model for the provider failover chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FallbackModel {
    /// Provider name (e.g. "openai", "gemini").
    pub provider: String,
    /// Model name.
    #[serde(default)]
    pub model: Option<String>,
    /// Custom base URL.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Embedding provider configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Provider name (currently only "openai").
    #[serde(default = "default_embedding_provider")]
    pub provider: String,

    /// Environment variable name for the API key (overrides provider default).
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Custom base URL for the embedding API.
    #[serde(default)]
    pub base_url: Option<String>,
}

fn default_embedding_provider() -> String {
    "openai".to_string()
}

impl Config {
    /// Directories to scan for plugins: local (.crew/plugins/) then global (~/.crew/plugins/).
    pub fn plugin_dirs(cwd: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        let local = cwd.join(".crew").join("plugins");
        if local.exists() {
            dirs.push(local);
        }
        if let Some(home) = dirs::home_dir() {
            let global = home.join(".crew").join("plugins");
            if global.exists() {
                dirs.push(global);
            }
        }
        dirs
    }
}

/// Gateway mode configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Channels to enable.
    #[serde(default)]
    pub channels: Vec<ChannelEntry>,

    /// Maximum conversation history messages to include.
    #[serde(default = "default_max_history")]
    pub max_history: usize,

    /// Custom system prompt for gateway mode.
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// A channel entry in gateway config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelEntry {
    /// Channel type: "cli", "telegram", "discord".
    #[serde(rename = "type")]
    pub channel_type: String,

    /// Allowed sender IDs (empty = allow all).
    #[serde(default)]
    pub allowed_senders: Vec<String>,

    /// Channel-specific settings.
    #[serde(default)]
    pub settings: serde_json::Value,
}

fn default_max_history() -> usize {
    50
}

impl Config {
    /// Load config from file, returns default if not found.
    pub fn load(cwd: &Path) -> Result<Self> {
        // Try project-local config first
        let local_config = cwd.join(".crew").join("config.json");
        if local_config.exists() {
            return Self::from_file(&local_config);
        }

        // Try global config
        if let Some(global_config) = Self::global_config_path() {
            if global_config.exists() {
                return Self::from_file(&global_config);
            }
        }

        // No config found, use defaults
        Ok(Self::default())
    }

    /// Load config from a specific file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read config file: {}", path.display()))?;

        // Parse as raw Value first for migration
        let mut value: serde_json::Value = serde_json::from_str(&content)
            .wrap_err_with(|| format!("failed to parse config file: {}", path.display()))?;

        let migrated = migrate_config(&mut value);

        let mut config: Self = serde_json::from_value(value)
            .wrap_err_with(|| format!("failed to deserialize config: {}", path.display()))?;

        // Expand environment variables in config values
        config.expand_env_vars();

        // Write back if migration changed something
        if migrated {
            if let Ok(json) = serde_json::to_string_pretty(&config) {
                let _ = std::fs::write(path, json);
            }
        }

        Ok(config)
    }

    /// Get global config path (~/.config/crew/config.json).
    pub fn global_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("crew").join("config.json"))
    }

    /// Expand environment variables in config values.
    /// Supports ${VAR_NAME} syntax.
    fn expand_env_vars(&mut self) {
        if let Some(ref mut base_url) = self.base_url {
            *base_url = Self::expand_env_var(base_url);
        }
        if let Some(ref mut model) = self.model {
            *model = Self::expand_env_var(model);
        }
        if let Some(ref mut provider) = self.provider {
            *provider = Self::expand_env_var(provider);
        }
    }

    /// Expand ${VAR_NAME} patterns in a string.
    fn expand_env_var(s: &str) -> String {
        let mut result = s.to_string();
        let mut start = 0;

        while let Some(begin) = result[start..].find("${") {
            let begin = start + begin;
            if let Some(end) = result[begin..].find('}') {
                let end = begin + end;
                let var_name = &result[begin + 2..end];
                if let Ok(value) = std::env::var(var_name) {
                    result = format!("{}{}{}", &result[..begin], value, &result[end + 1..]);
                    start = begin + value.len();
                } else {
                    start = end + 1;
                }
            } else {
                break;
            }
        }
        result
    }

    /// Get the API key: auth store first, then environment variable.
    pub fn get_api_key(&self, provider: &str) -> Result<String> {
        // Check auth store first.
        if let Ok(store) = crate::auth::AuthStore::load() {
            if let Some(cred) = store.get(provider) {
                if !cred.is_expired() {
                    return Ok(cred.access_token.clone());
                }
            }
        }

        // Fall back to environment variable.
        let env_var = self.api_key_env.clone().unwrap_or_else(|| match provider {
            "anthropic" => "ANTHROPIC_API_KEY".to_string(),
            "openai" => "OPENAI_API_KEY".to_string(),
            "gemini" => "GEMINI_API_KEY".to_string(),
            "zhipu" | "glm" => "ZHIPU_API_KEY".to_string(),
            _ => format!("{}_API_KEY", provider.to_uppercase()),
        });

        std::env::var(&env_var).wrap_err_with(|| {
            format!("{env_var} not set. Run `crew auth login -p {provider}` or set the env var")
        })
    }

    /// Validate the configuration.
    #[allow(clippy::manual_map)]
    pub fn validate(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Check provider is valid
        if let Some(ref provider) = self.provider {
            const VALID: &[&str] = &[
                "anthropic",
                "openai",
                "gemini",
                "openrouter",
                "deepseek",
                "groq",
                "moonshot",
                "kimi",
                "dashscope",
                "qwen",
                "minimax",
                "zhipu",
                "glm",
                "ollama",
                "vllm",
            ];
            if !VALID.contains(&provider.as_str()) {
                warnings.push(format!(
                    "Unknown provider '{}'. Valid options: {}",
                    provider,
                    VALID.join(", ")
                ));
            }
        }

        // Check model/provider mismatch
        if let (Some(provider), Some(model)) = (&self.provider, &self.model) {
            if !is_valid_model_for_provider(provider, model) {
                warnings.push(format!(
                    "Model '{}' may not be valid for provider '{}'. Check provider docs.",
                    model, provider
                ));
            }
        }

        // Check base_url format
        if let Some(ref url) = self.base_url {
            if !(url.starts_with("http://") || url.starts_with("https://")) || url.contains(' ') {
                warnings.push(format!("base_url '{}' is not a valid URL", url));
            }
        }

        // Check gateway config
        if let Some(ref gw) = self.gateway {
            const VALID_CHANNELS: &[&str] = &[
                "cli", "telegram", "discord", "slack", "whatsapp", "email", "feishu",
            ];
            for ch in &gw.channels {
                if !VALID_CHANNELS.contains(&ch.channel_type.as_str()) {
                    warnings.push(format!(
                        "Unknown channel type '{}'. Valid: {}",
                        ch.channel_type,
                        VALID_CHANNELS.join(", ")
                    ));
                }
            }
            if gw.max_history == 0 || gw.max_history > 1000 {
                warnings.push(format!(
                    "max_history {} is out of range (1-1000)",
                    gw.max_history
                ));
            }
        }

        // Check API key is set
        let provider = self.provider.as_deref().unwrap_or("anthropic");
        if self.get_api_key(provider).is_err() {
            let env_var = self.api_key_env.clone().unwrap_or_else(|| match provider {
                "anthropic" => "ANTHROPIC_API_KEY".to_string(),
                "openai" => "OPENAI_API_KEY".to_string(),
                "gemini" => "GEMINI_API_KEY".to_string(),
                "zhipu" | "glm" => "ZHIPU_API_KEY".to_string(),
                _ => format!("{}_API_KEY", provider.to_uppercase()),
            });
            warnings.push(format!("{} environment variable not set", env_var));
        }

        Ok(warnings)
    }
}

/// Migrate config to current version. Returns true if anything changed.
fn migrate_config(value: &mut serde_json::Value) -> bool {
    let current = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    if current >= CURRENT_CONFIG_VERSION {
        return false;
    }

    // Future migrations go here:
    // if current < 2 { ... }

    // Set version to current
    value["version"] = serde_json::json!(CURRENT_CONFIG_VERSION);
    true
}

/// Check if a model name looks reasonable for a given provider.
/// Not exhaustive -- warns on clear mismatches only.
fn is_valid_model_for_provider(provider: &str, model: &str) -> bool {
    let m = model.to_lowercase();
    match provider {
        "anthropic" => m.contains("claude"),
        "openai" => {
            m.contains("gpt") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
        }
        "gemini" | "google" => m.contains("gemini"),
        "deepseek" => m.contains("deepseek"),
        "moonshot" | "kimi" => m.contains("kimi") || m.contains("moonshot"),
        "dashscope" | "qwen" => m.contains("qwen"),
        "zhipu" | "glm" => m.contains("glm"),
        "minimax" => m.contains("minimax"),
        // These host many models, accept any
        "groq" | "ollama" | "vllm" | "openrouter" => true,
        _ => true,
    }
}

/// Detect LLM provider from model name when no explicit provider is set.
pub fn detect_provider(model: &str) -> Option<&'static str> {
    let m = model.to_lowercase();
    if m.contains("claude") {
        return Some("anthropic");
    }
    if m.contains("gpt") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        return Some("openai");
    }
    if m.contains("gemini") {
        return Some("gemini");
    }
    if m.contains("deepseek") {
        return Some("deepseek");
    }
    if m.contains("kimi") || m.contains("moonshot") {
        return Some("moonshot");
    }
    if m.contains("qwen") {
        return Some("dashscope");
    }
    if m.contains("glm") {
        return Some("zhipu");
    }
    if m.contains("minimax") {
        return Some("minimax");
    }
    if m.contains("llama") || m.contains("mixtral") {
        return Some("groq");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(unsafe_code)]
    fn test_expand_env_var() {
        // SAFETY: test-only, single-threaded
        unsafe {
            std::env::set_var("TEST_VAR", "hello");
        }
        assert_eq!(Config::expand_env_var("${TEST_VAR}"), "hello");
        assert_eq!(
            Config::expand_env_var("prefix_${TEST_VAR}_suffix"),
            "prefix_hello_suffix"
        );
        assert_eq!(Config::expand_env_var("no_var"), "no_var");
        assert_eq!(
            Config::expand_env_var("${UNDEFINED_VAR}"),
            "${UNDEFINED_VAR}"
        );
        // SAFETY: test-only, single-threaded
        unsafe {
            std::env::remove_var("TEST_VAR");
        }
    }

    #[test]
    fn test_gateway_config_deserialize() {
        let json = r#"{
            "provider": "anthropic",
            "model": "claude-sonnet-4-20250514",
            "gateway": {
                "channels": [{"type": "cli"}],
                "max_history": 30
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let gw = config.gateway.unwrap();
        assert_eq!(gw.channels.len(), 1);
        assert_eq!(gw.channels[0].channel_type, "cli");
        assert_eq!(gw.max_history, 30);
        assert!(gw.system_prompt.is_none());
    }

    #[test]
    fn test_gateway_config_defaults() {
        let json = r#"{"provider": "anthropic"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.gateway.is_none());
    }

    #[test]
    fn test_gateway_max_history_default() {
        let json = r#"{"channels": [{"type": "cli"}]}"#;
        let gw: GatewayConfig = serde_json::from_str(json).unwrap();
        assert_eq!(gw.max_history, 50);
    }

    #[test]
    fn test_detect_provider_claude() {
        assert_eq!(
            detect_provider("claude-sonnet-4-20250514"),
            Some("anthropic")
        );
        assert_eq!(detect_provider("claude-3-haiku"), Some("anthropic"));
    }

    #[test]
    fn test_detect_provider_openai() {
        assert_eq!(detect_provider("gpt-4o"), Some("openai"));
        assert_eq!(detect_provider("o1-mini"), Some("openai"));
        assert_eq!(detect_provider("o3-mini"), Some("openai"));
    }

    #[test]
    fn test_detect_provider_others() {
        assert_eq!(detect_provider("gemini-2.0-flash"), Some("gemini"));
        assert_eq!(detect_provider("deepseek-chat"), Some("deepseek"));
        assert_eq!(detect_provider("kimi-k2.5"), Some("moonshot"));
        assert_eq!(detect_provider("qwen-max"), Some("dashscope"));
        assert_eq!(detect_provider("glm-4-plus"), Some("zhipu"));
        assert_eq!(detect_provider("llama-3.3-70b"), Some("groq"));
    }

    #[test]
    fn test_detect_provider_unknown() {
        assert_eq!(detect_provider("some-custom-model"), None);
    }

    #[test]
    fn test_validate_unknown_provider() {
        let config = Config {
            provider: Some("invalid".to_string()),
            ..Default::default()
        };
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("Unknown provider")));
    }

    #[test]
    fn test_validate_model_mismatch() {
        let config = Config {
            provider: Some("anthropic".to_string()),
            model: Some("gpt-4o".to_string()),
            ..Default::default()
        };
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("may not be valid")));
    }

    #[test]
    fn test_validate_invalid_base_url() {
        let config = Config {
            base_url: Some("not a url".to_string()),
            ..Default::default()
        };
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("not a valid URL")));
    }

    #[test]
    fn test_validate_invalid_channel_type() {
        let config = Config {
            gateway: Some(GatewayConfig {
                channels: vec![ChannelEntry {
                    channel_type: "irc".to_string(),
                    allowed_senders: vec![],
                    settings: serde_json::json!({}),
                }],
                max_history: 50,
                system_prompt: None,
            }),
            ..Default::default()
        };
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("Unknown channel type")));
    }

    #[test]
    fn test_embedding_config_deserialize() {
        let json = r#"{
            "provider": "anthropic",
            "embedding": {
                "provider": "openai",
                "base_url": "https://custom.api.com/v1"
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let emb = config.embedding.unwrap();
        assert_eq!(emb.provider, "openai");
        assert_eq!(emb.base_url.unwrap(), "https://custom.api.com/v1");
        assert!(emb.api_key_env.is_none());
    }

    #[test]
    fn test_embedding_config_absent() {
        let json = r#"{"provider": "anthropic"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.embedding.is_none());
    }

    #[test]
    fn test_tool_policy_by_provider_deserialize() {
        let json = r#"{
            "provider": "anthropic",
            "tool_policy_by_provider": {
                "gemini": {"deny": ["diff_edit"]},
                "claude-sonnet-4-20250514": {"allow": ["shell", "read_file"]}
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.tool_policy_by_provider.len(), 2);
        assert!(config.tool_policy_by_provider.contains_key("gemini"));
        assert!(
            config
                .tool_policy_by_provider
                .contains_key("claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn test_tool_policy_by_provider_absent() {
        let json = r#"{"provider": "anthropic"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.tool_policy_by_provider.is_empty());
    }

    #[test]
    fn test_validate_max_history_out_of_range() {
        let config = Config {
            gateway: Some(GatewayConfig {
                channels: vec![],
                max_history: 0,
                system_prompt: None,
            }),
            ..Default::default()
        };
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("out of range")));
    }
}
