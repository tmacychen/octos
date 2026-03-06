# Session Actor Architecture

RFC for migrating the gateway from a monolithic shared-agent model to a per-session actor model.

**Status**: Phase 1-3 implemented, Phase 4-5 pending
**Author**: yuechen + Claude
**Date**: 2026-03-05

---

## 1. Problem Statement

### 1.1 Current Architecture

The gateway runs a single dispatch loop that receives all inbound messages and spawns a `tokio::spawn` task per message:

```
Channels (Telegram, Feishu, CLI, ...)
        │
        ▼
  ┌──────────────┐
  │  AgentHandle  │  single mpsc receiver
  │  (main loop)  │
  └──────┬───────┘
         │ tokio::spawn per message
    ┌────┼────┐
    ▼    ▼    ▼
  task  task  task    (one per inbound message)
    │    │    │
    ▼    ▼    ▼
  Arc<Agent>          (single shared agent)
  Arc<Mutex<SessionManager>>  (single shared session store)
  Arc<MessageTool>    (single shared tool, set_context() race)
  Arc<SendFileTool>   (same race)
  Arc<SpawnTool>      (same race)
```

### 1.2 Problems

**P1: Tool context race condition** (existing TODO in gateway.rs:1704)

`MessageTool`, `SendFileTool`, `SpawnTool`, and `CronTool` each hold a `Mutex<String>` for `default_channel` and `default_chat_id`. Before processing a message, the gateway calls `set_context()` on these shared tools. If two sessions run concurrently, their `set_context()` calls interleave, causing tool outputs to be routed to the **wrong chat**.

```
Session A: set_context("telegram", "alice")
Session B: set_context("telegram", "bob")    ← overwrites A's context
Session A: agent calls MessageTool            ← sends to "bob" instead of "alice"
```

**P2: Agent hook context race**

`Agent.hook_context` is a `std::sync::Mutex<Option<HookContext>>` — shared across all concurrent sessions. `set_session_id()` before processing can be overwritten by another session.

**P3: Serial processing within sessions, no queuing feedback**

Messages to the same session are serialized via a per-session lock (`session_locks: HashMap<String, Arc<Mutex<()>>>`). While one message is being processed, subsequent messages from the same user block silently — no "still working..." feedback, no queue depth visibility.

**P4: No cancellation**

If a user wants to cancel a long-running session (e.g., a pipeline that takes 10 minutes), there is no mechanism. The `shutdown: AtomicBool` is global — it stops all sessions.

**P5: Session lock map grows unbounded**

`session_locks: HashMap<String, Arc<Mutex<()>>>` is pruned periodically, but the pruning logic runs inside each spawned task and is racy. Stale entries accumulate.

**P6: Stateless spawned tasks**

Each `tokio::spawn` creates a fresh execution context. There is no continuity between consecutive messages to the same session — each task independently fetches history from `SessionManager`, sets up tools, creates a `TokenTracker`, etc. This repeated setup is wasteful and prevents stateful optimizations (e.g., keeping a warm LLM connection, caching tool specs).

---

## 2. Proposed Architecture: Session Actors

### 2.1 Overview

Replace the spawn-per-message model with long-lived **session actors**. Each actor is a `tokio::spawn` task that owns its session state, tools, and an `mpsc` inbox. The dispatcher routes messages to actors by session key.

```
Channels (Telegram, Feishu, CLI, ...)
        │
        ▼
  ┌──────────────┐
  │  Dispatcher   │   routes by SessionKey
  └──┬───┬───┬───┘
     │   │   │
     ▼   ▼   ▼
  ┌─────┐ ┌─────┐ ┌─────┐
  │Actor│ │Actor│ │Actor│   long-lived tokio tasks
  │  A  │ │  B  │ │  C  │   each owns: tools, history, inbox
  └──┬──┘ └──┬──┘ └──┬──┘
     │       │       │
     ▼       ▼       ▼
  shared: Arc<dyn LlmProvider>    (stateless, thread-safe)
  shared: Arc<EpisodeStore>       (stateless, thread-safe)
  shared: out_tx                  (mpsc sender, cloneable)
```

