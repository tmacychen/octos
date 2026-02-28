//! Skills command: list, install, and remove skills.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use colored::Colorize;
use eyre::{Result, WrapErr};
use serde::Deserialize;

use super::Executable;

const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/humanagency-org/skill-registry/main/registry.json";

/// A skill package entry in the registry.
#[derive(Debug, Deserialize)]
struct RegistryEntry {
    /// Package name.
    name: String,
    /// Human-readable description.
    description: String,
    /// GitHub repo path (user/repo).
    repo: String,
    /// Individual skill names included in this package.
    #[serde(default)]
    skills: Vec<String>,
    /// External tools required (e.g. git, node).
    #[serde(default)]
    requires: Vec<String>,
    /// Searchable tags.
    #[serde(default)]
    tags: Vec<String>,
}

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
    /// Install skills from GitHub (e.g. user/repo or user/repo/skill-name).
    Install {
        /// GitHub path: user/repo (all skills) or user/repo/skill-name (single skill).
        repo: String,
        /// Overwrite existing skills.
        #[arg(long)]
        force: bool,
        /// Git branch or tag to clone (default: main).
        #[arg(long, default_value = "main")]
        branch: String,
    },
    /// Remove an installed skill.
    Remove {
        /// Skill name to remove.
        name: String,
    },
    /// Search available skill packages from the registry.
    Search {
        /// Optional search query to filter results.
        query: Option<String>,
        /// Custom registry URL.
        #[arg(long)]
        registry: Option<String>,
    },
}

impl Executable for SkillsCommand {
    fn execute(self) -> Result<()> {
        let cwd = match self.cwd {
            Some(p) => p,
            None => std::env::current_dir().wrap_err("failed to get current directory")?,
        };
        let skills_dir = cwd.join(".crew").join("skills");

        match self.subcommand {
            SkillsSubcommand::List => cmd_list(&skills_dir),
            SkillsSubcommand::Install {
                repo,
                force,
                branch,
            } => cmd_install(&skills_dir, &repo, force, &branch),
            SkillsSubcommand::Remove { name } => cmd_remove(&skills_dir, &name),
            SkillsSubcommand::Search { query, registry } => cmd_search(query.as_deref(), registry.as_deref()),
        }
    }
}

fn cmd_list(skills_dir: &Path) -> Result<()> {
    println!("{}", "Installed Skills".cyan().bold());
    println!("{}", "=".repeat(50));
    println!();

    // Built-in skills
    let builtins = [
        "cron",
        "github",
        "news",
        "skill-creator",
        "skill-store",
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

fn cmd_search(query: Option<&str>, registry_url: Option<&str>) -> Result<()> {
    let url = registry_url.unwrap_or(DEFAULT_REGISTRY_URL);

    let entries: Vec<RegistryEntry> = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .wrap_err("failed to create HTTP client")?
        .get(url)
        .send()
        .wrap_err_with(|| format!("failed to fetch registry from {url}"))?
        .error_for_status()
        .wrap_err("registry request failed")?
        .json()
        .wrap_err("failed to parse registry JSON")?;

    // Filter by query if provided (match against name, description, tags, skills).
    let query_lower = query.map(|q| q.to_lowercase());
    let filtered: Vec<&RegistryEntry> = entries
        .iter()
        .filter(|e| {
            let Some(q) = &query_lower else {
                return true;
            };
            e.name.to_lowercase().contains(q)
                || e.description.to_lowercase().contains(q)
                || e.tags.iter().any(|t| t.to_lowercase().contains(q))
                || e.skills.iter().any(|s| s.to_lowercase().contains(q))
        })
        .collect();

    println!("{}", "Available Skill Packages".cyan().bold());
    println!("{}", "=".repeat(50));
    println!();

    if filtered.is_empty() {
        if let Some(q) = query {
            println!("  No packages matching '{}'", q);
        } else {
            println!("  Registry is empty.");
        }
        println!();
        return Ok(());
    }

    for entry in &filtered {
        println!("  {}  {}", entry.name.cyan().bold(), entry.description);
        if !entry.skills.is_empty() {
            println!(
                "  {}  {}",
                "Skills:".dimmed(),
                entry.skills.join(", ")
            );
        }
        if !entry.requires.is_empty() {
            println!(
                "  {} {}",
                "Requires:".dimmed(),
                entry.requires.join(", ")
            );
        }
        if !entry.tags.is_empty() {
            println!(
                "  {}     {}",
                "Tags:".dimmed(),
                entry.tags.join(", ")
            );
        }
        println!(
            "  {} crew skills install {}",
            "Install:".dimmed(),
            entry.repo
        );
        println!();
    }

    Ok(())
}

/// Parsed repo specification.
struct RepoSpec {
    /// GitHub user/org.
    user: String,
    /// Repository name.
    repo: String,
    /// Optional subdirectory within the repo (for single-skill install).
    subdir: Option<String>,
}

impl RepoSpec {
    fn parse(input: &str) -> Result<Self> {
        let segments: Vec<&str> = input.trim_matches('/').split('/').collect();
        match segments.len() {
            2 => Ok(Self {
                user: segments[0].to_string(),
                repo: segments[1].to_string(),
                subdir: None,
            }),
            3.. => Ok(Self {
                user: segments[0].to_string(),
                repo: segments[1].to_string(),
                subdir: Some(segments[2..].join("/")),
            }),
            _ => eyre::bail!(
                "Invalid repo path: '{input}'. Expected user/repo or user/repo/skill-name"
            ),
        }
    }

    fn clone_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.user, self.repo)
    }
}

