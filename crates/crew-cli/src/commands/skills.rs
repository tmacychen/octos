//! Skills command: list, install, and remove skills.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use colored::Colorize;
use eyre::{Result, WrapErr};

use super::Executable;

/// Manage agent skills.
#[derive(Debug, Args)]
pub struct SkillsCommand {
    /// Working directory (defaults to current directory).
    #[arg(short, long)]
    pub cwd: Option<PathBuf>,

    #[command(subcommand)]
    pub subcommand: SkillsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum SkillsSubcommand {
    /// List installed skills.
    List,
    /// Install a skill from GitHub (e.g. user/repo/skill-name).
    Install {
        /// GitHub path: user/repo/skill-name (fetches SKILL.md from main branch).
        repo: String,
    },
    /// Remove an installed skill.
    Remove {
        /// Skill name to remove.
        name: String,
    },
}

impl Executable for SkillsCommand {
    fn execute(self) -> Result<()> {
        let cwd = self.cwd.unwrap_or_else(|| std::env::current_dir().unwrap());
        let skills_dir = cwd.join(".crew").join("skills");

        match self.subcommand {
            SkillsSubcommand::List => cmd_list(&skills_dir),
            SkillsSubcommand::Install { repo } => cmd_install(&skills_dir, &repo),
            SkillsSubcommand::Remove { name } => cmd_remove(&skills_dir, &name),
        }
    }
}

fn cmd_list(skills_dir: &std::path::Path) -> Result<()> {
    println!("{}", "Installed Skills".cyan().bold());
    println!("{}", "=".repeat(50));
    println!();

    // Built-in skills
    let builtins = [
        "cron",
        "github",
        "skill-creator",
        "summarize",
        "tmux",
        "weather",
    ];
    for name in &builtins {
        let overridden = skills_dir.join(name).join("SKILL.md").exists();
        if overridden {
            println!("  {} {}", name.cyan(), "(overridden by workspace)".dimmed());
        } else {
            println!("  {} {}", name.cyan(), "(built-in)".dimmed());
        }
    }

    // Workspace skills
    if skills_dir.exists() {
        let mut found_custom = false;
        for entry in std::fs::read_dir(skills_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !builtins.contains(&name.as_str()) && entry.path().join("SKILL.md").exists() {
                if !found_custom {
                    println!();
                    found_custom = true;
                }
                println!("  {} {}", name.cyan(), "(workspace)".dimmed());
            }
        }
    }

    println!();
    Ok(())
}

fn cmd_install(skills_dir: &std::path::Path, repo: &str) -> Result<()> {
    // Extract skill name from repo path (last segment)
    let name = repo
        .rsplit('/')
        .next()
        .ok_or_else(|| eyre::eyre!("invalid repo path: {repo}"))?;

    let dest = skills_dir.join(name);
    if dest.exists() {
        eyre::bail!("Skill '{name}' already exists at {}", dest.display());
    }

    // Fetch SKILL.md from GitHub raw content
    let url = format!("https://raw.githubusercontent.com/{repo}/main/SKILL.md");
    println!("Fetching {}...", url.dimmed());

    let body = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .wrap_err("failed to create HTTP client")?
        .get(&url)
        .send()
        .wrap_err_with(|| format!("failed to fetch {url}"))?;

    if !body.status().is_success() {
        eyre::bail!(
            "Failed to fetch SKILL.md (HTTP {}). Check the repo path: {repo}",
            body.status()
        );
    }

    let content = body.text().wrap_err("failed to read response body")?;

    // Write to skills directory
    std::fs::create_dir_all(&dest)?;
    std::fs::write(dest.join("SKILL.md"), &content)?;

    println!(
        "{} Installed skill '{}' to {}",
        "OK".green(),
        name.cyan(),
        dest.display()
    );
    Ok(())
}

fn cmd_remove(skills_dir: &std::path::Path, name: &str) -> Result<()> {
    let dest = skills_dir.join(name);
    if !dest.exists() {
        eyre::bail!("Skill '{name}' not found in {}", skills_dir.display());
    }

    std::fs::remove_dir_all(&dest)?;
    println!("{} Removed skill '{}'", "OK".green(), name.cyan());
    Ok(())
}