### 2.2 Core Types

#### SessionActor

```rust
/// A long-lived task that processes all messages for one session.
pub struct SessionActor {
    // --- Identity ---
    session_key: SessionKey,
    channel: String,        // e.g. "telegram"
    chat_id: String,        // e.g. "12345"

    // --- Inbox ---
    inbox: mpsc::Receiver<ActorMessage>,

    // --- Owned tools (no set_context race) ---
    message_tool: MessageTool,
    send_file_tool: SendFileTool,
    spawn_tool: Option<SpawnTool>,
    cron_tool: Option<CronTool>,
    pipeline_tool: Option<crew_pipeline::RunPipelineTool>,

    // --- Session state ---
    history: Vec<Message>,
    session_mgr: Arc<Mutex<SessionManager>>,

    // --- Shared (stateless) ---
    agent_config: AgentConfig,
    llm: Arc<dyn LlmProvider>,
    memory: Arc<EpisodeStore>,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    system_prompt: Arc<RwLock<String>>,
    hooks: Option<Arc<HookExecutor>>,

    // --- Output ---
    out_tx: mpsc::Sender<OutboundMessage>,

    // --- Status ---
    status_indicator: Option<Arc<StatusIndicator>>,

    // --- Lifecycle ---
    idle_timeout: Duration,
    shutdown: Arc<AtomicBool>,
}
```

#### ActorMessage

```rust
/// Messages sent to a session actor's inbox.
pub enum ActorMessage {
    /// A user message to process.
    Inbound(InboundMessage),
    /// Cancel the current operation.
    Cancel,
    /// Update configuration (hot-reload).
    ConfigUpdate(ActorConfigUpdate),
}
```

#### ActorRegistry

```rust
/// Manages the lifecycle of session actors.
pub struct ActorRegistry {
    /// Active actors: session_key → sender handle.
    actors: HashMap<String, ActorHandle>,
    /// Factory for creating new actors.
    factory: ActorFactory,
    /// Global concurrency semaphore.
    semaphore: Arc<Semaphore>,
}

pub struct ActorHandle {
    /// Send messages to this actor.
    tx: mpsc::Sender<ActorMessage>,
    /// When the actor started.
    created_at: Instant,
    /// JoinHandle for awaiting shutdown.
    join_handle: JoinHandle<()>,
}
```

#### ActorFactory

```rust
/// Creates SessionActors with the correct shared resources and per-session tools.
pub struct ActorFactory {
    llm: Arc<dyn LlmProvider>,
    memory: Arc<EpisodeStore>,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    system_prompt: Arc<RwLock<String>>,
    hooks: Option<Arc<HookExecutor>>,
    agent_config: AgentConfig,
    session_mgr: Arc<Mutex<SessionManager>>,
    out_tx: mpsc::Sender<OutboundMessage>,
    spawn_inbound_tx: mpsc::Sender<InboundMessage>,
    cron_service: Option<Arc<CronService>>,
    pipeline_config: Option<PipelineConfig>,
    status_indicators: HashMap<String, Arc<StatusIndicator>>,
    idle_timeout: Duration,
    shutdown: Arc<AtomicBool>,
}

impl ActorFactory {
    /// Create a new SessionActor with per-session tool instances.
    fn create(&self, session_key: &SessionKey, channel: &str, chat_id: &str)
        -> (mpsc::Sender<ActorMessage>, JoinHandle<()>)
    {
        let (tx, rx) = mpsc::channel(32);  // bounded inbox

        // Per-session tools — no set_context() needed
        let message_tool = MessageTool::with_context(
            self.out_tx.clone(), channel, chat_id);
        let send_file_tool = SendFileTool::with_context(
            self.out_tx.clone(), channel, chat_id);
        // ... other tools ...

        let actor = SessionActor { /* ... */ };
        let handle = tokio::spawn(actor.run());
        (tx, handle)
    }
}
```

