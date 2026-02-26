//! User profile management for multi-user deployments.
//!
//! Each profile is a named configuration bundle that defines an LLM provider,
//! channel credentials, and gateway settings. Profiles are stored as individual
//! JSON files in `~/.crew/profiles/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use eyre::{Result, WrapErr, bail};
use serde::{Deserialize, Serialize};

/// A user profile with all configuration needed to run a gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// Unique identifier (slug: lowercase alphanumeric + hyphens).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether this profile's gateway should auto-start with the server.
    #[serde(default)]
    pub enabled: bool,
    /// Data directory override. Default: `~/.crew/profiles/{id}/data`
    #[serde(default)]
    pub data_dir: Option<String>,
    /// Inline configuration.
    pub config: ProfileConfig,
    /// When this profile was created.
    pub created_at: DateTime<Utc>,
    /// When this profile was last modified.
    pub updated_at: DateTime<Utc>,
}

/// LLM and gateway configuration for a profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// LLM provider name (anthropic, openai, etc.).
    #[serde(default)]
    pub provider: Option<String>,
    /// Model name.
    #[serde(default)]
    pub model: Option<String>,
    /// Env var name for API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Channel configurations.
    #[serde(default)]
    pub channels: Vec<ChannelCredentials>,
    /// Gateway-specific settings.
    #[serde(default)]
    pub gateway: GatewaySettings,
    /// Environment variables to pass to the gateway process (e.g. API keys).
    /// Keys are env var names, values are the actual secrets.
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
}

/// Channel-specific credentials (tagged by type).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChannelCredentials {
    Telegram {
        #[serde(default = "default_telegram_env")]
        token_env: String,
    },
    Discord {
        #[serde(default = "default_discord_env")]
        token_env: String,
    },
    Slack {
        #[serde(default = "default_slack_bot_env")]
        bot_token_env: String,
        #[serde(default = "default_slack_app_env")]
        app_token_env: String,
    },
    #[serde(rename = "whatsapp")]
    WhatsApp {
        #[serde(default = "default_whatsapp_url")]
        bridge_url: String,
    },
    Feishu {
        #[serde(default = "default_feishu_id_env")]
        app_id_env: String,
        #[serde(default = "default_feishu_secret_env")]
        app_secret_env: String,
    },
    Email {
        #[serde(default)]
        imap_host: String,
        #[serde(default = "default_imap_port")]
        imap_port: u16,
        #[serde(default)]
        smtp_host: String,
        #[serde(default = "default_smtp_port")]
        smtp_port: u16,
        #[serde(default = "default_email_user_env")]
        username_env: String,
        #[serde(default = "default_email_pass_env")]
        password_env: String,
    },
}

fn default_telegram_env() -> String {
    "TELEGRAM_BOT_TOKEN".into()
}
fn default_discord_env() -> String {
    "DISCORD_BOT_TOKEN".into()
}
fn default_slack_bot_env() -> String {
    "SLACK_BOT_TOKEN".into()
}
fn default_slack_app_env() -> String {
    "SLACK_APP_TOKEN".into()
}
fn default_whatsapp_url() -> String {
    "ws://localhost:3001".into()
}
fn default_feishu_id_env() -> String {
    "FEISHU_APP_ID".into()
}
fn default_feishu_secret_env() -> String {
    "FEISHU_APP_SECRET".into()
}
fn default_imap_port() -> u16 {
    993
}
fn default_smtp_port() -> u16 {
    465
}
fn default_email_user_env() -> String {
    "EMAIL_USERNAME".into()
}
fn default_email_pass_env() -> String {
    "EMAIL_PASSWORD".into()
}

/// Gateway-specific settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewaySettings {
    #[serde(default)]
    pub max_history: Option<usize>,
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_concurrent_sessions: Option<usize>,
}

/// Manages profile storage as individual JSON files.
pub struct ProfileStore {
    profiles_dir: PathBuf,
}

