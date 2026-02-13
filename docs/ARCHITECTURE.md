# Architecture Document: crew-rs

## Overview

crew-rs is a 6-crate Rust workspace providing both a coding agent CLI and a multi-channel messaging gateway.

```
┌─────────────────────────────────────────────────────────────┐
│                        crew-cli                             │
│           (CLI: chat, gateway, init, status)                │
├──────────────────────────┬──────────────────────────────────┤
│       crew-agent         │           crew-bus               │
│  (Agent, Tools, Skills)  │  (Channels, Sessions, Cron)     │
├──────────┬───────────────┴──────────────────────────────────┤
│crew-memory│           crew-llm                              │
│(Episodes) │      (LLM Providers)                            │
├──────────┴──────────────────────────────────────────────────┤
│                       crew-core                             │
│            (Types, Messages, Gateway Protocol)              │
└─────────────────────────────────────────────────────────────┘
```

## Crate Structure

### crew-core

Shared types with no internal dependencies.

- `Task`, `TaskKind`, `TaskStatus`, `TaskContext` - Task model with UUID v7 IDs
- `Message`, `MessageRole` - Conversation messages
- `AgentId` - Agent identification
- `InboundMessage`, `OutboundMessage` - Gateway protocol with metadata
- `SessionKey` - Channel:chat_id session routing
- `TokenUsage` - Token tracking
- `CrewError` - Error types with suggestions
- `truncate_utf8()` - Shared UTF-8 safe string truncation utility

### crew-llm

LLM provider abstraction with unified interface.

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, messages: &[Message], tools: &[ToolSpec], config: &ChatConfig) -> Result<ChatResponse>;
    fn model_id(&self) -> &str;
    fn provider_name(&self) -> &str;
}
```

**Native providers**: AnthropicProvider, OpenAIProvider, GeminiProvider, OpenRouterProvider

**OpenAI-compatible**: DeepSeek, Groq, Moonshot, DashScope, MiniMax, Zhipu, Ollama, vLLM - all use `OpenAIProvider::with_base_url()`

**RetryProvider**: Wraps any provider with exponential backoff on 429/5xx (max 3 retries)

### crew-memory

Persistence layer.

- `EpisodeStore` - redb database for task completion summaries
- `MemoryStore` - Long-term memory (MEMORY.md), daily notes (YYYY-MM-DD.md), recent memories (7-day window)
- `HybridSearch` - BM25 + vector (cosine similarity) hybrid ranking with configurable alpha. HNSW index via `hnsw_rs`, L2-normalized embeddings. Falls back to BM25-only when no embedding provider configured.

### crew-agent

Agent runtime and tool system.

**Agent**: Core execution loop with `run_task()` and `process_message()` methods.

**ToolRegistry**: HashMap of `Arc<dyn Tool>` with presets:
- `with_builtins()` - Standard tools
- `register_arc()` - For tools needing shared references (message, spawn)

**Built-in tools** (13):

| Category | Tools |
|----------|-------|
| File ops | read_file, write_file, edit_file, diff_edit |
| Search | glob, grep, list_dir |
| Execution | shell (with SafePolicy) |
| Web | web_search, web_fetch |
| Gateway | message, spawn, cron |

**Tool Policies** (`tools/policy.rs`): Allow/deny lists with deny-wins semantics, wildcard matching (`exec*`), and named groups (`group:fs`, `group:runtime`, `group:search`, `group:web`, `group:sessions`). Provider-specific policies via `tools.byProvider` in config — applied at both spec filtering and execution time, propagated to subagents.

**Context Compaction** (`compaction.rs`): Token-aware message compaction when context window fills. Estimates tokens, strips tool arguments, summarizes to first lines, preserves recent tool call/result pairs.

**Sandbox** (`sandbox.rs`): Three backends — `Bwrap` (Linux bubblewrap), `Macos` (sandbox-exec with SBPL profiles), `Docker` (container isolation with mount modes, resource limits, network isolation). Shared `BLOCKED_ENV_VARS` constant (18 env vars) sanitizes dangerous variables across all backends and MCP server spawning. Path validation rejects injection characters per backend.

**MCP** (`mcp.rs`): JSON-RPC stdio transport for Model Context Protocol servers. Environment variable sanitization via shared `BLOCKED_ENV_VARS`.

**Skills**: Markdown files with YAML frontmatter, loaded from `.crew/skills/`. Support `always: true` for auto-inclusion in system prompt. 6 built-in skills bundled at compile time via `include_str!()` (cron, github, skill-creator, summarize, tmux, weather). Workspace skills override built-ins with the same name.

### crew-bus

Gateway infrastructure.

**Message Bus**: `create_bus()` returns `(AgentHandle, BusPublisher)` linked by mpsc channels (capacity 256).

**Channel trait**:
```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    async fn start(&self, inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()>;
    async fn send(&self, msg: &OutboundMessage) -> Result<()>;
    fn is_allowed(&self, sender_id: &str) -> bool;
    async fn stop(&self) -> Result<()>;
}
```

**Channels**: CliChannel, TelegramChannel, DiscordChannel, SlackChannel, WhatsAppChannel, FeishuChannel, EmailChannel

**Media**: `media.rs` provides shared `download_media()` helper for downloading photos, voice, audio, and documents from channels to `.crew/media/`.

**Transcription**: Voice/audio messages are auto-transcribed via Groq Whisper (configured in crew-llm `transcription` module) before being sent to the agent.

**Message Coalescing** (`coalesce.rs`): Splits long messages into channel-safe chunks. Break preference: paragraph > newline > sentence > space > hard cut. Per-channel limits (Telegram 4000, Discord 1900, Slack 3900). MAX_CHUNKS (50) DoS limit with truncation logging. UTF-8 safe boundary detection.

**ChannelManager**: Registers channels, dispatches outbound messages. Splits oversized messages via coalescer before sending.

**SessionManager**: JSONL persistence at `.crew/sessions/{key}.jsonl`. In-memory cache with disk sync. Percent-encoded filenames for collision-free key mapping. Atomic write-then-rename for crash safety. Session forking via `fork()` — creates child session with `parent_key` tracking, copies last N messages from parent.

**CronService**: JSON persistence, supports `Every` (interval), `At` (one-shot), and `Cron` (cron expression via `cron` crate) schedules. Timer-based execution. Supports enable/disable per job.

**HeartbeatService**: Periodic check of HEARTBEAT.md (default: 30 min). Sends to agent if non-empty.

### crew-cli

CLI interface and configuration.

**Commands**: chat, init, status, gateway, clean, completions, cron (list/add/remove/enable), channels (status/login), auth (login/logout/status), skills (list/install/remove)

**Auth module** (`auth/`): OAuth PKCE browser flow, device code flow, and paste-token for API key management. Credentials stored in `~/.crew/auth.json` (mode 0600). `config.rs` checks auth store before env vars for API keys.

**Config**: Loaded from `.crew/config.json` or `~/.config/crew/config.json`. Supports `${VAR}` expansion. Provider auto-detect via `detect_provider()`. Versioned config with automatic migration framework (`migrate_config()`).

**Config Watcher** (`config_watcher.rs`): SHA-256 hash-based change detection. Hot-reloads system prompt (AGENTS.md, SOUL.md, USER.md) without restart. Detects provider/model changes requiring restart.

## Data Flows

### Chat Mode (crew chat)

```
User Input → readline → Agent.process_message(input, history)
                              │
                              ├─ Build messages (system + history + input)
                              ├─ Call LLM with tool specs
                              ├─ Execute tools if returned (loop)
                              └─ Return ConversationResponse
                                    │
                              Print response, append to history