### 2.3 Actor Lifecycle

```
                     create()
                        │
                        ▼
  ┌─────────────────────────────────────────┐
  │              RUNNING                     │
  │                                          │
  │  loop {                                  │
  │    select! {                             │
  │      msg = inbox.recv() => process(msg)  │
  │      _ = sleep(idle_timeout) => break    │
  │    }                                     │
  │  }                                       │
  └─────────────────┬───────────────────────┘
                    │
                    ▼
  ┌─────────────────────────────────────────┐
  │              SHUTDOWN                    │
  │                                          │
  │  1. Flush pending history to disk        │
  │  2. Remove self from ActorRegistry       │
  │  3. Task completes                       │
  └─────────────────────────────────────────┘
```

#### Run Loop

```rust
impl SessionActor {
    async fn run(mut self) {
        // Load history from disk on start
        self.load_history().await;

        loop {
            tokio::select! {
                msg = self.inbox.recv() => {
                    match msg {
                        Some(ActorMessage::Inbound(inbound)) => {
                            self.process_inbound(inbound).await;
                        }
                        Some(ActorMessage::Cancel) => {
                            self.cancel_current().await;
                        }
                        Some(ActorMessage::ConfigUpdate(update)) => {
                            self.apply_config(update);
                        }
                        None => break,  // all senders dropped
                    }
                }
                _ = tokio::time::sleep(self.idle_timeout) => {
                    tracing::debug!(session = %self.session_key, "idle timeout, shutting down actor");
                    break;
                }
            }
        }

        // Cleanup: persist any unsaved state
        self.flush().await;
    }
}
```

#### Message Processing

```rust
impl SessionActor {
    async fn process_inbound(&mut self, inbound: InboundMessage) {
        // 1. Start status indicator
        let token_tracker = Arc::new(TokenTracker::new());
        let status_handle = self.start_status(&inbound, &token_tracker);

        // 2. Build Agent for this turn
        //    (lightweight — reuses shared LLM, just builds message list)
        let agent = self.build_agent();

        // 3. Process through agent (long LLM call, but only this actor blocks)
        let response = agent
            .process_message_tracked(
                &inbound.content,
                &self.history,
                self.extract_media(&inbound),
                &token_tracker,
            )
            .await;

        // 4. Stop status indicator
        if let Some(handle) = status_handle {
            handle.stop().await;
        }

        // 5. Update history and persist
        match response {
            Ok(conv_response) => {
                self.append_user_message(&inbound);
                if !conv_response.content.is_empty() {
                    self.append_assistant_message(&conv_response);
                    // Send reply — always goes to this actor's chat
                    let _ = self.out_tx.send(OutboundMessage {
                        channel: self.channel.clone(),
                        chat_id: self.chat_id.clone(),
                        content: conv_response.content,
                        reply_to: None,
                        media: vec![],
                        metadata: serde_json::json!({}),
                    }).await;
                }
                // Maybe compact
                self.maybe_compact().await;
            }
            Err(e) => {
                tracing::error!(session = %self.session_key, error = %e, "processing failed");
                let _ = self.out_tx.send(OutboundMessage {
                    channel: self.channel.clone(),
                    chat_id: self.chat_id.clone(),
                    content: format!("Error: {e}"),
                    reply_to: None,
                    media: vec![],
                    metadata: serde_json::json!({}),
                }).await;
            }
        }
    }
}
```

### 2.4 Dispatcher (replaces main loop)

