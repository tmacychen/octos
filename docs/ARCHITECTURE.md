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

### crew-agent

Agent runtime and tool system.

**Agent**: Core execution loop with `run_task()` and `process_message()` methods.

**ToolRegistry**: HashMap of `Arc<dyn Tool>` with presets:
- `with_builtins()` - Standard tools
- `register_arc()` - For tools needing shared references (message, spawn)

**Built-in tools** (12):

| Category | Tools |
|----------|-------|
| File ops | read_file, write_file, edit_file |
| Search | glob, grep, list_dir |
| Execution | shell (with SafePolicy) |
| Web | web_search, web_fetch |
| Gateway | message, spawn, cron |

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

**ChannelManager**: Registers channels, dispatches outbound messages.

**SessionManager**: JSONL persistence at `.crew/sessions/{key}.jsonl`. In-memory cache with disk sync.

**CronService**: JSON persistence, supports `Every` (interval), `At` (one-shot), and `Cron` (cron expression via `cron` crate) schedules. Timer-based execution. Supports enable/disable per job.

**HeartbeatService**: Periodic check of HEARTBEAT.md (default: 30 min). Sends to agent if non-empty.

### crew-cli

CLI interface and configuration.

**Commands**: chat, init, status, gateway, clean, completions, cron (list/add/remove/enable), channels (status/login), auth (login/logout/status), skills (list/install/remove)

**Auth module** (`auth/`): OAuth PKCE browser flow, device code flow, and paste-token for API key management. Credentials stored in `~/.crew/auth.json` (mode 0600). `config.rs` checks auth store before env vars for API keys.

**Config**: Loaded from `.crew/config.json` or `~/.config/crew/config.json`. Supports `${VAR}` expansion. Provider auto-detect via `detect_provider()`. Versioned config with automatic migration framework (`migrate_config()`).

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
│   ├── lib.rs, task.rs, types.rs, error.rs, gateway.rs
├── crew-llm/src/
│   ├── lib.rs, provider.rs, config.rs, types.rs, retry.rs, transcription.rs
│   ├── anthropic.rs, openai.rs, gemini.rs, openrouter.rs
├── crew-memory/src/
│   ├── lib.rs, episode.rs, store.rs, memory_store.rs
├── crew-agent/src/
│   ├── lib.rs, agent.rs, progress.rs, policy.rs, skills.rs, builtin_skills.rs
│   ├── skills/ (cron, github, skill-creator, summarize, tmux, weather SKILL.md)
│   └── tools/ (mod, shell, read_file, write_file, edit_file, list_dir,
│               glob_tool, grep_tool, web_search, web_fetch,
│               message, spawn)
├── crew-bus/src/
│   ├── lib.rs, bus.rs, channel.rs, session.rs
│   ├── cli_channel.rs, telegram_channel.rs, discord_channel.rs
│   ├── slack_channel.rs, whatsapp_channel.rs, feishu_channel.rs, email_channel.rs
│   ├── media.rs
│   ├── cron_service.rs, cron_types.rs, heartbeat.rs
└── crew-cli/src/
    ├── main.rs, config.rs, cron_tool.rs
    ├── auth/ (mod, store, oauth, token)
    └── commands/ (mod, chat, init, status, gateway,
                   clean, completions, cron, channels, auth, skills)
```

## Security

- API keys from environment variables only (never in config)
- `ShellTool` uses `SafePolicy` blocking: `rm -rf /`, `dd`, `mkfs`, fork bombs
- Channel access control via `allowed_senders` lists
- Path traversal prevention in file tools

## Testing

133+ tests across all crates. Categories:
- Unit: type serde, tool arg parsing, config validation, provider detection
- Integration: CLI commands, file tools, session persistence, cron jobs
- Channel: allowed_senders, message parsing, dedup logic
