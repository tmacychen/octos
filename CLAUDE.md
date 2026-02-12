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
crew-cli  (CLI: clap commands, config loading)
    |
crew-agent  (Agent loop, tool system, progress reporting)
    |          \
crew-memory   crew-llm  (redb episodes + memory store | LLM providers)
    \           /
    crew-core  (Task, Message, Error types - no internal deps)
```

crew-bus (Message bus, channels, sessions, cron, heartbeat) sits alongside crew-agent.

Commands: chat, init, status, gateway, clean, completions, cron, channels, auth (login/logout/status), skills (list/install/remove).

Auth module (`crew-cli/src/auth/`): OAuth PKCE + device code for OpenAI, paste-token for others. Stored in `~/.crew/auth.json`. `config.rs` checks auth store before env vars.

### Key Flow: Agent Loop (`crew-agent/src/agent.rs`)

1. Build messages (system prompt + conversation history + memory context)
2. Call LLM with tool specs
3. If tool calls returned -> execute tools -> append results -> loop
4. If EndTurn or budget exceeded -> return result

### Tool System (`crew-agent/src/tools/`)

All tools implement `Tool` trait (`spec() -> ToolSpec`, `execute(&Value) -> ToolResult`). Registered in `ToolRegistry` (HashMap). Tools: shell, read_file, write_file, edit_file, glob, grep, list_dir, web_search, web_fetch, message, spawn, cron.

### LLM Providers (`crew-llm/src/`)

`LlmProvider` trait with `chat()` method. Four native providers: `AnthropicProvider`, `OpenAIProvider`, `GeminiProvider`, `OpenRouterProvider`. 8 OpenAI-compatible via `with_base_url()`. `RetryProvider` wraps any provider with exponential backoff on 429/5xx.

### Memory (`crew-memory/src/`)

- `EpisodeStore`: redb database at `.crew/episodes.redb`, stores task completion summaries
- `MemoryStore`: Long-term memory (MEMORY.md), daily notes, recent memories (7-day window)

## Key Types

- `Task` (crew-core): UUID v7 ID, kind (Code/Plan/Review/Custom), status, context
- `Message` (crew-core): role (System/User/Assistant/Tool), content, tool_call_id
- `ChatResponse` (crew-llm): content, tool_calls, stop_reason, token usage
- `AgentConfig` (crew-agent): max_iterations (default 50), max_tokens, save_episodes

## Project Conventions

- Edition 2024, rust-version 1.85.0
- Pure Rust TLS via rustls (no OpenSSL dependency)
- `eyre`/`color-eyre` for error handling (not `anyhow`)
- `Arc<dyn Trait>` for shared providers/tools/reporters
- `AtomicBool` for shutdown signaling
- API keys from env vars via `api_key_env` or OAuth via `crew auth login`
- Email channel feature-gated: `async-imap` + `lettre` + `mailparse`
- `ShellTool` has `SafePolicy` that denies dangerous commands (rm -rf /, dd, mkfs, fork bomb)