```rust
impl ActorRegistry {
    /// Route an inbound message to the correct actor, creating one if needed.
    async fn dispatch(&mut self, inbound: InboundMessage, session_key: SessionKey) {
        let key_str = session_key.to_string();

        // Get or create actor
        let handle = if let Some(h) = self.actors.get(&key_str) {
            h
        } else {
            let channel = session_key.channel().to_string();
            let chat_id = session_key.chat_id().to_string();
            let (tx, join_handle) = self.factory.create(
                &session_key, &channel, &chat_id);
            self.actors.insert(key_str.clone(), ActorHandle {
                tx,
                created_at: Instant::now(),
                join_handle,
            });
            self.actors.get(&key_str).unwrap()
        };

        // Send to actor (backpressure via bounded channel)
        match handle.tx.try_send(ActorMessage::Inbound(inbound)) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Actor is busy — send "still working" feedback
                let _ = self.factory.out_tx.send(OutboundMessage {
                    channel: session_key.channel().to_string(),
                    chat_id: session_key.chat_id().to_string(),
                    content: "⏳ Still processing your previous message, \
                              yours is queued...".to_string(),
                    reply_to: None,
                    media: vec![],
                    metadata: serde_json::json!({}),
                }).await;
                // Block until space available (or use try_send + drop)
                let handle = self.actors.get(&key_str).unwrap();
                let _ = handle.tx.send(ActorMessage::Inbound(inbound)).await;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Actor died — remove and retry
                self.actors.remove(&key_str);
                // Recursive retry (will create new actor)
                // In practice, use a loop instead of recursion
            }
        }
    }

    /// Periodically clean up completed actor handles.
    fn reap_dead_actors(&mut self) {
        self.actors.retain(|key, handle| {
            if handle.join_handle.is_finished() {
                tracing::debug!(session = %key, "reaping completed actor");
                false
            } else {
                true
            }
        });
    }
}
```

### 2.5 Session Switching & Notifications

When a user switches sessions (e.g., via topic commands), the old actor **keeps running**:

```
Time 0: User sends "research quantum computing" → Actor A starts (long pipeline)
Time 1: User switches to topic "code" → Actor B created for new topic
Time 2: User sends "fix this bug" → routed to Actor B, processed immediately
Time 3: Actor A finishes → sends OutboundMessage to Actor A's chat_id
         → User gets Telegram notification: "Research complete! Here are the findings..."
Time 4: User switches back to topic "research" → Actor A may have shut down (idle),
         but history is persisted; new Actor A loads from disk
```

Key properties:

- **Long-running sessions don't block other sessions**: Actor A runs independently
- **User gets notified when done**: `out_tx.send()` delivers to the correct chat regardless of which session the user is "viewing"
- **Queue feedback**: If Actor A's inbox is full when user sends another message, the dispatcher immediately replies "still working..."
- **Cancellation**: Dispatcher can send `ActorMessage::Cancel` to Actor A, which sets a per-actor shutdown flag checked at each LLM iteration

### 2.6 Concurrency Control

```
Global semaphore (max_concurrent_sessions)
    │
    ├── Actor A acquires permit on first inbound message
    │   └── releases when idle-timeout or shutdown
    │
    ├── Actor B acquires permit
    │   └── ...
    │
    └── Actor C blocks on acquire (at capacity)
        └── dispatcher sends "server busy, please wait" feedback
```

The semaphore permit is acquired when the actor **starts processing** (not when created). An idle actor holding no permit doesn't count toward the limit.

### 2.7 Memory Budget

| Component | Per-actor cost | Shared cost |
|---|---|---|
| `mpsc::Receiver` | ~64 bytes + buffer (32 msgs × ~1KB) | — |
| `MessageTool` | ~128 bytes (two Strings + Sender clone) | — |
| `SendFileTool` | ~128 bytes | — |
| `SpawnTool` | ~256 bytes (includes LLM Arc clone) | — |
| `CronTool` | ~128 bytes | — |
| `history: Vec<Message>` | Varies (typically 10-50 messages, ~50KB) | — |
| `TokenTracker` | ~8 bytes (two AtomicU32) | — |
| `LlmProvider` | — | ~1KB (shared via Arc) |
| `EpisodeStore` | — | ~10KB (shared via Arc) |
| `SessionManager` | — | ~1MB LRU cache (shared via Arc<Mutex>) |

