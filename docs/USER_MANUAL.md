# User Manual: crew-rs

## Table of Contents

1. [Introduction](#introduction)
2. [Installation](#installation)
3. [Quick Start](#quick-start)
4. [Configuration](#configuration)
5. [Commands Reference](#commands-reference)
6. [Working with Providers](#working-with-providers)
7. [Gateway Mode](#gateway-mode)
8. [Memory & Skills](#memory--skills)
9. [Task Management](#task-management)
10. [Advanced Usage](#advanced-usage)
11. [Troubleshooting](#troubleshooting)

---

## Introduction

crew-rs is a Rust-native AI agent framework that operates in three modes:

- **Chat mode** (`crew chat`): Interactive multi-turn conversation with tools
- **Task mode** (`crew run`): One-shot coding task execution with progress tracking
- **Gateway mode** (`crew gateway`): Persistent daemon serving multiple messaging channels

### Key Concepts

| Term | Description |
|------|-------------|
| **Agent** | AI that executes tasks using tools |
| **Tool** | A capability (shell, file ops, search, messaging) |
| **Provider** | LLM API service (Anthropic, OpenAI, etc.) |
| **Channel** | Messaging platform (CLI, Telegram, Slack, etc.) |
| **Session** | Conversation history per channel:chat_id |
| **Skill** | Reusable instruction template (SKILL.md) |
| **Bootstrap** | Context files loaded into system prompt (AGENTS.md, SOUL.md, etc.) |

---

## Installation

### Prerequisites

- Rust 1.85.0 or later
- An API key from at least one supported provider

### From Source

```bash
git clone https://github.com/heyong4725/crew-rs
cd crew-rs

# Basic (CLI, chat, run, gateway with CLI channel)
cargo install --path crates/crew-cli

# With messaging channels
cargo install --path crates/crew-cli --features telegram,discord,slack,whatsapp,feishu

# Verify
crew --version
```

### API Keys

```bash
# Anthropic (Claude) - recommended
export ANTHROPIC_API_KEY="sk-ant-..."

# OpenAI
export OPENAI_API_KEY="sk-..."

# Or any other supported provider (see Providers section)
```

Add to `~/.bashrc` or `~/.zshrc` for persistence.

---

## Quick Start

```bash
# 1. Initialize workspace
cd your-project
crew init

# 2. Check setup
crew status

# 3. Start chatting
crew chat

# 4. Or run a one-shot task
crew run "Add a hello function to lib.rs"
```

---

## Configuration

### Config Files

Loaded in order (first found wins):
1. `.crew/config.json` (project-local)
2. `~/.config/crew/config.json` (global)

### Basic Config

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-4-20250514",
  "api_key_env": "ANTHROPIC_API_KEY"
}
```

### Gateway Config

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-4-20250514",
  "gateway": {
    "channels": [
      {"type": "cli"},
      {"type": "telegram", "allowed_senders": ["123456789"]},
      {"type": "discord", "settings": {"token_env": "DISCORD_BOT_TOKEN"}},
      {"type": "slack", "settings": {"bot_token_env": "SLACK_BOT_TOKEN", "app_token_env": "SLACK_APP_TOKEN"}},
      {"type": "whatsapp", "settings": {"bridge_url": "ws://localhost:3001"}},
      {"type": "feishu", "settings": {"app_id_env": "FEISHU_APP_ID", "app_secret_env": "FEISHU_APP_SECRET"}}
    ],
    "max_history": 50,
    "system_prompt": "You are a helpful assistant."
  }
}
```

### Environment Variable Expansion

Use `${VAR_NAME}` syntax:

```json
{
  "base_url": "${ANTHROPIC_BASE_URL}",
  "model": "${CREW_MODEL}"
}
```

---

## Commands Reference

### `crew chat`

Interactive multi-turn conversation with readline history.

```bash
crew chat [OPTIONS]

Options:
  -c, --cwd <PATH>         Working directory
      --config <PATH>      Config file path
      --provider <NAME>    LLM provider
      --model <NAME>       Model name
      --base-url <URL>     Custom API endpoint
      --max-iterations <N> Max tool iterations per message (default: 20)
  -v, --verbose            Show tool outputs
      --no-retry           Disable retry
```

Features:
- Arrow keys, line editing (rustyline)
- Persistent history at `.crew/history/chat_history`
- Exit: `/exit`, `/quit`, `exit`, `quit`, `:q`, Ctrl+C, Ctrl+D
- Full tool access (shell, files, search, web)

```bash
crew chat                          # Default
crew chat --provider deepseek      # Use DeepSeek
crew chat --model glm-4-plus       # Auto-detects Zhipu
```

---

### `crew run <goal>`

Execute a one-shot coding task.

```bash
crew run [OPTIONS] <GOAL>

Options:
  -c, --cwd <PATH>         Working directory
      --config <PATH>      Config file path
      --provider <NAME>    LLM provider
      --model <NAME>       Model name
      --base-url <URL>     Custom API endpoint
      --coordinate         Run as coordinator (decompose + delegate)
      --max-iterations <N> Max iterations (default: 50)
      --max-tokens <N>     Token budget
  -v, --verbose            Show tool outputs
      --no-retry           Disable retry
```

```bash
crew run "Fix the bug in auth.rs"
crew run --coordinate "Build REST API for users"
crew run --model gpt-4o --max-tokens 50000 "Refactor database module"
```

---

### `crew gateway`

Run as a persistent multi-channel daemon.

```bash
crew gateway [OPTIONS]

Options:
  -c, --cwd <PATH>         Working directory
      --config <PATH>      Config file path
      --provider <NAME>    Override provider
      --model <NAME>       Override model
      --base-url <URL>     Override API endpoint
  -v, --verbose            Verbose logging
      --no-retry           Disable retry
```

Requires `gateway` section in config with `channels` array. Runs continuously until Ctrl+C.

---

### `crew init`

Initialize workspace with config and bootstrap files.

```bash
crew init [OPTIONS]

Options:
  -c, --cwd <PATH>    Working directory
      --defaults       Skip prompts, use defaults
```

Creates:
- `.crew/config.json` - Provider/model config
- `.crew/.gitignore` - Ignores state files
- `.crew/AGENTS.md` - Agent instructions template
- `.crew/SOUL.md` - Personality template
- `.crew/USER.md` - User info template
- `.crew/memory/` - Memory storage directory
- `.crew/sessions/` - Session history directory
- `.crew/skills/` - Custom skills directory

---

### `crew status [task-id]`

Show system status or task details.

```bash
crew status [OPTIONS] [TASK_ID]

Options:
  -c, --cwd <PATH>    Working directory
```

**System status** (no args):
```
crew-rs Status
══════════════════════════════════════════════════

Config:    .crew/config.json (found)
Workspace: .crew/            (found)
Provider:  anthropic
Model:     claude-sonnet-4-20250514

API Keys
──────────────────────────────────────────────────
  Anthropic    ANTHROPIC_API_KEY         set
  OpenAI       OPENAI_API_KEY           not set
  Gemini       GEMINI_API_KEY           not set
  ...

Bootstrap Files
──────────────────────────────────────────────────
  AGENTS.md        found
  SOUL.md          found
  USER.md          found
  TOOLS.md         missing
  IDENTITY.md      missing
```

**Task status** (with ID): shows task details, progress, token usage, conversation preview.

---

### Other Commands

```bash
crew resume [task-id]          # Resume interrupted task
crew list                      # List resumable tasks
crew clean [--all] [--dry-run] # Clean state files
crew completions <shell>       # Generate completions (bash/zsh/fish/powershell)
crew cron list [--all]         # List cron jobs
crew cron add [OPTIONS]        # Add a cron job
crew cron remove <job-id>      # Remove a cron job
crew cron enable <job-id>      # Enable/disable a cron job
crew channels status           # Show channel compile/config status
```

---

## Working with Providers

### Supported Providers

| Provider | Env Var | Default Model | Notes |
|----------|---------|---------------|-------|
| anthropic | `ANTHROPIC_API_KEY` | claude-sonnet-4-20250514 | Native API |
| openai | `OPENAI_API_KEY` | gpt-4o | Native API |
| gemini | `GEMINI_API_KEY` | gemini-2.0-flash | Native API |
| openrouter | `OPENROUTER_API_KEY` | anthropic/claude-sonnet-4-20250514 | Aggregator |
| deepseek | `DEEPSEEK_API_KEY` | deepseek-chat | OpenAI-compatible |
| groq | `GROQ_API_KEY` | llama-3.3-70b-versatile | OpenAI-compatible |
| moonshot | `MOONSHOT_API_KEY` | kimi-k2.5 | Also: `kimi` |
| dashscope | `DASHSCOPE_API_KEY` | qwen-max | Also: `qwen` |
| minimax | `MINIMAX_API_KEY` | MiniMax-Text-01 | OpenAI-compatible |
| zhipu | `ZHIPU_API_KEY` | glm-4-plus | Also: `glm` |
| ollama | (none) | llama3.2 | Local, no key needed |
| vllm | `VLLM_API_KEY` | (required) | Requires --base-url |

### Provider Auto-Detection

When `--provider` is omitted, crew-rs detects from model name:

```bash
crew chat --model gpt-4o           # -> openai
crew chat --model claude-sonnet-4-20250514  # -> anthropic
crew chat --model deepseek-chat    # -> deepseek
crew chat --model glm-4-plus       # -> zhipu
crew chat --model qwen-max         # -> dashscope
```

### Custom Endpoints

```bash
# Azure OpenAI
crew run --provider openai --base-url "https://your.openai.azure.com/v1" "Task"

# Local Ollama
crew chat --provider ollama --model llama3.2

# vLLM server
crew run --provider vllm --base-url "http://localhost:8000/v1" --model "meta-llama/Llama-3-70b" "Task"
```

---

## Gateway Mode

### Channel Setup

#### Telegram

```bash
export TELEGRAM_BOT_TOKEN="123456:ABC..."
```

Config:
```json
{"type": "telegram", "allowed_senders": ["your_user_id"], "settings": {"token_env": "TELEGRAM_BOT_TOKEN"}}
```

#### Slack

Requires Socket Mode app with bot token + app-level token.

```bash
export SLACK_BOT_TOKEN="xoxb-..."
export SLACK_APP_TOKEN="xapp-..."
```

Config:
```json
{"type": "slack", "settings": {"bot_token_env": "SLACK_BOT_TOKEN", "app_token_env": "SLACK_APP_TOKEN"}}
```

#### Discord

```bash
export DISCORD_BOT_TOKEN="..."
```

Config:
```json
{"type": "discord", "settings": {"token_env": "DISCORD_BOT_TOKEN"}}
```

#### WhatsApp

Requires Node.js bridge (Baileys) running at `ws://localhost:3001`.

Config:
```json
{"type": "whatsapp", "settings": {"bridge_url": "ws://localhost:3001"}}
```

#### Feishu/Lark

```bash
export FEISHU_APP_ID="cli_..."
export FEISHU_APP_SECRET="..."
```

Config:
```json
{"type": "feishu", "settings": {"app_id_env": "FEISHU_APP_ID", "app_secret_env": "FEISHU_APP_SECRET"}}
```

### Access Control

Use `allowed_senders` to restrict access. Empty list = allow all.

```json
{"type": "telegram", "allowed_senders": ["123456", "789012"]}
```

### Cron Jobs

The agent can schedule recurring tasks via the `cron` tool:

- `add` - Create a scheduled job with `every_seconds`, `cron_expr`, or `at` timestamp
- `list` - Show all scheduled jobs
- `remove` - Delete a job by ID
- `enable` / `disable` - Toggle job active state

Cron expressions use standard syntax (e.g., `"0 0 9 * * * *"` for daily at 9am).

Cron jobs send messages through the bus and can deliver responses to any channel.

#### Cron CLI

Manage jobs directly from the command line (no running gateway needed):

```bash
crew cron list                          # List active jobs
crew cron list --all                    # Include disabled
crew cron add --name "report" --message "Generate daily report" --cron "0 0 9 * * * *"
crew cron add --name "check" --message "Check status" --every 3600
crew cron add --name "once" --message "Run migration" --at "2025-03-01T09:00:00Z"
crew cron remove <job-id>
crew cron enable <job-id>               # Enable
crew cron enable <job-id> --disable     # Disable
```

### Channel Status

Check configured channels and their compile/config status:

```bash
crew channels status
```

Shows a table with channel name, compile status (feature flags), and config summary (env vars set/missing).

### Heartbeat

The heartbeat service periodically reads `.crew/HEARTBEAT.md` and sends its content to the agent if non-empty. Default interval: 30 minutes. Use this for background task instructions.

---

## Memory & Skills

### Bootstrap Files

Loaded into the system prompt at startup:

| File | Purpose |
|------|---------|
| `.crew/AGENTS.md` | Agent instructions and guidelines |
| `.crew/SOUL.md` | Personality and values |
| `.crew/USER.md` | User information and preferences |
| `.crew/TOOLS.md` | Tool-specific guidance |
| `.crew/IDENTITY.md` | Custom identity definition |

Create via `crew init` (creates AGENTS, SOUL, USER templates).

### Memory System

- **Long-term**: `.crew/memory/MEMORY.md` - Persistent notes
- **Daily**: `.crew/memory/YYYY-MM-DD.md` - Auto-created daily logs
- **Recent**: Last 7 days of daily notes included in context

### Skills

Place custom skills in `.crew/skills/{name}/SKILL.md`:

```markdown
---
description: My custom skill
always: true
available: true
---

# Skill Instructions

Your skill content here...
```

Skills with `always: true` are included in every prompt. Others are available for the agent to read on demand.

### Built-in Skills

crew-rs bundles 6 skills at compile time. These are available without any setup:

| Skill | Description | Requirements |
|-------|-------------|-------------|
| cron | Cron tool usage examples | (none, always-on) |
| github | gh CLI patterns (PR, issue, API) | `gh` binary |
| skill-creator | How to create custom skills | (none) |
| summarize | URL/file summarization | `summarize` binary |
| tmux | tmux session automation | `tmux` binary |
| weather | Weather via wttr.in | `curl` binary |

Workspace skills in `.crew/skills/` override built-in skills with the same name.

---

## Task Management

### Task Lifecycle

```
crew run "goal" → Task created → Agent loop → State saved each iteration
                                                    │
                                              Ctrl+C interrupts
                                                    │
                                              crew resume <id>
```

### Files

```
.crew/
├── config.json          # Configuration (versioned, auto-migrated)
├── cron.json            # Cron job store
├── AGENTS.md            # Agent instructions
├── SOUL.md              # Personality
├── USER.md              # User info
├── HEARTBEAT.md         # Background tasks
├── tasks/               # Task state (JSON)
├── sessions/            # Chat history (JSONL)
├── memory/              # Memory files
│   ├── MEMORY.md        # Long-term
│   └── 2025-02-10.md    # Daily
├── skills/              # Custom skills
├── episodes.redb        # Episodic memory DB
└── history/
    └── chat_history     # Readline history
```

---

## Advanced Usage

### Coordinator Mode

For complex tasks, the coordinator decomposes work and delegates to workers:

```bash
crew run --coordinate "Implement user auth with login, logout, and password reset"
```

### Token Budgets

```bash
crew run --max-tokens 50000 "Large refactoring"
```

### Verbose Mode

```bash
crew run -v "Add logging"    # Shows full tool outputs
crew chat -v                 # Shows tool execution details
```

---

## Troubleshooting

### API Key Not Set

```
Error: ANTHROPIC_API_KEY environment variable not set
```

Fix: `export ANTHROPIC_API_KEY="your-key"` or check with `crew status`.

### Rate Limited (429)

Retry mechanism handles this automatically (3 attempts with backoff). If persistent, try a different provider or wait.

### Max Iterations Reached

```bash
crew resume <task-id> --max-iterations 100
```

### Debug Logging

```bash
RUST_LOG=debug crew chat
RUST_LOG=crew_agent=trace crew run "task"
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `GEMINI_API_KEY` | Gemini API key |
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `DEEPSEEK_API_KEY` | DeepSeek API key |
| `GROQ_API_KEY` | Groq API key |
| `MOONSHOT_API_KEY` | Moonshot API key |
| `DASHSCOPE_API_KEY` | DashScope API key |
| `MINIMAX_API_KEY` | MiniMax API key |
| `ZHIPU_API_KEY` | Zhipu API key |
| `RUST_LOG` | Log level (error/warn/info/debug/trace) |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token |
| `DISCORD_BOT_TOKEN` | Discord bot token |
| `SLACK_BOT_TOKEN` | Slack bot token |
| `SLACK_APP_TOKEN` | Slack app-level token |
| `FEISHU_APP_ID` | Feishu app ID |
| `FEISHU_APP_SECRET` | Feishu app secret |
