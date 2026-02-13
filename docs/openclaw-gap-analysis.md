# OpenClaw Gap Analysis for crew-rs

Based on analysis of [openclaw/openclaw](https://github.com/openclaw/openclaw) (Feb 2026).

## Current Parity

crew-rs already has: tool registry, path traversal protection, MCP support, session management, shell sandbox (bwrap/macOS), cron scheduling, SSRF protection, plugin system.

## High-Value Gaps

### 1. Tool Policy System (Priority: HIGH, Effort: Medium ~150 lines)

OpenClaw uses 6-layer hierarchical tool filtering:
1. Tool profiles (minimal, coding, messaging, full)
2. Global allow/deny lists
3. Provider-specific overrides
4. Agent-level overrides
5. Group-based permissions
6. Sandbox restrictions

Key patterns:
- `ToolPolicy { allow: Vec<String>, deny: Vec<String> }` with deny-wins semantics
- Wildcard matching: `exec*` matches `exec`, `exec_bg`
- `alsoAllow` for additive policies without override
- Owner-only tool gates (restrict sensitive tools to session owner)

**crew-rs today**: Flat registry with `retain()` filter. No allow/deny, no profiles, no per-provider policies.

### 2. Tool Groups (Priority: HIGH, Effort: Small ~50 lines)

Named groups that expand to tool sets:
- `group:fs` -> read_file, write_file, edit_file, diff_edit
- `group:runtime` -> shell
- `group:memory` -> memory_search, memory_get
- `group:web` -> web_search, web_fetch
- `group:sessions` -> spawn

Enables concise policy expressions like `{ allow: ["group:fs", "group:web"] }`.

**crew-rs today**: No grouping concept.

### 3. Context Compaction (Priority: HIGH, Effort: Medium ~200 lines)

Token-aware message summarization when context window fills:
- Estimate tokens per message
- Split history into chunks by token count
- Summarize old chunks to 40% of original size (BASE_CHUNK_RATIO)
- Never compress below 15% (MIN_CHUNK_RATIO)
- Strip `toolResult.details` from summaries (security: untrusted payloads)
- Keep recent messages intact
- Safety margin: 1.2x buffer for estimation inaccuracy

**crew-rs today**: Simple history truncation by message count. No token-aware compaction.

### 4. Config Hot-Reload (Priority: MEDIUM, Effort: Medium ~120 lines)

Chokidar watcher with hierarchical reload rules:
- SHA-256 hash to detect changes
- Debounce 300ms to prevent thrashing
- Rules per config path: hot (live update) vs restart vs ignore
- Hot-applicable: channels, agents, tools, cron, hooks, sessions
- Restart-required: gateway port, bind, auth, TLS

**crew-rs today**: Static config, requires full restart.

### 5. Hybrid Memory Search (Priority: HIGH, Effort: Large ~500 lines)

Vector + BM25 hybrid search:
- Backend: SQLite with vector extension
- Chunking: 400-token chunks with 80-token overlap
- Ranking: 0.7 * vectorScore + 0.3 * bm25Score
- Candidate multiplier: fetch 4x, re-rank to top N
- Sources: workspace markdown + session transcripts
- Min score threshold: 0.35, max results: 6
- Incremental sync: watch files, index on search

**crew-rs today**: Simple MEMORY.md + daily notes (7-day window). No semantic search.

### 6. Streaming Block Coalescing (Priority: LOW, Effort: Small ~80 lines)

Channel-aware response chunking:
- Break preference: paragraph > newline > sentence > length
- Configurable min/max chars per block
- Channel-specific limits (Discord shorter, Telegram longer)
- Flush on paragraph boundary

**crew-rs today**: Basic SSE broadcast, no intelligent chunking.

### 7. Session Forking (Priority: LOW, Effort: Small ~60 lines)

Parent UUID tracking for branched conversations:
- `/new` creates child session with parent reference
- Preserves thinking/verbose level overrides across fork
- Delivery context tracking (last channel/recipient)

**crew-rs today**: Single linear session per key.

### 8. Docker Sandbox (Priority: MEDIUM, Effort: Large ~300 lines)

Container pooling with scope isolation:
- Scope: session (most isolated), agent (balanced), shared (efficient)
- Hot container reuse: 5-minute window
- Workspace mount modes: none, read-only, read-write
- Resource limits: CPU, memory, PIDs, ulimits
- Environment sanitization: block LD_PRELOAD, NODE_OPTIONS, etc.
- Container prefix naming for cleanup

**crew-rs today**: bwrap (Linux) and sandbox-exec (macOS) only. No container support.

### 9. Provider-Specific Tool Policies (Priority: LOW, Effort: Small ~40 lines)

Different tool sets per LLM model:
- `tools.byProvider["openai/gpt-4"]` -> { allow, deny }
- Match model ID at runtime, filter before agent invocation
- Useful for models that don't support certain tool schemas

**crew-rs today**: Same tools for all providers.

## Implementation Order

```
Phase A (Foundation):  Tool groups -> Tool policies -> Provider policies
Phase B (Intelligence): Context compaction -> Hybrid memory search
Phase C (Operations):  Config hot-reload -> Docker sandbox
Phase D (Polish):      Streaming coalescing -> Session forking
```

Each phase is independently shippable. Phases A and B have highest ROI.