impl ProfileStore {
    /// Open (or create) the profile store at `data_dir/profiles/`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let profiles_dir = data_dir.join("profiles");
        std::fs::create_dir_all(&profiles_dir).wrap_err_with(|| {
            format!("failed to create profiles dir: {}", profiles_dir.display())
        })?;
        Ok(Self { profiles_dir })
    }

    /// List all profiles sorted by name.
    pub fn list(&self) -> Result<Vec<UserProfile>> {
        let mut profiles = Vec::new();
        let entries = match std::fs::read_dir(&self.profiles_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(profiles),
            Err(e) => return Err(e).wrap_err("failed to read profiles directory"),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<UserProfile>(&content) {
                        Ok(profile) => profiles.push(profile),
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "skipping invalid profile");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "failed to read profile");
                    }
                }
            }
        }
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(profiles)
    }

    /// Get a single profile by ID.
    pub fn get(&self, id: &str) -> Result<Option<UserProfile>> {
        let path = self.profile_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .wrap_err_with(|| format!("failed to read profile: {id}"))?;
        let profile = serde_json::from_str(&content)
            .wrap_err_with(|| format!("failed to parse profile: {id}"))?;
        Ok(Some(profile))
    }

    /// Save a profile (create or update). Also initializes the data directory.
    pub fn save(&self, profile: &UserProfile) -> Result<()> {
        validate_profile_id(&profile.id)?;

        // Initialize data directory structure
        let data_dir = self.resolve_data_dir(profile);
        for sub in ["memory", "sessions", "research", "skills", "history"] {
            std::fs::create_dir_all(data_dir.join(sub)).ok();
        }

        let path = self.profile_path(&profile.id);
        let content =
            serde_json::to_string_pretty(profile).wrap_err("failed to serialize profile")?;
        std::fs::write(&path, &content)
            .wrap_err_with(|| format!("failed to write profile: {}", path.display()))?;

        // Restrict file permissions to owner-only (mode 0600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&path, perms) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to set restrictive permissions on profile file"
                );
            }
        }

        Ok(())
    }

    /// Delete a profile by ID.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let path = self.profile_path(id);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).wrap_err_with(|| format!("failed to delete profile: {id}"))?;
        Ok(true)
    }

    /// Resolve the data directory for a profile.
    pub fn resolve_data_dir(&self, profile: &UserProfile) -> PathBuf {
        if let Some(ref dir) = profile.data_dir {
            PathBuf::from(dir)
        } else {
            self.profiles_dir.join(&profile.id).join("data")
        }
    }

    /// Generate a crew-rs config JSON for a profile's gateway process.
    pub fn generate_config(&self, profile: &UserProfile) -> Result<PathBuf> {
        let config_dir = self.profiles_dir.join(&profile.id);
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.json");

        let channels: Vec<serde_json::Value> = profile
            .config
            .channels
            .iter()
            .map(channel_to_entry)
            .collect();

        let mut gateway = serde_json::json!({
            "channels": channels,
        });
        if let Some(mh) = profile.config.gateway.max_history {
            gateway["max_history"] = serde_json::json!(mh);
        }
        if let Some(ref sp) = profile.config.gateway.system_prompt {
            gateway["system_prompt"] = serde_json::json!(sp);
        }
        if let Some(mcs) = profile.config.gateway.max_concurrent_sessions {
            gateway["max_concurrent_sessions"] = serde_json::json!(mcs);
        }

        let mut config = serde_json::json!({
            "gateway": gateway,
        });
        if let Some(ref p) = profile.config.provider {
            config["provider"] = serde_json::json!(p);
        }
        if let Some(ref m) = profile.config.model {
            config["model"] = serde_json::json!(m);
        }
        if let Some(ref k) = profile.config.api_key_env {
            config["api_key_env"] = serde_json::json!(k);
        }

        let content = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, content)?;
        Ok(config_path)
    }

    fn profile_path(&self, id: &str) -> PathBuf {
        self.profiles_dir.join(format!("{id}.json"))
    }
}