**Per-actor total**: ~100KB typical (dominated by message history)
**1000 actors**: ~100MB — well within budget for a server process

---

## 3. Implementation Plan

### Phase 1: Tool Context Extraction (No behavior change)

**Goal**: Eliminate `set_context()` by giving tools their context at construction time.

**Files changed**:
- `crew-agent/src/tools/message.rs` — add `with_context()` constructor
- `crew-agent/src/tools/send_file.rs` — add `with_context()` constructor
- `crew-agent/src/tools/spawn.rs` — add `with_context()` constructor
- `crew-cli/src/cron_tool.rs` — add `with_context()` constructor

**Changes**:
1. Add `MessageTool::with_context(out_tx, channel, chat_id) -> Self` that sets defaults at construction. Keep `set_context()` for backward compatibility during migration.
2. Same for `SendFileTool`, `SpawnTool`, `CronTool`.
3. Add `ToolRegistry::new_for_session(channel, chat_id, ...)` factory method that builds a complete per-session registry.

**Tests**:
- Existing tool tests pass unchanged
- New test: `with_context()` tools route to correct chat without `set_context()`
- New test: two tool instances with different contexts don't interfere

**Verification**: Gateway still works with old `set_context()` path. No behavior change.

### Phase 2: SessionActor + ActorFactory (New code, not yet wired)

**Goal**: Implement `SessionActor`, `ActorFactory`, `ActorRegistry` as new types. Not yet used by gateway.

**Files added**:
- `crew-cli/src/actor.rs` — `SessionActor`, `ActorMessage`, `ActorHandle`
- `crew-cli/src/actor_registry.rs` — `ActorRegistry`, `ActorFactory`

**Key decisions**:
- Inbox channel buffer size: **32** (enough for burst, provides backpressure)
- Idle timeout: **30 minutes** (configurable via `GatewayConfig`)
- Actor reuses `Agent::process_message_tracked()` internally — no changes to Agent
- History loaded from `SessionManager` on actor start, appended on each turn
- Compaction triggered after each turn if threshold exceeded

**Tests**:
- Unit test: `SessionActor` processes a message and sends reply to `out_tx`
- Unit test: `SessionActor` shuts down after idle timeout
- Unit test: `ActorRegistry` creates actor on first message, reuses on second
- Unit test: `ActorRegistry` reaps dead actors
- Unit test: inbox backpressure — full inbox triggers "still working" feedback
- Unit test: `ActorMessage::Cancel` stops processing at next iteration boundary

### Phase 3: Wire Dispatcher (Replace gateway main loop) ✅

**Status**: Implemented

**Goal**: Replace the `tokio::spawn`-per-message dispatch in `gateway.rs` with `ActorRegistry::dispatch()`.

**Files changed**:
- `crew-cli/src/commands/gateway.rs` — replace dispatch section (~lines 1530-1630)

**Before** (current):
```rust
while let Some(inbound) = agent_handle.recv_inbound().await {
    // ... resolve session key ...
    let handle = tokio::spawn(async move {
        let _permit = semaphore.acquire().await;
        let _session_guard = session_lock.lock().await;
        process_session_message(&agent, &session_mgr, ...).await;
    });
}
```

**After**:
```rust
let mut registry = ActorRegistry::new(factory, semaphore);

while let Some(inbound) = agent_handle.recv_inbound().await {
    // ... resolve session key ...
    registry.dispatch(inbound, session_key).await;

    // Periodic cleanup
    if cleanup_interval.tick() {
        registry.reap_dead_actors();
    }
}

// Shutdown: cancel all actors
registry.shutdown_all().await;
```

**Removed**:
- `session_locks: HashMap<String, Arc<Mutex<()>>>` — replaced by actor inbox serialization
- `concurrency_semaphore` usage in spawned tasks — moved to actor lifecycle
- All `set_context()` calls — tools are constructed with context
- `process_session_message()` function — logic moves into `SessionActor::process_inbound()`

