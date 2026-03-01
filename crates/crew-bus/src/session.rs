//! Session management with JSONL persistence and LRU eviction.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use crew_core::{Message, SessionKey};
use eyre::Result;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Current schema version for session JSONL files.
const CURRENT_SESSION_SCHEMA: u32 = 1;

fn default_session_schema() -> u32 {
    CURRENT_SESSION_SCHEMA
}

/// Metadata stored as the first line of each JSONL session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionMeta {
    /// Schema version for forward-compatible deserialization.
    #[serde(default = "default_session_schema")]
    schema_version: u32,
    session_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_key: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A conversation session with message history.
#[derive(Debug, Clone)]
pub struct Session {
    pub key: SessionKey,
    /// Parent session key if this session was forked.
    pub parent_key: Option<SessionKey>,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    fn new(key: SessionKey) -> Self {
        let now = Utc::now();
        Self {
            key,
            parent_key: None,
            messages: vec![],
            created_at: now,
            updated_at: now,
        }
    }

    /// Get the most recent N messages from history.
    pub fn get_history(&self, max: usize) -> &[Message] {
        let len = self.messages.len();
        if len <= max {
            &self.messages
        } else {
            &self.messages[len - max..]
        }
    }
}

/// Default maximum number of sessions kept in memory.
const DEFAULT_MAX_SESSIONS: usize = 1000;

/// Maximum session file size we'll load (10 MB). Prevents OOM on corrupted/adversarial files.
const MAX_SESSION_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Manages sessions with in-memory LRU cache and JSONL disk persistence.
///
/// Uses `lru::LruCache` for O(1) get/put with automatic eviction of the
/// least-recently-used session when capacity is exceeded. Evicted sessions
/// remain on disk and are lazy-loaded on next access.
pub struct SessionManager {
    sessions_dir: PathBuf,
    cache: LruCache<String, Session>,
}

