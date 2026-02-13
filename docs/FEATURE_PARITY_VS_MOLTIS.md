# Feature Parity: crew-rs vs Moltis

Comparison of [crew-rs](https://github.com/heyong4725/crew-rs) (6-crate workspace, coding agent CLI + gateway) against [Moltis](https://github.com/moltis-org/moltis) (27-crate workspace, local-first AI gateway with web UI).

Both are Rust, single-binary, multi-provider, sandboxed agent frameworks.

---

## Features crew-rs ALREADY HAS (parity)

| Feature | crew-rs | Moltis |
|---|---|---|
| Multi-provider LLM | 4 native + 8 compatible (12 total) | 16+ providers |
| Streaming SSE | Per-provider parsers | WebSocket streaming |
| Tool system | 13 built-in tools | Similar set |
| Sandbox | bwrap / sandbox-exec / Docker | Docker / Apple Container |
| MCP support | JSON-RPC stdio | stdio + HTTP/SSE |
| Skills system | SKILL.md + built-ins | SKILL.md + built-ins |
| Plugin system | manifest.json + executable | Similar |
| Session persistence | JSONL + in-memory cache | JSONL + SQLite metadata |
| Memory/embeddings | BM25 + HNSW hybrid search | SQLite vector + full-text |
| Cron/scheduling | CronService with at/every/cron | Similar |
| Channels | CLI, Telegram, Discord, Slack, WhatsApp, Feishu, Email (7) | Web, Telegram, Discord (3) |
| Sub-agent spawning | spawn tool (sync + background) | spawn_agent tool |
| Context compaction | 80% threshold, summary | 95% threshold, summary |
| Retry with backoff | RetryProvider (3 retries) | ProviderChain failover |
| Tool policies | allow/deny with groups | Hook-based gating |
| SSRF protection | Private IP blocking | DNS resolution + IP blocking |
| Shell safety | SafePolicy deny/ask patterns | Hook-based blocking |
| Audio transcription | Groq Whisper | STT providers |
| Image/vision | Base64 encoding per provider | Similar |
| Config hot-reload | SHA-256 polling, 5s | Similar |

---

## Features Moltis HAS that crew-rs LACKS

| # | Feature | Moltis Implementation | Improvement Potential for crew-rs |
|---|---|---|---|
| 1 | **Hook/Lifecycle System** | 17 lifecycle events (BeforeToolCall, BeforeLLMCall, MessageSending, etc.). Sequential for modifying, parallel for read-only. Shell protocol (JSON stdin, exit code + stdout). Circuit breaker (3 failures -> auto-disable). HOOK.md discovery. | **HIGH** -- Would enable user-defined approval workflows, audit logging, content filtering, and tool gating without code changes. Could replace SafePolicy with a more flexible hook-based approach. |
| 2 | **Built-in Web UI** | SPA embedded via `include_dir!()`. WebSocket streaming. Settings panel. Hook editor. Session browser. | **MEDIUM** -- crew-rs has a REST API (feature-gated) but no bundled UI. Could embed a simple SPA for session browsing and config editing. |
| 3 | **Provider Circuit Breaker / Failover** | ProviderChain with automatic failover on retriable errors. Circuit breaker per provider (degrades on failure, resets on success). | **HIGH** -- crew-rs has RetryProvider for single provider retries but no multi-provider failover chain. Adding a ProviderChain would improve reliability. |
| 4 | **WebAuthn / Passkey Auth** | FIDO2 credentials (Touch ID, security keys). Stored in SQLite. | **LOW** -- crew-rs targets CLI/bot use cases where passkeys are less relevant. |
| 5 | **Parallel Tool Execution** | `futures::join_all` when LLM requests multiple tool calls in one turn. | **HIGH** -- crew-rs executes tool calls sequentially. Parallel execution would significantly reduce latency for multi-tool turns. |
| 6 | **Apple Container** (macOS native containers) | Native macOS containerization beyond sandbox-exec. | **LOW** -- crew-rs already has sandbox-exec. Apple Container is newer/niche. |
| 7 | **Browser Automation** | Playwright-based browser tool with session pool. | **MEDIUM** -- Would enhance web interaction capabilities beyond fetch/search. |
| 8 | **TTS (Text-to-Speech)** | Multiple TTS providers (ElevenLabs, etc.). | **LOW** -- Niche for CLI/bot agent. |
| 9 | **Onboarding Wizard** | Guided first-run setup for identity, profile, personality. | **LOW** -- crew-rs has `crew init` which is simpler but sufficient. |
| 10 | **Tool Result Sanitization** | Strips base64 data URIs, long hex strings, redacts secrets from output before feeding back to LLM. | **MEDIUM** -- crew-rs truncates output but doesn't redact secrets/base64 from tool results. |
| 11 | **Wall-Clock Agent Timeout** | 600s hard timeout independent of iterations. | **MEDIUM** -- crew-rs has max_iterations but no wall-clock timeout. Runaway tool calls could hang indefinitely. |
| 12 | **Sandbox Image Management** | CLI commands: `sandbox list/build/clean/remove`. Deterministic image tags (hash of base + packages). Auto-rebuild on package change. | **LOW** -- Nice UX but not critical. |
| 13 | **Message Queue Modes** | `followup` (replay each queued message) vs `collect` (concatenate). Handles messages arriving during active agent run. | **MEDIUM** -- crew-rs doesn't have explicit handling for messages arriving during an active run. |
| 14 | **MCP HTTP/SSE Transport** | Supports both stdio and HTTP/SSE remote MCP servers. Health polling with restart backoff. | **MEDIUM** -- crew-rs only supports stdio MCP transport. HTTP/SSE would enable remote MCP servers. |
| 15 | **Prometheus Metrics** | `/metrics` endpoint, SQLite history for metrics. | **LOW** -- Observability improvement for production deployments. |
| 16 | **DNS-Based SSRF** | Resolves DNS before HTTP request, blocks private IPs at resolved address level. | **MEDIUM** -- crew-rs checks URL hostname but doesn't resolve DNS first, leaving a DNS rebinding gap. |
| 17 | **`secrecy::Secret<String>`** | Secrets use wrapper that redacts Debug, prevents Display, zeroes memory on drop. | **MEDIUM** -- crew-rs stores API keys as plain `String`. |
| 18 | **`#![deny(unsafe_code)]`** | Workspace-wide. | **LOW** -- Easy to add as a lint. |
| 19 | **fd-lock for Sessions** | File-level locking prevents concurrent JSONL corruption. | **LOW** -- crew-rs uses atomic write-then-rename which is mostly safe. |
| 20 | **Per-IP Rate Limiting** | Built-in throttling for unauthenticated traffic. `429 + Retry-After`. | **LOW** -- Only relevant for the REST API feature. |

---

## Features crew-rs HAS that Moltis LACKS

| Feature | crew-rs | Notes |
|---|---|---|
| **WhatsApp channel** | WebSocket bridge to Baileys | Moltis doesn't have this |
| **Slack channel** | Socket Mode + REST | Moltis doesn't have this |
| **Feishu/Lark channel** | WebSocket + REST | Moltis doesn't have this |
| **Email channel** | IMAP + SMTP | Moltis doesn't have this |
| **Heartbeat service** | Periodic HEARTBEAT.md check | Unique to crew-rs |
| **diff_edit tool** | Unified diff with fuzzy matching | Moltis uses standard edit |
| **Pricing module** | Per-model cost tracking | Not mentioned in Moltis |
| **OAuth PKCE for OpenAI** | Browser + device code flows | Moltis has OAuth but less documented |

---

## Recommended Improvements (Prioritized)

### Tier 1 -- High Impact, Moderate Effort

1. **Parallel tool execution** -- Use `futures::join_all` for concurrent tool calls when LLM requests multiple tools in one turn. Significant latency reduction for multi-tool responses.

2. **Provider failover chain** -- Wrap multiple providers with automatic failover and circuit breaker. When one provider returns a retriable error (429, 5xx), transparently try the next. Track failure counts per provider, auto-degrade after threshold, reset on success.

3. **Hook/lifecycle system** -- Even a simplified version with key events (BeforeToolCall, AfterToolCall, BeforeLLMCall) and shell protocol (JSON stdin, exit code control flow) would add powerful extensibility. HOOK.md discovery with eligibility checks (requires_bins, requires_env). Circuit breaker for auto-disabling broken hooks.

### Tier 2 -- Medium Impact

4. **Wall-clock agent timeout** -- Add `tokio::time::timeout` around the entire agent loop (default 600s). Currently only max_iterations limits execution, but a single long-running tool call could hang indefinitely.

5. **Tool output sanitization** -- Strip base64 data URIs (`data:...;base64,...`) and long hex strings from tool results before feeding back to LLM. Reduces context waste and prevents accidental secret leakage.

6. **MCP HTTP/SSE transport** -- Support remote MCP servers via HTTP/SSE in addition to stdio. Would enable connecting to hosted MCP services without local process spawning.

7. **DNS-based SSRF protection** -- Resolve DNS and check resolved IP addresses (not just hostname patterns) before making HTTP requests in web_fetch. Prevents DNS rebinding attacks where a hostname resolves to a private IP.

### Tier 3 -- Low Priority / Nice-to-Have

8. **`secrecy::Secret<String>`** for API keys -- Prevents accidental logging of credentials via Debug/Display traits.

9. **`#![deny(unsafe_code)]`** workspace-wide -- Easy lint to add for safety assurance.

10. **Built-in web UI** -- Embed a simple SPA for session browsing and config editing (significant effort).

11. **Prometheus metrics endpoint** -- `/metrics` for production observability.

12. **Message queue modes** for gateway -- Handle messages arriving during an active agent run (followup vs collect modes).

---

*Analysis date: 2026-02-13*
*Sources: [Moltis GitHub](https://github.com/moltis-org/moltis), [DeepWiki](https://deepwiki.com/moltis-org/moltis)*