/// Return a copy of the profile with secret values in `env_vars` masked.
/// Shows the first 4 and last 3 characters for keys longer than 12 chars,
/// otherwise replaces the entire value with `***`.
pub fn mask_secrets(profile: &UserProfile) -> UserProfile {
    let mut masked = profile.clone();
    for value in masked.config.env_vars.values_mut() {
        *value = mask_value(value);
    }
    masked
}

fn mask_value(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    if len > 12 {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[len - 3..].iter().collect();
        format!("{prefix}***{suffix}")
    } else if len > 0 {
        "***".into()
    } else {
        String::new()
    }
}

/// Validate a profile ID (slug format).
fn validate_profile_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        bail!("profile ID must be 1-64 characters");
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("profile ID must contain only lowercase letters, digits, and hyphens");
    }
    if id.starts_with('-') || id.ends_with('-') {
        bail!("profile ID must not start or end with a hyphen");
    }
    Ok(())
}

/// Convert a `ChannelCredentials` to a crew-rs `ChannelEntry` JSON value.
fn channel_to_entry(cred: &ChannelCredentials) -> serde_json::Value {
    match cred {
        ChannelCredentials::Telegram { token_env } => serde_json::json!({
            "type": "telegram",
            "settings": { "token_env": token_env }
        }),
        ChannelCredentials::Discord { token_env } => serde_json::json!({
            "type": "discord",
            "settings": { "token_env": token_env }
        }),
        ChannelCredentials::Slack {
            bot_token_env,
            app_token_env,
        } => serde_json::json!({
            "type": "slack",
            "settings": { "bot_token_env": bot_token_env, "app_token_env": app_token_env }
        }),
        ChannelCredentials::WhatsApp { bridge_url } => serde_json::json!({
            "type": "whatsapp",
            "settings": { "bridge_url": bridge_url }
        }),
        ChannelCredentials::Feishu {
            app_id_env,
            app_secret_env,
        } => serde_json::json!({
            "type": "feishu",
            "settings": { "app_id_env": app_id_env, "app_secret_env": app_secret_env }
        }),
        ChannelCredentials::Email {
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            username_env,
            password_env,
        } => serde_json::json!({
            "type": "email",
            "settings": {
                "imap_host": imap_host,
                "imap_port": imap_port,
                "smtp_host": smtp_host,
                "smtp_port": smtp_port,
                "username_env": username_env,
                "password_env": password_env,
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_profile_id() {
        assert!(validate_profile_id("alice").is_ok());
        assert!(validate_profile_id("team-bot").is_ok());
        assert!(validate_profile_id("user123").is_ok());
        assert!(validate_profile_id("").is_err());
        assert!(validate_profile_id("-bad").is_err());
        assert!(validate_profile_id("bad-").is_err());
        assert!(validate_profile_id("UPPER").is_err());
        assert!(validate_profile_id("has space").is_err());
        assert!(validate_profile_id("a".repeat(65).as_str()).is_err());
    }

    #[test]
    fn test_profile_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path()).unwrap();

        let profile = UserProfile {
            id: "test".into(),
            name: "Test Bot".into(),
            enabled: true,
            data_dir: None,
            config: ProfileConfig {
                provider: Some("anthropic".into()),
                model: Some("claude-sonnet-4-20250514".into()),
                api_key_env: Some("ANTHROPIC_API_KEY".into()),
                channels: vec![ChannelCredentials::Telegram {
                    token_env: "TG_TOKEN".into(),
                }],
                gateway: GatewaySettings {
                    max_history: Some(50),
                    ..Default::default()
                },
                env_vars: Default::default(),
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        store.save(&profile).unwrap();
        let loaded = store.get("test").unwrap().unwrap();
        assert_eq!(loaded.id, "test");
        assert_eq!(loaded.name, "Test Bot");
        assert!(loaded.enabled);

        let profiles = store.list().unwrap();
        assert_eq!(profiles.len(), 1);

        assert!(store.delete("test").unwrap());
        assert!(store.get("test").unwrap().is_none());
    }

    #[test]
    fn test_generate_config() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path()).unwrap();

        let profile = UserProfile {
            id: "gen-test".into(),
            name: "Config Gen".into(),
            enabled: false,
            data_dir: None,
            config: ProfileConfig {
                provider: Some("openai".into()),
                model: Some("gpt-4o".into()),
                api_key_env: None,
                channels: vec![
                    ChannelCredentials::Telegram {
                        token_env: "TG".into(),
                    },
                    ChannelCredentials::Slack {
                        bot_token_env: "SB".into(),
                        app_token_env: "SA".into(),
                    },
                ],
                gateway: GatewaySettings {
                    max_history: Some(100),
                    system_prompt: Some("Hello".into()),
                    ..Default::default()
                },
                env_vars: Default::default(),
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let config_path = store.generate_config(&profile).unwrap();
        assert!(config_path.exists());

        let content = std::fs::read_to_string(&config_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["provider"], "openai");
        assert_eq!(json["model"], "gpt-4o");
        assert_eq!(json["gateway"]["max_history"], 100);
        assert_eq!(json["gateway"]["system_prompt"], "Hello");
        assert_eq!(json["gateway"]["channels"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_resolve_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path()).unwrap();

        let mut profile = UserProfile {
            id: "alice".into(),
            name: "Alice".into(),
            enabled: false,
            data_dir: None,
            config: ProfileConfig::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        // Default: profiles_dir/{id}/data
        let default_dir = store.resolve_data_dir(&profile);
        assert!(default_dir.ends_with("alice/data"));

        // Override
        profile.data_dir = Some("/custom/path".into());
        let custom_dir = store.resolve_data_dir(&profile);
        assert_eq!(custom_dir, PathBuf::from("/custom/path"));
    }

    #[test]
    fn test_mask_secrets() {
        assert_eq!(mask_value(""), "");
        assert_eq!(mask_value("short"), "***");
        assert_eq!(mask_value("exactly12ch"), "***");
        assert_eq!(mask_value("sk-1234567890abcdef"), "sk-1***def");

        let profile = UserProfile {
            id: "test".into(),
            name: "Test".into(),
            enabled: false,
            data_dir: None,
            config: ProfileConfig {
                env_vars: [
                    ("API_KEY".into(), "sk-1234567890abcdef".into()),
                    ("SHORT".into(), "abc".into()),
                ]
                .into(),
                ..Default::default()
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let masked = mask_secrets(&profile);
        assert_eq!(masked.config.env_vars["API_KEY"], "sk-1***def");
        assert_eq!(masked.config.env_vars["SHORT"], "***");
    }

    #[test]
    fn test_file_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::open(dir.path()).unwrap();
        let profile = UserProfile {
            id: "perms-test".into(),
            name: "Perms".into(),
            enabled: false,
            data_dir: None,
            config: ProfileConfig::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.save(&profile).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(store.profile_path("perms-test")).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn test_channel_serde_roundtrip() {
        let channels = vec![
            ChannelCredentials::Telegram {
                token_env: "TG".into(),
            },
            ChannelCredentials::Discord {
                token_env: "DC".into(),
            },
            ChannelCredentials::Slack {
                bot_token_env: "SB".into(),
                app_token_env: "SA".into(),
            },
            ChannelCredentials::WhatsApp {
                bridge_url: "ws://localhost:3001".into(),
            },
            ChannelCredentials::Feishu {
                app_id_env: "FID".into(),
                app_secret_env: "FSE".into(),
            },
            ChannelCredentials::Email {
                imap_host: "imap.test.com".into(),
                imap_port: 993,
                smtp_host: "smtp.test.com".into(),
                smtp_port: 465,
                username_env: "EU".into(),
                password_env: "EP".into(),
            },
        ];

        let json = serde_json::to_string(&channels).unwrap();
        let parsed: Vec<ChannelCredentials> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 6);
    }
}