**Tests**:
- Integration test: two concurrent sessions don't interfere (tool output goes to correct chat)
- Integration test: messages to same session are processed in order
- Integration test: full inbox triggers queued feedback message
- Integration test: actor shuts down after idle, new message creates fresh actor, history preserved

### Phase 4: Cancellation Support

**Goal**: Allow per-session cancellation.

**Files changed**:
- `crew-agent/src/agent.rs` — accept per-call `CancellationToken` (or `Arc<AtomicBool>`)
- `crew-cli/src/actor.rs` — `Cancel` message sets actor's shutdown flag
- `crew-cli/src/commands/gateway.rs` — `/cancel` command sends `ActorMessage::Cancel`

**Changes**:
1. `SessionActor` holds a per-actor `Arc<AtomicBool>` shutdown flag
2. This flag is passed to `Agent` via a new `process_message_with_cancel()` method
3. Agent checks the flag in `check_budget()` (already checked each iteration)
4. When cancel is received, agent finishes current LLM call, then exits loop
5. Actor sends "Cancelled." reply and returns to inbox recv

**Tests**:
- Unit test: cancel stops processing after current iteration
- Unit test: cancel during tool execution waits for tool to finish, then stops

### Phase 5: Hot Reload & Observability

**Goal**: Leverage actor model for config hot-reload and monitoring.

**Changes**:
1. `ActorMessage::ConfigUpdate` — push new system prompt, max_history, etc. to live actors
2. `ActorRegistry::status()` — returns per-actor status (idle/processing, queue depth, uptime, token usage)
3. Expose via admin API endpoint: `GET /admin/actors`
4. `ConfigWatcher` sends updates to all actors via broadcast channel

**Tests**:
- Config update propagates to running actors
- Admin endpoint returns accurate actor status

---

## 4. Migration Strategy

### Incremental, backward-compatible

Each phase is independently shippable and testable:

| Phase | Ships as | Risk | Rollback |
|---|---|---|---|
| 1: Tool with_context | Additive API | None — old code still works | Delete new constructors |
| 2: Actor types | New files, unused | None — no runtime change | Delete files |
| 3: Wire dispatcher | **Breaking change** to gateway loop | Medium — new dispatch path | Revert gateway.rs to pre-actor version |
| 4: Cancellation | Additive API on Agent | Low | Remove cancel flag checks |
| 5: Hot reload | Additive | Low | Remove config update handling |

### Feature flag (optional)

Phase 3 can be gated behind a config flag during testing:

```json
{
  "gateway": {
    "actor_mode": true
  }
}
```

When `false`, falls back to the current `tokio::spawn`-per-message path. Remove the flag after validation.

---

## 5. Testing Plan

### 5.1 Unit Tests

| Test | Location | What it verifies |
|---|---|---|
| `tool_with_context_routes_correctly` | `crew-agent/src/tools/message.rs` | `with_context()` sets routing without `set_context()` |
| `two_tools_different_context` | `crew-agent/src/tools/message.rs` | Two MessageTool instances don't interfere |
| `actor_processes_message` | `crew-cli/src/actor.rs` | Single message → reply appears on `out_tx` |
| `actor_idle_shutdown` | `crew-cli/src/actor.rs` | Actor exits after `idle_timeout` with no messages |
| `actor_cancel` | `crew-cli/src/actor.rs` | `ActorMessage::Cancel` stops current processing |
| `actor_queue_serialization` | `crew-cli/src/actor.rs` | Two messages to same actor processed in order |
| `registry_creates_actor` | `crew-cli/src/actor_registry.rs` | First message creates actor |
| `registry_reuses_actor` | `crew-cli/src/actor_registry.rs` | Second message reuses existing actor |
| `registry_reaps_dead` | `crew-cli/src/actor_registry.rs` | Completed actors removed from map |
| `registry_backpressure` | `crew-cli/src/actor_registry.rs` | Full inbox → "still working" feedback sent |

