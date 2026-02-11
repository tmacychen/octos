# crew-rs

Rust-native AI agent framework with multi-channel gateway, 12+ LLM providers, and coding automation tools.

## Features

- **12+ LLM providers**: Anthropic, OpenAI, Gemini, OpenRouter, DeepSeek, Groq, Moonshot, DashScope, MiniMax, Zhipu, Ollama, vLLM
- **Multi-channel gateway**: CLI, Telegram, Discord, Slack, WhatsApp, Feishu/Lark
- **Interactive chat**: Multi-turn conversation with readline history
- **Task execution**: Run coding tasks with coordinator/worker pattern
- **Resumable tasks**: Interrupt with Ctrl+C and resume later
- **Memory system**: Episodic memory, daily notes, long-term memory, bootstrap files
- **Skills system**: Markdown-based skills with YAML frontmatter + 6 built-in skills
- **Cron & heartbeat**: Scheduled tasks (interval, one-shot, cron expressions) and periodic background checks
- **Subagent spawning**: Background agents for long-running tasks
- **Cross-channel messaging**: Send messages across any connected channel
- **Provider auto-detect**: Automatically selects provider from model name
- **Built-in tools**: Shell, file ops, glob, grep, list_dir, web search/fetch, message, spawn, cron
- **Config migration**: Versioned config with automatic migration

## Installation

```bash
# From source
cargo install --path crates/crew-cli

# With channel support
cargo install --path crates/crew-cli --features telegram,discord,slack

# Or build locally
cargo build --release
./target/release/crew --help
```

## Quick Start

```bash
# Initialize configuration and workspace
crew init

# Set your API key
export ANTHROPIC_API_KEY=your-key-here

# Interactive chat
crew chat

# Run a one-shot task
crew run "Add a hello function to lib.rs"

# Check system status
crew status
```

## Commands

### `crew chat`

Interactive multi-turn conversation:

```bash
crew chat                          # Default provider
crew chat --provider openai        # Use OpenAI
crew chat --model gpt-4o           # Auto-detects OpenAI
crew chat --verbose                # Show tool outputs
```

### `crew run <goal>`

Execute a coding task:

```bash
crew run "Fix the bug in auth.rs"
crew run "Refactor the API" --provider deepseek --verbose
crew run "Add authentication" --coordinate    # Coordinator mode
crew run "Quick fix" --max-iterations 10 --max-tokens 50000
```

### `crew gateway`

Run as a persistent multi-channel messaging daemon:

```bash
crew gateway                       # Uses config from .crew/config.json
crew gateway --provider openai     # Override provider
crew gateway --verbose             # Verbose logging
```

### `crew init`

Initialize workspace with config and bootstrap files:

```bash
crew init              # Interactive setup
crew init --defaults   # Use defaults (Anthropic/Claude)
```

Creates:
- `.crew/config.json` - Configuration
- `.crew/AGENTS.md` - Agent instructions
- `.crew/SOUL.md` - Personality definition
- `.crew/USER.md` - User preferences
- `.crew/memory/`, `.crew/sessions/`, `.crew/skills/` directories

### `crew status [task-id]`

Show system or task status:

```bash
crew status              # System status (config, API keys, bootstrap files)
crew status abc123       # Task details
```

### `crew cron`

Manage scheduled cron jobs directly:

```bash
crew cron list                          # List active jobs
crew cron list --all                    # Include disabled jobs
crew cron add --name "daily" --message "Run report" --cron "0 0 9 * * * *"
crew cron add --name "check" --message "Check status" --every 3600
crew cron remove <job-id>
crew cron enable <job-id>               # Enable a job
crew cron enable <job-id> --disable     # Disable a job
```

### `crew channels status`

Show configured gateway channels and their compile/config status:

```bash
crew channels status
```

### Other Commands

```bash
crew resume [task-id]    # Resume interrupted task
crew list                # List resumable tasks
crew clean [--all]       # Clean up state files
crew completions <shell> # Generate shell completions
```

## Configuration

Config is loaded from `.crew/config.json` (project) or `~/.config/crew/config.json` (global).

### Basic config

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-4-20250514",
  "api_key_env": "ANTHROPIC_API_KEY"
}
```

### Gateway config

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-4-20250514",
  "gateway": {
    "channels": [
      {"type": "cli"},
      {"type": "telegram", "allowed_senders": ["123456"]},
      {"type": "slack", "settings": {"bot_token_env": "SLACK_BOT_TOKEN", "app_token_env": "SLACK_APP_TOKEN"}}
    ],
    "max_history": 50,
    "system_prompt": "You are a helpful assistant."
  }
}
```

### Supported Providers

| Provider | API Key Env | Default Model |
|----------|-------------|---------------|
| anthropic | `ANTHROPIC_API_KEY` | claude-sonnet-4-20250514 |
| openai | `OPENAI_API_KEY` | gpt-4o |
| gemini | `GEMINI_API_KEY` | gemini-2.0-flash |
| openrouter | `OPENROUTER_API_KEY` | anthropic/claude-sonnet-4-20250514 |
| deepseek | `DEEPSEEK_API_KEY` | deepseek-chat |
| groq | `GROQ_API_KEY` | llama-3.3-70b-versatile |
| moonshot | `MOONSHOT_API_KEY` | kimi-k2.5 |
| dashscope | `DASHSCOPE_API_KEY` | qwen-max |
| minimax | `MINIMAX_API_KEY` | MiniMax-Text-01 |
| zhipu | `ZHIPU_API_KEY` | glm-4-plus |
| ollama | (none) | llama3.2 |
| vllm | `VLLM_API_KEY` | (requires --model) |

Provider is auto-detected from model name when not specified (e.g., `--model gpt-4o` selects OpenAI).

## Architecture

```
crew-rs/
  crates/
    crew-core/      # Types, task model, message protocols
    crew-memory/    # Episodic memory, task store, memory store
    crew-llm/       # LLM provider abstraction (4 providers)
    crew-agent/     # Agent runtime, tools, skills, coordination
    crew-bus/       # Message bus, channels, sessions, cron, heartbeat
    crew-cli/       # CLI interface (chat, run, gateway, init, status)
```

### Built-in Tools

| Tool | Description |
|------|-------------|
| `shell` | Execute shell commands (SafePolicy) |
| `read_file` | Read file contents |
| `write_file` | Write/create files |
| `edit_file` | Edit files with search/replace |
| `glob` | Find files by pattern |
| `grep` | Search file contents (regex) |
| `list_dir` | List directory contents |
| `web_search` | Internet search |
| `web_fetch` | Fetch and parse web content |
| `message` | Send cross-channel messages |
| `spawn` | Launch background subagents |
| `cron` | Schedule recurring tasks |
| `delegate_task` | (Coordinator) Delegate subtask |
| `delegate_batch` | (Coordinator) Parallel delegation |

### Gateway Channels

| Channel | Feature Flag | Transport |
|---------|-------------|-----------|
| CLI | (built-in) | stdin/stdout |
| Telegram | `telegram` | teloxide (long poll) |
| Discord | `discord` | serenity (gateway) |
| Slack | `slack` | WebSocket (Socket Mode) |
| WhatsApp | `whatsapp` | WebSocket (Node.js bridge) |
| Feishu/Lark | `feishu` | WebSocket + REST |

## Development

```bash
cargo build --workspace           # Build
cargo test --workspace            # Test (133+ tests)
cargo clippy --workspace          # Lint
cargo fmt --all                   # Format
```

## License

Apache-2.0