fn cmd_install(skills_dir: &Path, repo: &str, force: bool, branch: &str) -> Result<()> {
    let spec = RepoSpec::parse(repo)?;

    // Try git clone first, fall back to HTTP for single-skill installs
    match install_via_git(skills_dir, &spec, force, branch) {
        Ok(()) => Ok(()),
        Err(e) => {
            // If git is not available and we have a subdir target, try HTTP fallback
            let is_git_missing = e.to_string().contains("git not found");
            if is_git_missing && spec.subdir.is_some() {
                println!(
                    "{}",
                    "git not found, falling back to HTTP fetch...".yellow()
                );
                install_via_http(skills_dir, &spec, force, branch)
            } else {
                Err(e)
            }
        }
    }
}

fn install_via_git(skills_dir: &Path, spec: &RepoSpec, force: bool, branch: &str) -> Result<()> {
    // Clone to a temp directory
    let tmp = tempfile::tempdir().wrap_err("failed to create temp directory")?;
    let clone_dir = tmp.path().join(&spec.repo);

    println!(
        "Cloning {} (branch: {})...",
        spec.clone_url().dimmed(),
        branch.cyan()
    );

    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--branch", branch])
        .arg(spec.clone_url())
        .arg(&clone_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .map_err(|_| eyre::eyre!("git not found. Please install git."))?;

    if !status.success() {
        eyre::bail!(
            "git clone failed. Check the repo path: {}/{}",
            spec.user,
            spec.repo
        );
    }

    std::fs::create_dir_all(skills_dir)?;

    let mut installed = Vec::new();
    let mut skipped = Vec::new();
    let mut deps_installed = Vec::new();

    if let Some(subdir) = &spec.subdir {
        // Targeted install: just one subdirectory + shared deps
        let src = clone_dir.join(subdir);
        if !src.is_dir() {
            eyre::bail!(
                "Subdirectory '{subdir}' not found in {}/{}",
                spec.user,
                spec.repo
            );
        }

        let name = Path::new(subdir)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        // Install the target skill/dir
        let dest = skills_dir.join(&name);
        if dest.exists() && !force {
            println!(
                "  {} '{}' already exists (use --force to overwrite)",
                "SKIP".yellow(),
                name
            );
            skipped.push(name.clone());
        } else {
            if dest.exists() {
                std::fs::remove_dir_all(&dest)?;
            }
            copy_dir_recursive(&src, &dest)?;
            if src.join("SKILL.md").exists() {
                installed.push(name.clone());
            } else {
                deps_installed.push(name.clone());
            }
        }

        // Auto-detect shared dependencies referenced in SKILL.md
        let skill_md_path = src.join("SKILL.md");
        if skill_md_path.exists() {
            let content = std::fs::read_to_string(&skill_md_path)?;
            let shared = find_shared_deps(&content, &clone_dir, &name);
            for dep in shared {
                let dep_src = clone_dir.join(&dep);
                let dep_dest = skills_dir.join(&dep);
                if dep_dest.exists() && !force {
                    println!(
                        "  {} dependency '{}' already exists (use --force to overwrite)",
                        "SKIP".yellow(),
                        dep
                    );
                } else {
                    if dep_dest.exists() {
                        std::fs::remove_dir_all(&dep_dest)?;
                    }
                    copy_dir_recursive(&dep_src, &dep_dest)?;
                    deps_installed.push(dep);
                }
            }
        }
    } else {
        // Whole-repo install: check if root is a single skill or multi-skill
        if clone_dir.join("SKILL.md").exists() {
            // Single-skill repo: install as repo_name/
            let dest = skills_dir.join(&spec.repo);
            if dest.exists() && !force {
                println!(
                    "  {} '{}' already exists (use --force to overwrite)",
                    "SKIP".yellow(),
                    spec.repo
                );
                skipped.push(spec.repo.clone());
            } else {
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)?;
                }
                copy_dir_recursive(&clone_dir, &dest)?;
                installed.push(spec.repo.clone());
            }
        } else {
            // Multi-skill repo: copy all top-level directories
            for entry in std::fs::read_dir(&clone_dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }

                let dest = skills_dir.join(&name);
                if dest.exists() && !force {
                    println!(
                        "  {} '{}' already exists (use --force to overwrite)",
                        "SKIP".yellow(),
                        name
                    );
                    skipped.push(name);
                    continue;
                }
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)?;
                }
                copy_dir_recursive(&entry.path(), &dest)?;

                if entry.path().join("SKILL.md").exists() {
                    installed.push(name);
                } else {
                    deps_installed.push(name);
                }
            }
        }
    }

    // Run npm install in any installed dirs that have package.json
    for name in installed.iter().chain(deps_installed.iter()) {
        let dir = skills_dir.join(name);
        maybe_npm_install(&dir)?;
    }

    // Summary
    println!();
    if !installed.is_empty() {
        println!(
            "{} Installed {} skill(s): {}",
            "OK".green(),
            installed.len(),
            installed.join(", ").cyan()
        );
    }
    if !deps_installed.is_empty() {
        println!(
            "{} Installed {} shared dep(s): {}",
            "OK".green(),
            deps_installed.len(),
            deps_installed.join(", ").dimmed()
        );
    }
    if !skipped.is_empty() {
        println!(
            "{} Skipped {} existing: {}",
            "SKIP".yellow(),
            skipped.len(),
            skipped.join(", ")
        );
    }
    if installed.is_empty() && deps_installed.is_empty() && skipped.is_empty() {
        println!("{} No skills found in repository", "WARN".yellow());
    }

    Ok(())
}

