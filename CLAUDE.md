# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests
cargo test -p crew-agent         # Test single crate
cargo test -p crew-agent test_name  # Run single test
cargo clippy --workspace         # Lint
cargo fmt --all                  # Format
cargo fmt --all -- --check       # Check formatting
cargo install --path crates/crew-cli  # Install CLI locally
```

## Architecture

crew-rs is a Rust-native AI coding agent framework. 6-crate workspace, layered:

```
crew-cli  (CLI: clap commands, config loading, config watcher)
    |
crew-agent  (Agent loop, tool system, sandbox, MCP, compaction)
    |          \
crew-memory   crew-llm  (hybrid search + memory store | LLM providers)
    \           /
    crew-core  (Task, Message, Error types, truncate_utf8 - no internal deps)
```

crew-bus (Message bus, channels, sessions, coalescing, cron, heartbeat) sits alongside crew-agent.

Commands: chat, init, status, gateway, clean, completions, cron, channels, auth (login/logout/status), skills (list/install/remove).

Auth module (`crew-cli/src/auth/`): OAuth PKCE + device code for OpenAI, paste-token for others. Stored in `~/.crew/auth.json`. `config.rs` checks auth store before env vars.

### Key Flow: Agent Loop (`crew-agent/src/agent.rs`)

1. Build messages (system prompt + conversation history + memory context)
2. Call LLM with tool specs (filtered by ToolPolicy + provider policy)
3. If tool calls returned -> execute tools -> append results -> loop
4. If EndTurn or budget exceeded -> return result
5. Context compaction kicks in when token budget fills (`compaction.rs`)

### Tool System (`crew-agent/src/tools/`)

All tools implement `Tool` trait (`spec() -> ToolSpec`, `execute(&Value) -> ToolResult`). Registered in `ToolRegistry` (HashMap). Tools: shell, read_file, write_file, edit_file, glob, grep, list_dir, web_search, web_fetch, message, spawn, cron.

**Tool Policies** (`tools/policy.rs`): Allow/deny lists with deny-wins semantics, wildcard matching (`exec*`), and named groups (`group:fs`, `group:runtime`, `group:search`, `group:web`, `group:sessions`). Provider-specific policies via `tools.byProvider` in config.

### Sandbox (`crew-agent/src/sandbox.rs`)

Three sandbox backends: `Bwrap` (Linux), `Macos` (sandbox-exec), `Docker`. Auto-detection in `SandboxMode::Auto`. Shared `BLOCKED_ENV_VARS` constant (18 env vars) across all backends and MCP server spawning. Docker supports mount modes (none/ro/rw), resource limits (CPU/memory/PIDs), network isolation. Path validation rejects injection characters (`:`, `\0`, `\n`, `\r` for Docker; control chars, `(`, `)`, `\`, `"` for macOS SBPL).

### MCP (`crew-agent/src/mcp.rs`)

JSON-RPC stdio transport for MCP servers. Env var sanitization via shared `BLOCKED_ENV_VARS`.

### Context Compaction (`crew-agent/src/compaction.rs`)

Token-aware message compaction: estimates tokens, strips tool arguments, summarizes to first lines, preserves recent tool call/result pairs.

### LLM Providers (`crew-llm/src/`)

`LlmProvider` trait with `chat()` method. Four native providers: `AnthropicProvider`, `OpenAIProvider`, `GeminiProvider`, `OpenRouterProvider`. 8 OpenAI-compatible via `with_base_url()`. `RetryProvider` wraps any provider with exponential backoff on 429/5xx.

### Memory (`crew-memory/src/`)

- `EpisodeStore`: redb database at `.crew/episodes.redb`, stores task completion summaries
- `MemoryStore`: Long-term memory (MEMORY.md), daily notes, recent memories (7-day window)
- `HybridSearch`: BM25 + vector (cosine similarity) hybrid ranking with HNSW index (`hnsw_rs`). Falls back to BM25-only without embedding provider.

### Message Coalescing (`crew-bus/src/coalesce.rs`)

Splits long messages into channel-safe chunks (paragraph > newline > sentence > space > hard cut). Per-channel limits. MAX_CHUNKS (50) DoS limit. UTF-8 safe boundary detection.

### Session Management (`crew-bus/src/session.rs`)

JSONL persistence with in-memory cache. Session forking (`/new` command) with parent_key tracking. Percent-encoded filenames. Atomic write-then-rename for crash safety.

### Config Hot-Reload (`crew-cli/src/config_watcher.rs`)

SHA-256 hash-based change detection. Hot-reload for system prompt; restart-required for provider/model changes.

## Key Types

- `Task` (crew-core): UUID v7 ID, kind (Code/Plan/Review/Custom), status, context
- `Message` (crew-core): role (System/User/Assistant/Tool), content, tool_call_id
- `ChatResponse` (crew-llm): content, tool_calls, stop_reason, token usage
- `AgentConfig` (crew-agent): max_iterations (default 50), max_tokens, save_episodes
- `truncate_utf8` (crew-core): Shared UTF-8 safe string truncation utility

## Project Conventions

- Edition 2024, rust-version 1.85.0
- Pure Rust TLS via rustls (no OpenSSL dependency)
- `eyre`/`color-eyre` for error handling (not `anyhow`)
- `Arc<dyn Trait>` for shared providers/tools/reporters
- `AtomicBool` for shutdown signaling
- API keys from env vars via `api_key_env` or OAuth via `crew auth login`
- Email channel feature-gated: `async-imap` + `lettre` + `mailparse`
- `ShellTool` has `SafePolicy` that denies dangerous commands (rm -rf /, dd, mkfs, fork bomb)
- `BLOCKED_ENV_VARS` shared across sandbox backends and MCP (18 vars: LD_PRELOAD, DYLD_*, NODE_OPTIONS, etc.)