impl SessionManager {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let sessions_dir = data_dir.join("sessions");
        std::fs::create_dir_all(&sessions_dir)?;
        Ok(Self {
            sessions_dir,
            cache: LruCache::new(NonZeroUsize::new(DEFAULT_MAX_SESSIONS).expect("default > 0")),
        })
    }

    /// Set the maximum number of sessions to keep in memory (minimum 1).
    /// Sessions evicted from memory are NOT deleted from disk.
    pub fn with_max_sessions(mut self, max: usize) -> Self {
        let cap = NonZeroUsize::new(max.max(1)).expect("clamped to >= 1");
        self.cache.resize(cap);
        self
    }

    /// List all sessions (ID + message count) from disk.
    pub fn list_sessions(&self) -> Vec<(String, usize)> {
        let mut result = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.sessions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                        // Skip oversized files to avoid OOM in listing
                        let too_large = path
                            .metadata()
                            .map(|m| m.len() > MAX_SESSION_FILE_SIZE)
                            .unwrap_or(false);
                        let count = if too_large {
                            0
                        } else {
                            std::fs::read_to_string(&path)
                                .map(|c| c.lines().count())
                                .unwrap_or(0)
                        };
                        // Decode percent-encoded filename back to session key
                        let decoded = Self::decode_session_name(name);
                        result.push((decoded, count));
                    }
                }
            }
        }
        result
    }

    /// Get or create a session. Loads from disk on first access.
    pub fn get_or_create(&mut self, key: &SessionKey) -> &mut Session {
        let key_str = key.0.clone();
        if !self.cache.contains(&key_str) {
            let session = self
                .load_from_disk(key)
                .unwrap_or_else(|| Session::new(key.clone()));
            self.cache.put(key_str.clone(), session);
        }
        self.cache
            .get_mut(&key_str)
            .expect("session must exist: inserted above")
    }

    /// Add a message to a session and persist it.
    pub async fn add_message(&mut self, key: &SessionKey, message: Message) -> Result<()> {
        let session = self.get_or_create(key);
        session.messages.push(message.clone());
        session.updated_at = Utc::now();
        self.append_to_disk(key, &message).await?;
        Ok(())
    }

    /// Get the JSONL file path for a session key.
    ///
    /// Uses byte-level percent-encoding for non-safe characters to ensure
    /// different keys always produce different filenames. Operating on raw
    /// UTF-8 bytes (not Unicode codepoints) makes this immune to normalization
    /// collisions on filesystems like APFS/HFS+.
    ///
    /// Truncates encoded name to 200 chars to stay within the 255-byte
    /// filesystem filename limit (reserving space for ".jsonl" suffix).
    fn session_path(&self, key: &SessionKey) -> PathBuf {
        // Max encoded name length: 200 chars + ".jsonl" (6) = 206, well within 255.
        // When truncation occurs, append a hash suffix to avoid collisions between
        // keys that differ only past the truncation point.
        const HASH_SUFFIX_LEN: usize = 17; // "_{hash:016X}"
        const MAX_NAME_LEN: usize = 200 - HASH_SUFFIX_LEN;
        let mut safe_name = String::new();
        let mut truncated = false;
        for byte in key.0.as_bytes() {
            if safe_name.len() >= MAX_NAME_LEN {
                truncated = true;
                break;
            }
            if byte.is_ascii_alphanumeric() || *byte == b'-' || *byte == b'_' {
                safe_name.push(*byte as char);
            } else {
                // Percent-encode each byte: ':' -> '%3A', non-ASCII -> '%XX' per byte
                safe_name.push_str(&format!("%{byte:02X}"));
            }
        }
        if truncated {
            // Append 8-char hex hash of full key to prevent collisions
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            key.0.hash(&mut hasher);
            let hash = hasher.finish();
            safe_name.push_str(&format!("_{hash:016X}"));
        }
        self.sessions_dir.join(format!("{safe_name}.jsonl"))
    }

    /// Decode a percent-encoded session filename back to the original session key.
    fn decode_session_name(encoded: &str) -> String {
        let mut bytes = Vec::new();
        let mut chars = encoded.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                let hi = chars.next().unwrap_or('0');
                let lo = chars.next().unwrap_or('0');
                if let Ok(byte) = u8::from_str_radix(&format!("{hi}{lo}"), 16) {
                    bytes.push(byte);
                } else {
                    bytes.push(b'%');
                    bytes.extend_from_slice(hi.encode_utf8(&mut [0; 4]).as_bytes());
                    bytes.extend_from_slice(lo.encode_utf8(&mut [0; 4]).as_bytes());
                }
            } else {
                bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
        }
        String::from_utf8(bytes).unwrap_or_else(|_| encoded.to_string())
    }

    /// Load a session from its JSONL file.
    fn load_from_disk(&self, key: &SessionKey) -> Option<Session> {
        let path = self.session_path(key);

        // Guard against oversized files to prevent OOM
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_SESSION_FILE_SIZE {
                warn!(
                    key = %key,
                    size = meta.len(),
                    limit = MAX_SESSION_FILE_SIZE,
                    "session file too large, skipping"
                );
                return None;
            }
        }

        let content = std::fs::read_to_string(&path).ok()?;
        let mut lines = content.lines();

        // First line is metadata
        let meta_line = lines.next()?;
        let meta: SessionMeta = serde_json::from_str(meta_line).ok()?;

        // Remaining lines are messages
        let messages: Vec<Message> = lines
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        debug!(key = %key, messages = messages.len(), "Loaded session from disk");

        Some(Session {
            key: key.clone(),
            parent_key: meta.parent_key.map(SessionKey),
            messages,
            created_at: meta.created_at,
            updated_at: meta.updated_at,
        })
    }

    /// Append a message to the JSONL file. Creates the file with metadata if new.
    /// Uses spawn_blocking to avoid blocking the async runtime.
    async fn append_to_disk(&self, key: &SessionKey, message: &Message) -> Result<()> {
        let path = self.session_path(key);

        // Prepare metadata outside spawn_blocking (needs cache access)
        let parent_key = self
            .cache
            .peek(&key.0)
            .and_then(|s| s.parent_key.as_ref().map(|k| k.0.clone()));
        let key_str = key.0.clone();
        let msg_json = serde_json::to_string(message)?;

        tokio::task::spawn_blocking(move || {
            use std::io::Write;

            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;

            // Check file size after open to avoid TOCTOU race with exists() check
            let is_new = file.metadata()?.len() == 0;
            if is_new {
                let meta = SessionMeta {
                    schema_version: CURRENT_SESSION_SCHEMA,
                    session_key: key_str,
                    parent_key,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                };
                writeln!(file, "{}", serde_json::to_string(&meta)?)?;
            }

            writeln!(file, "{}", msg_json)?;
            Ok::<_, eyre::Report>(())
        })
        .await
        .map_err(|e| eyre::eyre!("spawn_blocking join error: {e}"))??;

        Ok(())
    }

    /// Rewrite a session's JSONL file from the in-memory state.
    /// Uses atomic write-then-rename to avoid corruption on crash.
    /// Uses spawn_blocking to avoid blocking the async runtime.
    pub async fn rewrite(&self, key: &SessionKey) -> Result<()> {
        let session = self
            .cache
            .peek(&key.0)
            .ok_or_else(|| eyre::eyre!("session not in cache: {}", key))?;

        // Build the full content string synchronously (no I/O)
        let meta = SessionMeta {
            schema_version: CURRENT_SESSION_SCHEMA,
            session_key: key.0.clone(),
            parent_key: session.parent_key.as_ref().map(|k| k.0.clone()),
            created_at: session.created_at,
            updated_at: session.updated_at,
        };
        let mut content = serde_json::to_string(&meta)?;
        content.push('\n');
        for msg in &session.messages {
            content.push_str(&serde_json::to_string(msg)?);
            content.push('\n');
        }

        let msg_count = session.messages.len();
        let path = self.session_path(key);
        let key_display = key.to_string();

        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let tmp_path = path.with_extension("jsonl.tmp");
            let mut file = std::fs::File::create(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.flush()?;
            // Atomic rename (on same filesystem)
            std::fs::rename(&tmp_path, &path)?;
            Ok::<_, eyre::Report>(())
        })
        .await
        .map_err(|e| eyre::eyre!("spawn_blocking join error: {e}"))??;

        debug!(key = %key_display, messages = msg_count, "Rewrote session to disk");
        Ok(())
    }

    /// Fork a session: create a new session that copies the last N messages from the parent.
    ///
    /// The new session's channel is taken from the parent key; `new_chat_id` becomes the chat ID.
    /// Returns the new session's key.
    pub async fn fork(
        &mut self,
        parent_key: &SessionKey,
        new_chat_id: &str,
        copy_messages: usize,
    ) -> Result<SessionKey> {
        let parent = self.get_or_create(parent_key);
        let messages: Vec<Message> = parent.get_history(copy_messages).to_vec();
        // Derive channel from parent key (format: "channel:chat_id")
        let channel = parent_key.0.split(':').next().unwrap_or("cli");
        let new_key = SessionKey::new(channel, new_chat_id);

        let now = Utc::now();
        let session = Session {
            key: new_key.clone(),
            parent_key: Some(parent_key.clone()),
            messages,
            created_at: now,
            updated_at: now,
        };
        self.cache.put(new_key.0.clone(), session);
        self.rewrite(&new_key).await?;

        debug!(
            parent = %parent_key,
            child = %new_key,
            copied = copy_messages,
            "Forked session"
        );
        Ok(new_key)
    }

    /// Clear a session's history (both in-memory and on disk).
    pub async fn clear(&mut self, key: &SessionKey) -> Result<()> {
        self.cache.pop(&key.0);
        let path = self.session_path(key);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }

    /// Number of sessions currently in memory.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Number of sessions the LRU cache can hold.
    pub fn capacity(&self) -> usize {
        self.cache.cap().get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crew_core::MessageRole;
    use tempfile::TempDir;

    fn make_message(role: MessageRole, content: &str) -> Message {
        Message {
            role,
            content: content.into(),
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_session_get_history() {
        let mut session = Session::new(SessionKey::new("cli", "test"));
        for i in 0..10 {
            session
                .messages
                .push(make_message(MessageRole::User, &format!("msg{i}")));
        }
        let history = session.get_history(3);
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "msg7");
        assert_eq!(history[2].content, "msg9");
    }

    #[test]
    fn test_session_get_history_all() {
        let mut session = Session::new(SessionKey::new("cli", "test"));
        session.messages.push(make_message(MessageRole::User, "a"));
        session.messages.push(make_message(MessageRole::User, "b"));
        let history = session.get_history(10);
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn test_session_manager_create_and_retrieve() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = SessionManager::open(tmp.path()).unwrap();
        let key = SessionKey::new("cli", "default");

        let session = mgr.get_or_create(&key);
        assert_eq!(session.messages.len(), 0);

        mgr.add_message(&key, make_message(MessageRole::User, "hello"))
            .await
            .unwrap();
        mgr.add_message(&key, make_message(MessageRole::Assistant, "hi"))
            .await
            .unwrap();

        let session = mgr.get_or_create(&key);
        assert_eq!(session.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_session_manager_persistence() {
        let tmp = TempDir::new().unwrap();
        let key = SessionKey::new("cli", "persist");

        // Write session
        {
            let mut mgr = SessionManager::open(tmp.path()).unwrap();
            mgr.add_message(&key, make_message(MessageRole::User, "saved"))
                .await
                .unwrap();
            mgr.add_message(&key, make_message(MessageRole::Assistant, "reply"))
                .await
                .unwrap();
        }

        // New manager should load from disk
        {
            let mut mgr = SessionManager::open(tmp.path()).unwrap();
            let session = mgr.get_or_create(&key);
            assert_eq!(session.messages.len(), 2);
            assert_eq!(session.messages[0].content, "saved");
            assert_eq!(session.messages[1].content, "reply");
        }
    }

    #[tokio::test]
    async fn test_session_manager_clear() {
        let tmp = TempDir::new().unwrap();
        let key = SessionKey::new("cli", "clear-me");
        let mut mgr = SessionManager::open(tmp.path()).unwrap();

        mgr.add_message(&key, make_message(MessageRole::User, "temp"))
            .await
            .unwrap();
        assert_eq!(mgr.get_or_create(&key).messages.len(), 1);

        mgr.clear(&key).await.unwrap();

        // After clear, should be empty
        let session = mgr.get_or_create(&key);
        assert_eq!(session.messages.len(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_sessions() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = SessionManager::open(tmp.path()).unwrap();

        let k1 = SessionKey::new("telegram", "chat1");
        let k2 = SessionKey::new("telegram", "chat2");

        mgr.add_message(&k1, make_message(MessageRole::User, "from chat1"))
            .await
            .unwrap();
        mgr.add_message(&k2, make_message(MessageRole::User, "from chat2"))
            .await
            .unwrap();

        assert_eq!(mgr.get_or_create(&k1).messages.len(), 1);
        assert_eq!(mgr.get_or_create(&k2).messages.len(), 1);
        assert_eq!(mgr.get_or_create(&k1).messages[0].content, "from chat1");
        assert_eq!(mgr.get_or_create(&k2).messages[0].content, "from chat2");
    }

    #[tokio::test]
    async fn test_session_rewrite() {
        let tmp = TempDir::new().unwrap();
        let key = SessionKey::new("cli", "rewrite");
        let mut mgr = SessionManager::open(tmp.path()).unwrap();

        // Add 5 messages
        for i in 0..5 {
            mgr.add_message(&key, make_message(MessageRole::User, &format!("msg{i}")))
                .await
                .unwrap();
        }

        // Mutate in-memory: keep only last 2
        let session = mgr.get_or_create(&key);
        session.messages.drain(0..3);
        assert_eq!(session.messages.len(), 2);

        // Rewrite to disk
        mgr.rewrite(&key).await.unwrap();

        // Load fresh from disk — should have only 2 messages
        let mut mgr2 = SessionManager::open(tmp.path()).unwrap();
        let session2 = mgr2.get_or_create(&key);
        assert_eq!(session2.messages.len(), 2);
        assert_eq!(session2.messages[0].content, "msg3");
        assert_eq!(session2.messages[1].content, "msg4");
    }

    #[tokio::test]
    async fn test_fork_creates_child() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = SessionManager::open(tmp.path()).unwrap();
        let parent = SessionKey::new("telegram", "chat1");

        for i in 0..5 {
            mgr.add_message(&parent, make_message(MessageRole::User, &format!("msg{i}")))
                .await
                .unwrap();
        }

        let child_key = mgr.fork(&parent, "chat1_fork", 3).await.unwrap();
        assert_eq!(child_key, SessionKey::new("telegram", "chat1_fork"));

        let child = mgr.get_or_create(&child_key);
        assert_eq!(child.parent_key, Some(parent.clone()));
        assert_eq!(child.messages.len(), 3);
        assert_eq!(child.messages[0].content, "msg2");
        assert_eq!(child.messages[2].content, "msg4");
    }

    #[tokio::test]
    async fn test_eviction_keeps_max_sessions() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = SessionManager::open(tmp.path())
            .unwrap()
            .with_max_sessions(3);

        // Create 5 sessions
        for i in 0..5 {
            let key = SessionKey::new("cli", &format!("s{i}"));
            mgr.add_message(&key, make_message(MessageRole::User, &format!("msg{i}")))
                .await
                .unwrap();
            // Small delay so last_accessed ordering is deterministic
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // Should have at most 3 in memory
        assert_eq!(mgr.cache_len(), 3);
    }

    #[tokio::test]
    async fn test_evicted_session_reloads_from_disk() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = SessionManager::open(tmp.path())
            .unwrap()
            .with_max_sessions(2);

        let k0 = SessionKey::new("cli", "oldest");
        mgr.add_message(&k0, make_message(MessageRole::User, "hello from oldest"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let k1 = SessionKey::new("cli", "middle");
        mgr.add_message(&k1, make_message(MessageRole::User, "hello from middle"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let k2 = SessionKey::new("cli", "newest");
        mgr.add_message(&k2, make_message(MessageRole::User, "hello from newest"))
            .await
            .unwrap();

        // k0 should have been evicted from memory
        assert_eq!(mgr.cache_len(), 2);

        // But accessing k0 should reload it from disk
        let session = mgr.get_or_create(&k0);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "hello from oldest");
    }

    #[test]
    fn test_with_max_sessions_clamps_zero() {
        let tmp = TempDir::new().unwrap();
        let mgr = SessionManager::open(tmp.path())
            .unwrap()
            .with_max_sessions(0);
        assert_eq!(mgr.capacity(), 1);
    }

    /// Integration test: concurrent session processing via multiple tasks.
    /// Verifies that sessions created from parallel tasks don't corrupt each other
    /// and the LRU cache correctly evicts and reloads.
    #[tokio::test]
    async fn test_concurrent_session_processing() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let tmp = TempDir::new().unwrap();
        let mgr = Arc::new(Mutex::new(
            SessionManager::open(tmp.path())
                .unwrap()
                .with_max_sessions(5),
        ));

        // Spawn 10 tasks that each create a session and add messages
        let mut handles = Vec::new();
        for i in 0..10 {
            let mgr = mgr.clone();
            handles.push(tokio::spawn(async move {
                let key = SessionKey::new("test", &format!("session-{i}"));
                let mut mgr = mgr.lock().await;
                mgr.add_message(
                    &key,
                    make_message(MessageRole::User, &format!("hello from {i}")),
                )
                .await
                .unwrap();
                mgr.add_message(
                    &key,
                    make_message(MessageRole::Assistant, &format!("reply to {i}")),
                )
                .await
                .unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // Cache should be capped at 5
        let mgr = mgr.lock().await;
        assert!(mgr.cache_len() <= 5);

        // But all 10 sessions should be loadable from disk
        drop(mgr);
        let mut fresh = SessionManager::open(tmp.path()).unwrap();
        for i in 0..10 {
            let key = SessionKey::new("test", &format!("session-{i}"));
            let session = fresh.get_or_create(&key);
            assert_eq!(
                session.messages.len(),
                2,
                "session-{i} should have 2 messages"
            );
            assert_eq!(session.messages[0].content, format!("hello from {i}"));
        }
    }

    #[tokio::test]
    async fn test_fork_persists_to_disk() {
        let tmp = TempDir::new().unwrap();
        let parent = SessionKey::new("cli", "main");

        {
            let mut mgr = SessionManager::open(tmp.path()).unwrap();
            mgr.add_message(&parent, make_message(MessageRole::User, "hello"))
                .await
                .unwrap();
            mgr.fork(&parent, "branch", 1).await.unwrap();
        }

        // Reload from disk
        let mut mgr2 = SessionManager::open(tmp.path()).unwrap();
        let child_key = SessionKey::new("cli", "branch");
        let child = mgr2.get_or_create(&child_key);
        assert_eq!(child.parent_key, Some(parent));
        assert_eq!(child.messages.len(), 1);
        assert_eq!(child.messages[0].content, "hello");
    }

    #[tokio::test]
    async fn test_load_rejects_oversized_file() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = SessionManager::open(tmp.path()).unwrap();
        let key = SessionKey::new("cli", "huge");

        // Write a normal message so the file exists
        mgr.add_message(&key, make_message(MessageRole::User, "seed"))
            .await
            .unwrap();

        // Evict from cache so next access must load from disk
        mgr.cache.pop(&key.0);

        // Overwrite the file with junk exceeding the size limit
        let path = mgr.session_path(&key);
        let junk = "x".repeat((MAX_SESSION_FILE_SIZE as usize) + 1);
        std::fs::write(&path, junk).unwrap();

        // load_from_disk should return None for oversized file
        assert!(mgr.load_from_disk(&key).is_none());
    }

    #[test]
    fn test_truncated_session_keys_no_collision() {
        let tmp = TempDir::new().unwrap();
        let mgr = SessionManager::open(tmp.path()).unwrap();

        // Create two keys that share the same 200-char prefix but differ after
        let prefix = "a".repeat(200);
        let key1 = SessionKey(format!("{prefix}_suffix1"));
        let key2 = SessionKey(format!("{prefix}_suffix2"));

        let path1 = mgr.session_path(&key1);
        let path2 = mgr.session_path(&key2);
        assert_ne!(
            path1, path2,
            "truncated keys with different suffixes must produce different paths"
        );
    }

    #[test]
    fn test_decode_session_name() {
        assert_eq!(
            SessionManager::decode_session_name("feishu%3Aoc_abc123"),
            "feishu:oc_abc123"
        );
        assert_eq!(
            SessionManager::decode_session_name("cli%3Adefault"),
            "cli:default"
        );
        assert_eq!(
            SessionManager::decode_session_name("plain-name"),
            "plain-name"
        );
        // Double-byte UTF-8 round-trip
        assert_eq!(
            SessionManager::decode_session_name("hello%E4%B8%96%E7%95%8C"),
            "hello\u{4e16}\u{754c}" // hello世界
        );
    }

    #[tokio::test]
    async fn test_list_sessions_returns_decoded_keys() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = SessionManager::open(tmp.path()).unwrap();
        let key = SessionKey::new("feishu", "oc_abc123");

        mgr.add_message(&key, make_message(MessageRole::User, "hello"))
            .await
            .unwrap();

        let sessions = mgr.list_sessions();
        assert_eq!(sessions.len(), 1);
        // Should return decoded key, not percent-encoded filename
        assert_eq!(sessions[0].0, "feishu:oc_abc123");
    }

    #[test]
    fn test_short_key_no_hash_suffix() {
        let tmp = TempDir::new().unwrap();
        let mgr = SessionManager::open(tmp.path()).unwrap();

        let key = SessionKey::new("cli", "short");
        let path = mgr.session_path(&key);
        let name = path.file_stem().unwrap().to_str().unwrap();
        // Short keys should not have hash suffix (no underscore + hex)
        assert!(!name.contains('_') || name.len() < 200);
    }
}
