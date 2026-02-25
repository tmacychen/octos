//! Gateway child process lifecycle management.
//!
//! Spawns `crew gateway` as child processes, monitors their output, and
//! provides start/stop/status/log-streaming capabilities.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use crew_agent::sandbox::BLOCKED_ENV_VARS;
use eyre::{Result, bail};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, watch};

use crate::profiles::{ProfileStore, UserProfile};

/// Manages gateway child processes — one per user profile.
pub struct ProcessManager {
    processes: Arc<RwLock<HashMap<String, GatewayProcess>>>,
    profile_store: Arc<ProfileStore>,
}

struct GatewayProcess {
    pid: u32,
    started_at: DateTime<Utc>,
    log_tx: broadcast::Sender<String>,
    stop_tx: watch::Sender<bool>,
}

/// Status of a gateway process.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub started_at: Option<String>,
    pub uptime_secs: Option<i64>,
}

impl ProcessManager {
    /// Create a new process manager backed by the given profile store.
    pub fn new(profile_store: Arc<ProfileStore>) -> Self {
        Self {
            processes: Arc::new(RwLock::new(HashMap::new())),
            profile_store,
        }
    }

    /// Start the gateway for a profile. Returns an error if already running.
    pub async fn start(&self, profile: &UserProfile) -> Result<()> {
        // Hold the write lock for the entire operation to prevent TOCTOU races.
        let mut procs = self.processes.write().await;
        if procs.contains_key(&profile.id) {
            bail!("gateway for '{}' is already running", profile.id);
        }

        // Generate config file for the gateway
        let config_path = self.profile_store.generate_config(profile)?;
        let data_dir = self.profile_store.resolve_data_dir(profile);
        for sub in ["memory", "sessions", "research", "skills", "history"] {
            std::fs::create_dir_all(data_dir.join(sub))?;
        }

        // Spawn the gateway as a child process
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(&exe);
        cmd.arg("gateway")
            .arg("--config")
            .arg(&config_path)
            .arg("--data-dir")
            .arg(&data_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Pass env vars from profile config, filtering out dangerous ones.
        for (key, value) in &profile.config.env_vars {
            if BLOCKED_ENV_VARS
                .iter()
                .any(|blocked| key.eq_ignore_ascii_case(blocked))
            {
                tracing::warn!(
                    profile = %profile.id,
                    var = %key,
                    "skipping blocked environment variable"
                );
                continue;
            }
            cmd.env(key, value);
        }

        let mut child = cmd.spawn()?;

        let pid = child.id().unwrap_or(0);
        let (log_tx, _) = broadcast::channel::<String>(1024);
        let (stop_tx, stop_rx) = watch::channel(false);

        let process = GatewayProcess {
            pid,
            started_at: Utc::now(),
            log_tx: log_tx.clone(),
            stop_tx,
        };

        procs.insert(profile.id.clone(), process);
        // Release the write lock now that the entry is inserted.
        drop(procs);

        // Spawn task to read stdout and forward to log channel
        if let Some(stdout) = child.stdout.take() {
            let tx = log_tx.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(line);
                }
            });
        }

        // Spawn task to read stderr and forward to log channel
        if let Some(stderr) = child.stderr.take() {
            let tx = log_tx.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(format!("[stderr] {line}"));
                }
            });
        }

        // Spawn task to wait for process exit or stop signal.
        // On natural exit, removes the entry from the HashMap so status is correct.
        let profile_id = profile.id.clone();
        let processes = Arc::clone(&self.processes);
        tokio::spawn(async move {
            let mut stop_rx = stop_rx;
            tokio::select! {
                status = child.wait() => {
                    // Child exited on its own — clean up the HashMap entry.
                    processes.write().await.remove(&profile_id);
                    match status {
                        Ok(s) => tracing::info!(profile = %profile_id, exit = %s, "gateway exited"),
                        Err(e) => tracing::error!(profile = %profile_id, error = %e, "gateway error"),
                    }
                }
                _ = stop_rx.changed() => {
                    // Stop signal received — kill the child
                    let _ = child.kill().await;
                    tracing::info!(profile = %profile_id, "gateway stopped");
                }
            }
        });

        tracing::info!(profile = %profile.id, pid = pid, "gateway started");
        Ok(())
    }

    /// Stop the gateway for a profile. Returns false if not running.
    pub async fn stop(&self, profile_id: &str) -> Result<bool> {
        let process = {
            let mut procs = self.processes.write().await;
            procs.remove(profile_id)
        };

        match process {
            Some(proc) => {
                let _ = proc.stop_tx.send(true);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Restart a gateway (stop then start).
    pub async fn restart(&self, profile: &UserProfile) -> Result<()> {
        let _ = self.stop(&profile.id).await;
        // Small delay to let the process clean up
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        self.start(profile).await
    }

    /// Get the status of a gateway process.
    pub async fn status(&self, profile_id: &str) -> ProcessStatus {
        let procs = self.processes.read().await;
        match procs.get(profile_id) {
            Some(proc) => {
                let uptime = Utc::now() - proc.started_at;
                ProcessStatus {
                    running: true,
                    pid: Some(proc.pid),
                    started_at: Some(proc.started_at.to_rfc3339()),
                    uptime_secs: Some(uptime.num_seconds()),
                }
            }
            None => ProcessStatus {
                running: false,
                pid: None,
                started_at: None,
                uptime_secs: None,
            },
        }
    }

    /// Subscribe to log output for a profile. Returns None if not running.
    pub async fn subscribe_logs(&self, profile_id: &str) -> Option<broadcast::Receiver<String>> {
        let procs = self.processes.read().await;
        procs.get(profile_id).map(|p| p.log_tx.subscribe())
    }

    /// Get the status of all profiles.
    pub async fn all_statuses(&self) -> HashMap<String, ProcessStatus> {
        let procs = self.processes.read().await;
        let mut statuses = HashMap::new();
        for (id, proc) in procs.iter() {
            let uptime = Utc::now() - proc.started_at;
            statuses.insert(
                id.clone(),
                ProcessStatus {
                    running: true,
                    pid: Some(proc.pid),
                    started_at: Some(proc.started_at.to_rfc3339()),
                    uptime_secs: Some(uptime.num_seconds()),
                },
            );
        }
        statuses
    }

    /// Stop all running gateways.
    pub async fn stop_all(&self) -> usize {
        let processes: HashMap<String, GatewayProcess> = {
            let mut procs = self.processes.write().await;
            std::mem::take(&mut *procs)
        };
        let count = processes.len();
        for (id, proc) in processes {
            let _ = proc.stop_tx.send(true);
            tracing::info!(profile = %id, "gateway stopped");
        }
        count
    }

    /// Get the data directory path for a profile.
    pub fn resolve_data_dir(&self, profile: &UserProfile) -> PathBuf {
        self.profile_store.resolve_data_dir(profile)
    }
}