/// HTTP fallback: fetch a single SKILL.md (original behavior).
fn install_via_http(skills_dir: &Path, spec: &RepoSpec, force: bool, branch: &str) -> Result<()> {
    let subdir = spec.subdir.as_deref().unwrap_or(&spec.repo);
    let name = subdir.rsplit('/').next().unwrap();

    let dest = skills_dir.join(name);
    if dest.exists() && !force {
        eyre::bail!(
            "Skill '{name}' already exists at {} (use --force to overwrite)",
            dest.display()
        );
    }

    let url = format!(
        "https://raw.githubusercontent.com/{}/{}/{branch}/{subdir}/SKILL.md",
        spec.user, spec.repo
    );
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
            "Failed to fetch SKILL.md (HTTP {}). Check the repo path: {}",
            body.status(),
            spec.subdir.as_deref().unwrap_or("")
        );
    }

    let content = body.text().wrap_err("failed to read response body")?;

    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
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

/// Recursively copy a directory, skipping .git and node_modules.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "node_modules" {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Scan SKILL.md for references to sibling directories (shared deps).
/// Looks for patterns like `~/.crew/skills/XXX/` where XXX is a sibling dir in the clone.
fn find_shared_deps(skill_md: &str, clone_dir: &Path, self_name: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let re = regex::Regex::new(r"~/.crew/skills/([a-zA-Z0-9_-]+)/").unwrap();
    for cap in re.captures_iter(skill_md) {
        let dep_name = cap[1].to_string();
        if dep_name == self_name {
            continue;
        }
        // Only include if this dir actually exists in the cloned repo
        if clone_dir.join(&dep_name).is_dir() && !deps.contains(&dep_name) {
            deps.push(dep_name);
        }
    }
    deps
}

/// Run `npm install` if `package.json` exists but `node_modules/` does not.
fn maybe_npm_install(dir: &Path) -> Result<()> {
    if !dir.join("package.json").exists() {
        return Ok(());
    }
    if dir.join("node_modules").exists() {
        return Ok(());
    }
    println!(
        "  Running {} in {}...",
        "npm install".cyan(),
        dir.file_name().unwrap().to_string_lossy()
    );
    let status = std::process::Command::new("npm")
        .arg("install")
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .map_err(|_| {
            eyre::eyre!("npm not found. Please install Node.js to set up dependencies.")
        })?;
    if !status.success() {
        eyre::bail!("npm install failed in {}", dir.display());
    }
    Ok(())
}

fn cmd_remove(skills_dir: &Path, name: &str) -> Result<()> {
    let dest = skills_dir.join(name);
    if !dest.exists() {
        eyre::bail!("Skill '{name}' not found in {}", skills_dir.display());
    }

    std::fs::remove_dir_all(&dest)?;
    println!("{} Removed skill '{}'", "OK".green(), name.cyan());
    Ok(())
}
