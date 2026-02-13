//! Plugin loader: scans directories for plugins and registers their tools.

use std::path::{Path, PathBuf};

use eyre::Result;
use tracing::{info, warn};

use crate::tools::ToolRegistry;

use super::manifest::PluginManifest;
use super::tool::PluginTool;

/// Scans plugin directories and registers discovered tools.
pub struct PluginLoader;

impl PluginLoader {
    /// Scan directories for plugins and register tools into the registry.
    ///
    /// Each plugin is a directory containing:
    /// - `manifest.json` — plugin metadata and tool definitions
    /// - An executable file (same name as directory, or `main`)
    ///
    /// Returns the number of tools registered.
    pub fn load_into(registry: &mut ToolRegistry, dirs: &[PathBuf]) -> Result<usize> {
        let mut count = 0;

        for dir in dirs {
            if !dir.exists() {
                continue;
            }

            let entries = std::fs::read_dir(dir)?;
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                match Self::load_plugin(&path) {
                    Ok(tools) => {
                        let n = tools.len();
                        for tool in tools {
                            registry.register(tool);
                        }
                        count += n;
                    }
                    Err(e) => {
                        warn!(
                            plugin_dir = %path.display(),
                            error = %e,
                            "failed to load plugin, skipping"
                        );
                    }
                }
            }
        }

        if count > 0 {
            info!(tools = count, "loaded plugin tools");
        }

        Ok(count)
    }

    fn load_plugin(plugin_dir: &Path) -> Result<Vec<PluginTool>> {
        let manifest_path = plugin_dir.join("manifest.json");
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| eyre::eyre!("no manifest.json: {e}"))?;
        let manifest: PluginManifest = serde_json::from_str(&content)
            .map_err(|e| eyre::eyre!("invalid manifest.json: {e}"))?;

        // Find executable: try plugin name, then "main"
        let dir_name = plugin_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("main");
        let executable = [dir_name, "main"]
            .iter()
            .map(|name| plugin_dir.join(name))
            .find(|p| p.exists() && is_executable(p))
            .ok_or_else(|| {
                eyre::eyre!(
                    "no executable found in plugin '{}' (tried '{}', 'main')",
                    manifest.name,
                    dir_name
                )
            })?;

        warn!(
            plugin = %manifest.name,
            version = %manifest.version,
            tools = manifest.tools.len(),
            executable = %executable.display(),
            "loaded unverified plugin (no signature check)"
        );

        let tools = manifest
            .tools
            .into_iter()
            .map(|def| PluginTool::new(manifest.name.clone(), def, executable.clone()))
            .collect();

        Ok(tools)
    }
}

/// Check if a path is executable (Unix).
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// On non-Unix, just check existence.
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_nonexistent_dir() {
        let mut registry = ToolRegistry::new();
        let result = PluginLoader::load_into(&mut registry, &[PathBuf::from("/nonexistent/path")]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_load_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        let count = PluginLoader::load_into(&mut registry, &[dir.path().to_path_buf()]).unwrap();
        assert_eq!(count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn test_load_plugin_with_manifest() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        // Write manifest
        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{"name": "my-plugin", "version": "1.0", "tools": [{"name": "greet", "description": "Greet someone"}]}"#,
        ).unwrap();

        // Write executable
        let exec_path = plugin_dir.join("my-plugin");
        std::fs::write(
            &exec_path,
            "#!/bin/sh\necho '{\"output\": \"hi\", \"success\": true}'",
        )
        .unwrap();
        std::fs::set_permissions(&exec_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut registry = ToolRegistry::new();
        let count = PluginLoader::load_into(&mut registry, &[dir.path().to_path_buf()]).unwrap();
        assert_eq!(count, 1);
        assert_eq!(registry.len(), 1);
    }
}