### 5.2 Integration Tests

| Test | What it verifies |
|---|---|
| `concurrent_sessions_no_crosstalk` | Two sessions running simultaneously: each tool call routes to the correct chat. This is the **primary regression test** for P1 (set_context race). |
| `session_switch_long_running` | User starts long session A, switches to session B, sends message. B responds immediately. A finishes later and delivers notification to A's chat. |
| `actor_lifecycle_persist` | Actor processes messages → idles out → new message arrives → new actor loads history from disk → continues conversation. |
| `cancel_mid_processing` | Send cancel while agent is in tool-use loop. Agent finishes current tool, then stops. User gets "Cancelled." response. |
| `backpressure_feedback` | Fill actor inbox to capacity, send another message. Verify "still working..." response is sent immediately. |
| `graceful_shutdown` | Send shutdown signal. All actors finish current work, flush to disk, exit. No data loss. |
| `config_hot_reload` | Change system prompt while actors are running. Verify next message uses updated prompt. |

### 5.3 Stress Tests

| Test | Setup | Expected |
|---|---|---|
| `100_concurrent_sessions` | 100 sessions, each sending 5 messages. `max_concurrent_sessions = 20`. | All 500 messages processed. No crosstalk. Semaphore correctly bounds concurrency. |
| `rapid_fire_same_session` | 50 messages sent to one session in <1 second. | All queued, processed in order. Backpressure feedback after inbox fills. |
| `actor_churn` | Create 1000 actors, let them idle-timeout, verify memory returns to baseline. | No leaks. `ActorRegistry` map empty after all actors exit. |

### 5.4 Regression Tests

The existing gateway integration tests must continue to pass:
- `test_concurrent_sessions` (crew-bus session.rs)
- `test_concurrent_session_processing` (crew-bus session.rs)
- `test_evicted_session_reloads_from_disk` (crew-bus session.rs)
- All tool tests (`message.rs`, `send_file.rs`, `spawn.rs`)

### 5.5 Manual Testing Checklist

- [ ] Telegram: send message, get reply in correct chat
- [ ] Telegram: send two messages rapidly, both processed in order
- [ ] Telegram: start long pipeline in chat A, send quick question in chat B, get B's reply immediately
- [ ] Telegram: long pipeline finishes, notification appears in chat A
- [ ] Feishu: same as above
- [ ] CLI: `crew chat` still works (single-session mode)
- [ ] Gateway admin UI shows active actors and their status
- [ ] Shutdown gateway while sessions are active — no data loss, clean exit
- [ ] Hot-reload system prompt — next message uses new prompt

---

## 6. Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Actor leaks (never shuts down) | Memory growth | Idle timeout + periodic reaping + monitoring endpoint |
| Inbox overflow under load | Messages dropped | Bounded channel with explicit backpressure feedback; log warnings |
| Agent panics inside actor | Actor dies silently | `tokio::spawn` catches panics; registry reaps; next message creates new actor |
| Tool construction cost per actor | Startup latency | Tools are lightweight (~100 bytes each); benchmark to confirm |
| Config hot-reload races | Stale config | Use `tokio::sync::watch` for config broadcast; actors check on each turn |
| History divergence (actor cache vs disk) | Data inconsistency | Actor appends to SessionManager on each turn; flush on shutdown |

---

## 7. Future Extensions

Once the actor model is in place, several features become trivial:

| Feature | How |
|---|---|
| Per-session token budgets | Actor tracks cumulative usage, enforces limit |
| Session priority queues | Dispatcher sorts inbox by priority before sending |
| Multi-turn streaming | Actor holds SSE connection, streams tokens directly |
| Session transfer | Send actor's history to a different chat_id |
| Actor supervision | Registry restarts crashed actors with last-known state |
| Distributed gateway | Actors communicate via Redis pub/sub instead of in-process channels |
