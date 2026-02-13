//! Sandboxing for shell command execution.
//!
//! Provides platform-specific isolation: bubblewrap on Linux,
//! sandbox-exec on macOS, or no sandbox (pass-through).

use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Sandbox configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Whether sandboxing is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Sandbox mode (auto-detect by default).
    #[serde(default)]
    pub mode: SandboxMode,

    /// Allow network access inside the sandbox.
    #[serde(default)]
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SandboxMode::Auto,
            allow_network: false,
        }
    }
}

/// Which sandbox backend to use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Auto-detect: bwrap on Linux, sandbox-exec on macOS, none elsewhere.
    #[default]
    Auto,
    /// Linux bubblewrap.
    Bwrap,
    /// macOS sandbox-exec.
    Macos,
    /// No sandboxing (pass-through).
    None,
}

/// Trait for wrapping shell commands in a sandbox.
pub trait Sandbox: Send + Sync {
    /// Wrap a shell command string into a sandboxed `Command`.
    fn wrap_command(&self, shell_command: &str, cwd: &Path) -> Command;
}

/// No-op sandbox: executes commands directly.
pub struct NoSandbox;

impl Sandbox for NoSandbox {
    fn wrap_command(&self, shell_command: &str, cwd: &Path) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(shell_command).current_dir(cwd);
        cmd
    }
}

/// Linux sandbox using bubblewrap (bwrap).
pub struct BwrapSandbox {
    allow_network: bool,
}

impl Sandbox for BwrapSandbox {
    fn wrap_command(&self, shell_command: &str, cwd: &Path) -> Command {
        let mut cmd = Command::new("bwrap");

        // Read-only bind system directories
        for dir in &["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"] {
            if Path::new(dir).exists() {
                cmd.arg("--ro-bind").arg(dir).arg(dir);
            }
        }

        // Read-write bind the working directory
        let cwd_str = cwd.to_string_lossy();
        cmd.arg("--bind").arg(&*cwd_str).arg(&*cwd_str);

        // Bind /tmp for scratch space
        cmd.arg("--tmpfs").arg("/tmp");

        // /dev minimal
        cmd.arg("--dev").arg("/dev");
        cmd.arg("--proc").arg("/proc");

        if !self.allow_network {
            cmd.arg("--unshare-net");
        }

        cmd.arg("--unshare-pid");
        cmd.arg("--die-with-parent");
        cmd.arg("--chdir").arg(&*cwd_str);
        cmd.arg("--").arg("sh").arg("-c").arg(shell_command);

        cmd
    }
}

/// macOS sandbox using sandbox-exec.
pub struct MacosSandbox {
    allow_network: bool,
}

impl Sandbox for MacosSandbox {
    fn wrap_command(&self, shell_command: &str, cwd: &Path) -> Command {
        let cwd_str = cwd.to_string_lossy();

        // Reject paths with control characters to prevent SBPL profile injection
        if cwd_str.bytes().any(|b| b < 0x20) {
            tracing::warn!("cwd contains control characters, falling back to unsandboxed");
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(shell_command).current_dir(cwd);
            return cmd;
        }

        // Escape characters that could break the SBPL sandbox profile syntax
        let cwd_escaped = cwd_str.replace('\\', "\\\\").replace('"', "\\\"");

        let network_rule = if self.allow_network {
            "(allow network*)"
        } else {
            "(deny network*)"
        };

        let profile = format!(
            r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow sysctl-read)
(allow file-read*)
(allow file-write* (subpath "{cwd}"))
(allow file-write* (subpath "/private/tmp"))
(allow file-write* (subpath "/private/var/folders"))
{network_rule}
"#,
            cwd = cwd_escaped,
            network_rule = network_rule,
        );

        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-p")
            .arg(profile)
            .arg("sh")
            .arg("-c")
            .arg(shell_command)
            .current_dir(cwd);
        cmd
    }
}

/// Create a sandbox from config.
pub fn create_sandbox(config: &SandboxConfig) -> Box<dyn Sandbox> {
    if !config.enabled {
        tracing::info!("sandbox disabled, shell commands run without isolation");
        return Box::new(NoSandbox);
    }

    match &config.mode {
        SandboxMode::None => Box::new(NoSandbox),
        SandboxMode::Bwrap => Box::new(BwrapSandbox {
            allow_network: config.allow_network,
        }),
        SandboxMode::Macos => Box::new(MacosSandbox {
            allow_network: config.allow_network,
        }),
        SandboxMode::Auto => {
            if cfg!(target_os = "linux") && which_exists("bwrap") {
                Box::new(BwrapSandbox {
                    allow_network: config.allow_network,
                })
            } else if cfg!(target_os = "macos") && which_exists("sandbox-exec") {
                Box::new(MacosSandbox {
                    allow_network: config.allow_network,
                })
            } else {
                Box::new(NoSandbox)
            }
        }
    }
}

/// Check if a binary exists on PATH.
fn which_exists(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_sandbox_wraps_directly() {
        let sb = NoSandbox;
        let cmd = sb.wrap_command("echo hello", Path::new("/tmp"));
        // Command should be `sh -c "echo hello"` in /tmp
        let prog = cmd.as_std().get_program().to_string_lossy().to_string();
        assert_eq!(prog, "sh");
    }

    #[test]
    fn test_create_sandbox_disabled() {
        let config = SandboxConfig::default();
        let sb = create_sandbox(&config);
        // Should be NoSandbox — just verify it doesn't panic
        let _cmd = sb.wrap_command("ls", Path::new("/tmp"));
    }

    #[test]
    fn test_bwrap_sandbox_command() {
        let sb = BwrapSandbox {
            allow_network: false,
        };
        let cmd = sb.wrap_command("echo hi", Path::new("/tmp/test"));
        let prog = cmd.as_std().get_program().to_string_lossy().to_string();
        assert_eq!(prog, "bwrap");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"--unshare-net".to_string()));
        assert!(args.contains(&"echo hi".to_string()));
    }

    #[test]
    fn test_macos_sandbox_command() {
        let sb = MacosSandbox {
            allow_network: true,
        };
        let cmd = sb.wrap_command("echo hi", Path::new("/tmp/test"));
        let prog = cmd.as_std().get_program().to_string_lossy().to_string();
        assert_eq!(prog, "sandbox-exec");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.iter().any(|a| a.contains("allow network")));
    }
}
