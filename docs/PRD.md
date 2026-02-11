# Product Requirements Document: crew-rs

## Executive Summary

crew-rs is a Rust-native AI agent framework that provides both a coding automation CLI and a multi-channel messaging gateway. It supports 12+ LLM providers, 6 messaging channels, and a rich tool system for autonomous task execution.

## Problem Statement

1. **Fragmented AI tools**: Existing solutions are Python-based, slow, or tied to specific providers
2. **No multi-channel support**: Most coding agents only work via CLI
3. **No task persistence**: Interrupted sessions lose all progress
4. **Vendor lock-in**: Switching providers requires code changes
5. **No agent coordination**: Single-agent execution without task decomposition

## Target Users

- **Individual developers**: Automate coding tasks via CLI or chat
- **Teams**: Deploy AI assistant across Slack, Discord, Telegram, etc.
- **DevOps engineers**: Integrate AI into CI/CD with cron and scheduled tasks
- **International teams**: Chinese LLM support (DeepSeek, Qwen, Moonshot, Zhipu, MiniMax)

## Functional Requirements

### FR-1: Task Execution

| ID | Requirement | Status |
|----|-------------|--------|
| FR-1.1 | Execute coding tasks from natural language | Done |
| FR-1.2 | Interactive multi-turn chat mode | Done |
| FR-1.3 | Support iteration and token limits | Done |
| FR-1.4 | Real-time progress display | Done |
| FR-1.5 | Graceful interruption (Ctrl+C) with resume | Done |

### FR-2: LLM Providers

| ID | Requirement | Status |
|----|-------------|--------|
| FR-2.1 | Anthropic Claude | Done |
| FR-2.2 | OpenAI GPT | Done |
| FR-2.3 | Google Gemini | Done |
| FR-2.4 | OpenRouter (aggregator) | Done |
| FR-2.5 | OpenAI-compatible providers (DeepSeek, Groq, Moonshot, DashScope, MiniMax, Zhipu) | Done |
| FR-2.6 | Local deployment (Ollama, vLLM) | Done |
| FR-2.7 | Provider auto-detect from model name | Done |
| FR-2.8 | Automatic retry with exponential backoff | Done |
| FR-2.9 | Custom base URL support | Done |

### FR-3: Tool System

| ID | Requirement | Status |
|----|-------------|--------|
| FR-3.1 | Shell execution with SafePolicy | Done |
| FR-3.2 | File read/write/edit operations | Done |
| FR-3.3 | Glob and grep search | Done |
| FR-3.3b | Directory listing | Done |
| FR-3.4 | Web search and fetch | Done |
| FR-3.5 | Cross-channel messaging | Done |
| FR-3.6 | Background subagent spawning | Done |
| FR-3.7 | Cron job scheduling (interval, one-shot, cron expressions) | Done |
| FR-3.7b | Cron enable/disable | Done |
| FR-3.8 | Task delegation (coordinator mode) | Done |
| FR-3.9 | Parallel batch delegation | Done |

### FR-4: Gateway & Channels

| ID | Requirement | Status |
|----|-------------|--------|
| FR-4.1 | CLI channel (interactive) | Done |
| FR-4.2 | Telegram channel | Done |
| FR-4.3 | Discord channel | Done |
| FR-4.4 | Slack channel (Socket Mode) | Done |
| FR-4.5 | WhatsApp channel (Node.js bridge) | Done |
| FR-4.6 | Feishu/Lark channel | Done |
| FR-4.7 | Access control (allowed_senders) | Done |
| FR-4.8 | Session management per channel:chat_id | Done |
| FR-4.9 | Outbound message dispatch | Done |

### FR-5: Memory & Context

