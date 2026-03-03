//! Sub-account management commands.
//!
//! Sub-accounts inherit LLM provider config from a parent profile but have
//! their own data directory (memory, sessions, episodes, skills) and channels.

use clap::{Args, Subcommand};
use eyre::{Result, bail};

use super::Executable;
use crate::profiles::{ChannelCredentials, GatewaySettings, ProfileStore};

/// Manage sub-accounts under profiles.
#[derive(Debug, Args)]
pub struct AccountCommand {
    #[command(subcommand)]
    pub action: AccountAction,
}

#[derive(Debug, Subcommand)]
pub enum AccountAction {
    /// List sub-accounts for a profile.
    List {
        /// Parent profile ID.
        #[arg(long)]
        profile: String,
    },
    /// Create a new sub-account under a profile.
    Create {
        /// Parent profile ID.
        #[arg(long)]
        profile: String,
        /// Sub-account display name.
        name: String,
        /// Telegram bot token (creates a Telegram channel).
        #[arg(long)]
        telegram_token: Option<String>,
        /// Enable WhatsApp channel (auto-managed bridge).
        #[arg(long)]
        whatsapp: bool,
        /// System prompt for this sub-account.
        #[arg(long)]
        system_prompt: Option<String>,
        /// Auto-enable after creation.
        #[arg(long)]
        enable: bool,
    },
    /// Delete a sub-account.
    Delete {
        /// Sub-account ID (e.g. parent--work).
        id: String,
    },
    /// Show sub-account info.
    Info {
        /// Sub-account ID.
        id: String,
    },
}

impl Executable for AccountCommand {
    fn execute(self) -> Result<()> {
        let data_dir = super::resolve_data_dir(None)?;
        let store = ProfileStore::open(&data_dir)?;

        match self.action {
            AccountAction::List { profile } => {
                // Verify parent exists
                if store.get(&profile)?.is_none() {
                    bail!("profile '{profile}' not found");
                }

                let subs = store.list_sub_accounts(&profile)?;
                if subs.is_empty() {
                    println!("No sub-accounts for profile '{profile}'.");
                    println!("Create one with: crew account create --profile {profile} <name>");
                    return Ok(());
                }

                println!("Sub-accounts for '{profile}':");
                println!("{:<30} {:<20} {:<10}", "ID", "NAME", "ENABLED");
                println!("{}", "-".repeat(60));
                for s in &subs {
                    let channels: Vec<&str> = s.config.channels.iter().map(channel_type).collect();
                    println!(
                        "{:<30} {:<20} {:<10} [{}]",
                        s.id,
                        s.name,
                        if s.enabled { "yes" } else { "no" },
                        channels.join(", ")
                    );
                }
            }

            AccountAction::Create {
                profile,
                name,
                telegram_token,
                whatsapp,
                system_prompt,
                enable,
            } => {
                let mut channels = Vec::new();
                let mut env_vars = std::collections::HashMap::new();

                if let Some(ref token) = telegram_token {
                    let env_name = format!(
                        "TELEGRAM_BOT_TOKEN_{}",
                        name.to_uppercase().replace(' ', "_")
                    );
                    channels.push(ChannelCredentials::Telegram {
                        token_env: env_name.clone(),
                        allowed_senders: String::new(),
                    });
                    env_vars.insert(env_name, token.clone());
                }

                if whatsapp {
                    channels.push(ChannelCredentials::WhatsApp {
                        bridge_url: String::new(), // auto-managed
                    });
                }

                let gateway = GatewaySettings {
                    system_prompt,
                    ..Default::default()
                };

                let mut sub = store.create_sub_account(&profile, &name, channels, gateway)?;

                // Save env vars
                if !env_vars.is_empty() {
                    sub.config.env_vars = env_vars;
                    if enable {
                        sub.enabled = true;
                    }
                    sub.updated_at = chrono::Utc::now();
                    store.save(&sub)?;
                } else if enable {
                    sub.enabled = true;
                    sub.updated_at = chrono::Utc::now();
                    store.save(&sub)?;
                }

                println!("Created sub-account: {}", sub.id);
                println!("  Name: {}", sub.name);
                println!("  Parent: {profile}");
                println!("  Enabled: {}", sub.enabled);
                println!("  Data dir: {}", store.resolve_data_dir(&sub).display());

                if sub.config.channels.is_empty() {
                    println!();
                    println!("No channels configured. Add via dashboard or:");
                    println!(
                        "  crew account create --profile {profile} <name> --telegram-token <token>"
                    );
                }
            }

            AccountAction::Delete { id } => {
                let profile = store
                    .get(&id)?
                    .ok_or_else(|| eyre::eyre!("sub-account '{id}' not found"))?;

                if profile.parent_id.is_none() {
                    bail!(
                        "'{id}' is a top-level profile, not a sub-account. Use the dashboard to delete it."
                    );
                }

                store.delete(&id)?;
                println!("Deleted sub-account: {id}");
            }

            AccountAction::Info { id } => {
                let profile = store
                    .get(&id)?
                    .ok_or_else(|| eyre::eyre!("sub-account '{id}' not found"))?;

                let parent_label = profile
                    .parent_id
                    .as_deref()
                    .unwrap_or("(none — top-level profile)");

                println!("Sub-account: {}", profile.id);
                println!("  Name: {}", profile.name);
                println!("  Parent: {parent_label}");
                println!("  Enabled: {}", profile.enabled);
                println!("  Data dir: {}", store.resolve_data_dir(&profile).display());

                if !profile.config.channels.is_empty() {
                    println!("  Channels:");
                    for ch in &profile.config.channels {
                        println!("    - {}", channel_type(ch));
                    }
                }

                // Show effective provider if it's a sub-account
                if profile.parent_id.is_some() {
                    match crate::profiles::resolve_effective_profile(&store, &profile) {
                        Ok(eff) => {
                            println!(
                                "  Provider: {} (inherited)",
                                eff.config.provider.as_deref().unwrap_or("none")
                            );
                            println!(
                                "  Model: {} (inherited)",
                                eff.config.model.as_deref().unwrap_or("none")
                            );
                        }
                        Err(e) => {
                            println!("  Provider: error resolving — {e}");
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn channel_type(ch: &ChannelCredentials) -> &'static str {
    match ch {
        ChannelCredentials::Telegram { .. } => "telegram",
        ChannelCredentials::Discord { .. } => "discord",
        ChannelCredentials::Slack { .. } => "slack",
        ChannelCredentials::WhatsApp { .. } => "whatsapp",
        ChannelCredentials::Feishu { .. } => "feishu",
        ChannelCredentials::Email { .. } => "email",
    }
}