```

### Gateway Mode (crew gateway)

```
Channel → InboundMessage → MessageBus → Agent.process_message()
                                              │
                                        OutboundMessage
                                              │
                                  ChannelManager.dispatch()
                                              │
                                        Target Channel
```

System messages (cron, heartbeat, spawn results) flow through the same bus with `channel: "system"` and metadata routing.

## File Layout

```
crates/
├── crew-core/src/
│   ├── lib.rs, task.rs, types.rs, error.rs, gateway.rs, utils.rs
├── crew-llm/src/
│   ├── lib.rs, provider.rs, config.rs, types.rs, retry.rs, sse.rs
│   ├── embedding.rs, pricing.rs, context.rs, transcription.rs
│   ├── anthropic.rs, openai.rs, gemini.rs, openrouter.rs
├── crew-memory/src/
│   ├── lib.rs, episode.rs, store.rs, memory_store.rs, hybrid_search.rs
├── crew-agent/src/
│   ├── lib.rs, agent.rs, progress.rs, policy.rs, compaction.rs
│   ├── sandbox.rs, mcp.rs, skills.rs, builtin_skills.rs
│   ├── plugins/ (loader.rs, manifest.rs)
│   ├── skills/ (cron, github, skill-creator, summarize, tmux, weather SKILL.md)
│   └── tools/ (mod, policy, shell, read_file, write_file, edit_file, diff_edit,
│               list_dir, glob_tool, grep_tool, web_search, web_fetch,
│               message, spawn)
├── crew-bus/src/
│   ├── lib.rs, bus.rs, channel.rs, session.rs, coalesce.rs
│   ├── cli_channel.rs, telegram_channel.rs, discord_channel.rs
│   ├── slack_channel.rs, whatsapp_channel.rs, feishu_channel.rs, email_channel.rs
│   ├── media.rs
│   ├── cron_service.rs, cron_types.rs, heartbeat.rs
└── crew-cli/src/
    ├── main.rs, config.rs, config_watcher.rs, cron_tool.rs
    ├── auth/ (mod, store, oauth, token)
    └── commands/ (mod, chat, init, status, gateway,
                   clean, completions, cron, channels, auth, skills)
```

## Security

- API keys from environment variables only (never in config)
- `ShellTool` uses `SafePolicy` blocking: `rm -rf /`, `dd`, `mkfs`, fork bombs
- Channel access control via `allowed_senders` lists
- Path traversal prevention + symlink rejection in file tools
- SSRF protection in web_fetch: blocks private IPs (IPv4/IPv6), localhost, link-local, ULA, IPv4-mapped
- Sandbox isolation: bwrap (Linux), sandbox-exec (macOS), Docker — with `SandboxMode::Auto` detection
- Environment sanitization: `BLOCKED_ENV_VARS` (18 vars) shared across sandbox backends and MCP
- Path injection prevention: Docker rejects `:`, `\0`, `\n`, `\r`; macOS rejects control chars, `(`, `)`, `\`, `"`
- Tool policies: allow/deny with deny-wins semantics, provider-specific filtering
- UTF-8 safe truncation via `truncate_utf8()` across all tool outputs
- Session file collision prevention via percent-encoded filenames
- Atomic write-then-rename for session persistence (crash safety)

## Testing

253+ tests across all crates. Categories:
- Unit: type serde, tool arg parsing, config validation, provider detection, tool policies, compaction, coalescing
- Integration: CLI commands, file tools, session persistence, cron jobs, session forking
- Security: sandbox path injection, env sanitization, SSRF blocking, symlink rejection, private IP detection
- Channel: allowed_senders, message parsing, dedup logic