| ID | Requirement | Status |
|----|-------------|--------|
| FR-5.1 | Episodic memory (redb) | Done |
| FR-5.2 | Task state persistence (JSON) | Done |
| FR-5.3 | Long-term memory (MEMORY.md) | Done |
| FR-5.4 | Daily notes (YYYY-MM-DD.md) | Done |
| FR-5.5 | Recent memory (7-day window) | Done |
| FR-5.6 | Bootstrap files (AGENTS.md, SOUL.md, USER.md, TOOLS.md, IDENTITY.md) | Done |
| FR-5.7 | Skills system (SKILL.md with YAML frontmatter) | Done |
| FR-5.8 | Built-in skills (cron, github, skill-creator, summarize, tmux, weather) | Done |

### FR-6: Infrastructure

| ID | Requirement | Status |
|----|-------------|--------|
| FR-6.1 | Heartbeat service (periodic HEARTBEAT.md check) | Done |
| FR-6.2 | Cron scheduler (every/at/cron expression schedules) | Done |
| FR-6.5 | Config migration framework (versioned) | Done |
| FR-6.3 | Message bus (mpsc channels) | Done |
| FR-6.4 | Session persistence (JSONL) | Done |

### FR-7: CLI & UX

| ID | Requirement | Status |
|----|-------------|--------|
| FR-7.1 | `crew chat` - interactive conversation | Done |
| FR-7.2 | `crew run` - one-shot task execution | Done |
| FR-7.3 | `crew gateway` - multi-channel daemon | Done |
| FR-7.4 | `crew init` - workspace setup with bootstrap files | Done |
| FR-7.5 | `crew status` - system and task status | Done |
| FR-7.6 | `crew resume` - resume interrupted tasks | Done |
| FR-7.7 | `crew list` - list resumable tasks | Done |
| FR-7.8 | `crew clean` - cleanup state files | Done |
| FR-7.9 | Shell completions (bash/zsh/fish/powershell) | Done |
| FR-7.10 | `crew cron` - cron job management (list/add/remove/enable) | Done |
| FR-7.11 | `crew channels status` - channel status display | Done |

## Non-Functional Requirements

| ID | Requirement | Target | Status |
|----|-------------|--------|--------|
| NFR-1.1 | CLI startup time | < 50ms | Met |
| NFR-1.2 | Memory usage (idle) | < 50MB | Met |
| NFR-2.1 | State persistence durability | 100% on clean shutdown | Met |
| NFR-2.2 | Retry success rate | > 95% | Met |
| NFR-3.1 | No secrets in config files | Required | Met |
| NFR-3.2 | Shell command policy | SafePolicy implemented | Met |
| NFR-4.1 | Linux/macOS support | Required | Met |
| NFR-4.2 | Rust 1.85.0 MSRV | Required | Met |

## Technology Stack

- **Language**: Rust 2024 Edition (MSRV 1.85.0)
- **Async**: Tokio
- **HTTP**: Reqwest with rustls (no OpenSSL)
- **Database**: redb (embedded)
- **CLI**: Clap 4
- **Readline**: rustyline
- **Channels**: teloxide, serenity, tokio-tungstenite
- **Errors**: eyre/color-eyre

## Roadmap

### Completed
- [x] Core type system and task model
- [x] 4 native LLM providers + 8 OpenAI-compatible
- [x] 14 built-in tools
- [x] 6 messaging channels
- [x] Memory system (episodic + daily + long-term + bootstrap)
- [x] Skills system
- [x] Cron scheduler and heartbeat service
- [x] Interactive chat with readline
- [x] System status command
- [x] Provider auto-detect
- [x] Coordinator/worker pattern
- [x] Cron expression support (`"0 9 * * *"`)
- [x] Cron CLI subcommands (list/add/remove/enable)
- [x] Channels CLI subcommands (status)
- [x] Built-in skills (6 bundled)
- [x] Config migration framework

### Planned
- [ ] Telegram media handling (photos, voice)
- [ ] MCP server mode
- [ ] Streaming responses
- [ ] Custom tool plugins
- [ ] DingTalk, Email, QQ channels
