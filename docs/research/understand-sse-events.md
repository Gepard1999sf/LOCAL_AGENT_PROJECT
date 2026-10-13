# CodeWhale thread SSE stream: server contract vs CodeWhale Desktop client

Paths in this report:
- **S:** = `/tmp/claude-0/-home-user-LOCAL-AGENT-PROJECT/dfc6ae91-fcd0-5718-90b8-d857a427b4cb/scratchpad/cw-src` (upstream v0.10.2)
- **C:** = `/home/user/LOCAL_AGENT_PROJECT/codewhale-desktop`

No files were edited.

---

## 0. Server transport contract (`GET /v1/threads/{id}/events`)

| Aspect | Contract | Evidence |
|---|---|---|
| Route | `GET /v1/threads/{id}/events` → `stream_thread_events` | S:crates/tui/src/runtime_api.rs:2611, 9052 |
| Query | `since_seq: u64?`, `replay_limit: usize?`, `progress: bool` (default false) | S:runtime_api.rs:1509-1515 |
| Cursor precedence | `since_seq` query wins. `Last-Event-ID` is only a fallback, and only if it is a decimal number; anything else is ignored. | S:runtime_api.rs:9064-9070, 9918-9923 |
| Thread check | `get_thread` runs first. Unknown thread → error (404). It also flushes queued crash-recovery receipts. | S:runtime_api.rs:9058-9062; S:crates/tui/src/runtime_threads.rs:10948-10953 |
| No handoff gap | The live broadcast is subscribed **before** durable history is read. Duplicates are dropped with `event.seq <= last_seq`. | S:runtime_api.rs:9072-9075, 9193-9195 |
| replay_limit | Hard maximum 4096, otherwise 400. It is a **tail** limit: older events are skipped silently and `base_seq` moves past them. | S:runtime_api.rs:9076-9083; S:runtime_threads.rs:75, 4450-4483 |
| Journal frame | `event: <name>`, `id: <seq>`, `data: <envelope JSON>` | S:runtime_api.rs:9188-9207, 9907-9910 |
| Keepalive | SSE comment `:keepalive` every 15 s | S:runtime_api.rs:9100-9105 |
| Headers | `x-codewhale-stream-end: 1` (every server-initiated end sends a `stream.end` frame). `x-codewhale-event-progress: 1` when `progress=true`. | S:runtime_api.rs:9107-9117 |
| Lag recovery | A broadcast `Lagged` triggers a durable re-read from `last_seq`. With `progress=true` the server re-sends `stream.progress{replaying}`. | S:runtime_api.rs:9312-9360 |
| Broadcast capacity | 1024 events, shared by all subscribers. Each connection filters by thread id. | S:runtime_threads.rs:73, 7699; S:runtime_api.rs:9193 |
| `seq` numbering | **One counter for the whole store** (`state.next_seq`), persisted in state.json. A thread's seqs are monotonic but have **gaps**. | S:runtime_threads.rs:4141-4146 |
| Delta coalescing | Message and reasoning deltas are batched up to 32 ms or 16 KB, so a long answer produces about 30 `item.delta` events per second. | S:runtime_threads.rs:80-81, 197-241 |
| Canonical snapshot | `GET /v1/threads/{id}` → `ThreadDetail{thread, turns, items, latest_seq, pending_approvals, pending_user_inputs, pending_dynamic_tool_calls, approval_grants}`. It is taken under the projection lock, so subscribing with `since_seq=latest_seq` loses nothing and repeats nothing. | S:runtime_threads.rs:5284-5309, 11926-11985; S:runtime_api.rs:2485, 7328-7338 |

### Envelope (data of every journal frame)
`RuntimeEventEnvelope{schema_version, seq, event, kind(=event), thread_id, turn_id?, item_id?, timestamp, created_at, payload, ...extra}` (S:crates/protocol/src/runtime/mod.rs:25-42; built at S:runtime_api.rs:9675-9693).

The SSE stream also adds **`previous_seq`**, the cursor of the previous frame on this connection. This is the only correct way to detect a gap, because seqs are not contiguous (S:runtime_api.rs:9695-9704, 9200-9203).

---

## 1+2. Event catalog and client handling

Client reducer: C:src/store/threads.ts:211-354. Side effects: C:src/hooks/useBootstrap.ts:9-43. Rust proxy: C:src-tauri/src/api.rs:119-233.

### Transport frames (no `id:` and not stored in the journal)
| Event | Payload | Server | Client |
|---|---|---|---|
| `stream.progress` | `{schema_version,event,kind,thread_id,seq,state:"replaying"\|"live"}` | runtime_api.rs:9124-9133, 9246, 9286-9290, 9313-9316 | Sets `live=true` on "live" only and never resets it on "replaying" (threads.ts:217-219) |
| `stream.end` | `{...,reason:"replay_failed"\|"catch_up_failed"\|"runtime_shutdown", last_seq, retryable}` | runtime_api.rs:9139-9184 | Rust breaks the read loop and reconnects after 800 ms. `retryable` is ignored (api.rs:178-185, 189-190). The frontend ignores the frame (default branch). |
| keepalive | `:keepalive` comment | runtime_api.rs:9104 | Dropped because data is empty (api.rs:215-217, 229-231). No idle watchdog. |

### Thread events
| Event | Payload | Server | Client |
|---|---|---|---|
| `thread.started` | `{thread}` | runtime_threads.rs:10294-10300 | Merges `p.thread` (threads.ts:220-224) |
| `thread.updated` | `{thread, changes}` / `{thread, changes:{session_id}}` / `{thread, reason:"session_resume"}` | runtime_threads.rs:11310-11318, 11915-11921, 13548-13554 | Merges the thread, then `refreshThreads()` (useBootstrap.ts:39-41) |
| `thread.forked` | `{thread, source_thread_id[, backtrack_depth_from_tail, dropped_turn_id]}`, written to the **new** thread's log | runtime_threads.rs:12341-12350, 12951-12962 | Merges the thread only |
| `thread.permissions_changed` | `{from,to,source}` | runtime_threads.rs:11370-11376 | ignored |
| `thread_goal_updated` / `thread_goal_cleared` (no dot in the name) | `{kind, goal}` | runtime_threads.rs:8793-8813 | ignored |
| `agent_mail.queued/delivering/delivered/read/delivery_failed/canceled` | `{mail:{message_id, attempt_count,...}}` | S:crates/protocol/src/agent_mail.rs:19-24; runtime_threads.rs:9587-9615 | ignored |

### Turn events
| Event | Payload | Server | Client |
|---|---|---|---|
| `turn.started` | `{turn: TurnRecord[, manual_compaction:true]}` | runtime_threads.rs:13746-13764 | Merges the TurnRecord and sets `activeTurnId` (threads.ts:225-236) |
| `turn.lifecycle` | `{status:"in_progress"}` | runtime_threads.rs:16775-16781 | Patches status (threads.ts:237-244) |
| `turn.usage` (one per model request) | `{usage:{input_tokens,output_tokens,prompt_cache_*,reasoning_tokens...}, duration_ms, first_token_ms, request_ms, maxOutputTokens}` (last key is camelCase) | runtime_threads.rs:18375-18393; S:crates/tui/src/core/events.rs:352-370 | Adds to `outputTokens` and `requests`; tok/s = out / duration_ms (threads.ts:245-258) |
| `turn.workspace_snapshot` | `WorkspaceSnapshotRef{kind, snapshot_id, tree_id, session_id, tool_call_id?, write_paths?, changed_paths?}` | runtime_threads.rs:18423-18460; S:crates/tui/src/snapshot/mod.rs:106-134 | Replaces `turn.changedPaths` and increments `filesChanged` (threads.ts:259-266) |
| `turn.completed` (**the only terminal turn event**; failure, interrupt and cancel are carried in `turn.status` and `turn.error`) | `{turn[, recovered:true]}` | runtime_threads.rs:9703-9733, 18712, 14005, 9812 | Merges, clears `activeTurnId`; refreshes the thread list and context (threads.ts:225-236; useBootstrap.ts:33-38) |
| `turn.interrupt_requested` | `{thread_id, turn_id}` | runtime_threads.rs:15056-15062 | ignored |
| `turn.steered` / `turn.steer_dropped` | `{thread_id,turn_id,input[,reason,item]}` | runtime_threads.rs:14199-14243 | ignored (the client never steers) |
| `turn.artifacts` | `{turn_id, workspace, artifacts}` (sent after the turn ends) | runtime_threads.rs:18939-18950 | ignored |
| `model.tools.snapshot` | `{snapshot, projection_redacted}` (about 36 KB) | runtime_threads.rs:18395-18421 | Dropped in Rust; the seq still advances (api.rs:170-177) |
| `turn.failed` / `turn.interrupted` / `turn.canceled` | **never emitted** on this stream. They exist only in exec mode (S:crates/tui/src/exec_agent.rs:1282-1283). | — | Dead branches at threads.ts:227-229, 234 and useBootstrap.ts:34-35 |

### Item events (`item` = `TurnItemRecord`)
Kinds: `user_message|agent_message|agent_reasoning|tool_call|file_change|command_execution|context_compaction|status|error`. Statuses: `queued|in_progress|completed|failed|interrupted|canceled` (runtime_threads.rs:776-799). Tool kind comes from the tool name (runtime_threads.rs:19546-19555).

| Event | When / payload | Server | Client |
|---|---|---|---|
| `item.started` | user msg `{item}`; agent msg / reasoning `{item}`; tool `{item, tool:{id,name,input}}` with metadata `tool_use_id, tool_name, tool_input(string), execution_id, provider_tool_use_id?, visibility?`; compaction `{item, auto}` | 13776-13784, 16802-16827, 16880-16905, 16955-17016, 17366-17392 | `upsertItem(...,false)` (threads.ts:267-269) |
| `item.delta` | `{delta, kind:"agent_message"\|"agent_reasoning"}`; no tool-output deltas exist | 16851-16858, 16926-16933 | Appends to `text` and creates the item if it is unknown (threads.ts:274-296) |
| `item.completed` | agent msg / reasoning / tool `{item}`; compaction `{item, auto, messages_before, messages_after}`; **status items without a preceding started** (publish_status_item, sub-agents, tool projection warning) | 16862-16876, 16936-16952, 17326-17337, 17394-17449, 16530-16572, 18306-18344 | `upsertItem(...,true)` (threads.ts:270-273) |
| `item.failed` | tool failed; compaction failed `{item,auto}`; engine error (kind `error`); empty turn; monitor crash `{item, error}` | 17326-17337, 17469-17485, 18346-18373, 18614-18640, 13929-13935 | Handled as completed with status `failed` |
| `item.interrupted` | Open message or reasoning item when the turn is interrupted | 18555-18609 | **NOT handled** |
| `item.canceled` | Compaction cancelled `{item, auto}` | 17451-17467 | **NOT handled** |

### Approvals, user input, sandbox
| Event | Payload | Server | Client |
|---|---|---|---|
| `approval.required` | `{id, approval_id, tool_call_id, tool_name, summary(string), description, intent_summary}`. Also emitted for posture or grant **auto-approved** calls, immediately followed by `decided{auto:true}`. | 17797-17816, 17900-17919, 17967-17986 | Adds an approval card and an `approval:<id>` block. Client-side auto-approve applies if `t.live` (threads.ts:297-321; useBootstrap.ts:13-31). |
| `approval.decided` | `{approval_id, tool_call_id, decision:"allow"\|"deny", remember, auto?, posture?, grant_id?, cancelled?, timeout?}`. A posture auto-deny sends `decided` **without** a `required`. | 17823-17836, 17858-17872, 17921-17934, 18058-18070, 18091-18103, 18129-18141, 18147-18158, 18184-18196 | Updates the status. Unknown ids are ignored, so posture denials are invisible (threads.ts:322-333). |
| (no event) | Decision channel closed without an answer: no `approval.decided` is sent | 18163-18168 | Card stays pending |
| `approval.timeout` | `{approval_id, tool_call_id, timeout_secs}` | 18169-18181 | Status "timeout" |
| `approval.grant_added` / `grant_revoked` | `{grant}` | 8735-8741, 8773-8779, 11276-11282 | ignored |
| `sandbox.denied` | `{tool_id, tool_name, reason}` | 18207-18223 | ignored |
| `user_input.required` | `{id, request: UserInputRequest[, delivery_error]}`. Answer with `POST /v1/user-input/{thread_id}/{input_id}`. | 18264-18286, 8472-8481; route runtime_api.rs:2641-2644, 5797-5822 | Stored in `userInputs` (threads.ts:334-339), **but no component renders it** (no `userInputs` reference outside threads.ts) |
| `user_input.answered` / `user_input.canceled` | `{id, input_id[, terminal]}` | 8420-8441 | Status update only |

### Tools, operations, agents, failures
| Event | Payload | Server | Client |
|---|---|---|---|
| `tool.execution_started` | `{execution_id}` | 17089-17099 | ignored |
| `operation.activity_started/completed/heartbeat` | `{span_id, activity_kind(reading\|editing\|...\|thinking\|responding), action_id?, outcome?, observed_at}` | 17018-17087; S:crates/protocol/src/engine_owner.rs:27-69 | ignored |
| `agent.spawned/progress/completed/list` | `{item(kind status), agent_id, agent_name?, worker_status, step?, parent_run_id, spawn_depth, continuable?, usage?, agents?}` | 17487-17680 | ignored, so sub-agent status rows never appear |
| `tool_call.requested/resolved/timeout/canceled` | `DynamicToolCallParams` / result | 19341-19347, 9934-9959, 9760 | ignored (the desktop registers no dynamic tools) |
| `runtime.store_failure` | `RuntimeStoreFailureNotice{record kind/id/path, error, reason, next_action, message, terminal?}` | 2190, 2328-2344, 9833-9867 | **ignored** |

---

## 3. Mismatches and bugs (most severe first)

**B1. History is rebuilt from events, but the event log is not a complete history.**
- C:src/store/app.ts:171-174 (openThread), 188-189 (newThread), C:src/components/layout/Sidebar.tsx:48-54 all call `subscribe(id, 0)` on an empty store.
- Rust always sends `replay_limit=4096` (C:src-tauri/src/api.rs:129).
- (a) **Long threads lose their early history.** About 30 deltas per second (S:runtime_threads.rs:80) means a few reasoning-heavy turns from a local model exceed 4096 events. The server then silently replays only the tail (S:runtime_threads.rs:4466-4475). Older turns vanish, and a turn whose `turn.started` or user item was cut shows up as an untitled turn.
- (b) **Forked threads look empty.** `publish_fork` copies turns and items but no events (S:runtime_threads.rs:12982-13031). The fork's log holds only `thread.forked`, so `forkThread` → `openThread` (C:app.ts:270-279) shows an empty chat.
- (c) **"Resume terminal session" looks empty.** `seed_thread_from_messages_*` writes turns and items directly and emits only `thread.updated{reason:"session_resume"}` (S:runtime_threads.rs:13049-13554; the emit is at 13548). `resumeSession` (C:app.ts:282-290) therefore shows none of the imported conversation.
- **Fix:** load `GET /v1/threads/{id}`, hydrate turns, items, pending approvals and pending user inputs, then `cw_subscribe(id, latest_seq)`.

**B2. Opening an old thread triggers a burst of HTTP requests.**
- `onEvent` runs `refreshThreads()` and `refreshContext()` for **every** `turn.completed`, including replayed ones. It has no `live` check (C:useBootstrap.ts:33-38).
- Replaying a 100-turn thread sends about 100 × `GET /v1/threads/summary?limit=300` (C:app.ts:137). Each call starts up to 5 `git` processes per workspace (S:runtime_api.rs:3294-3301), plus 100 context requests. On Windows this means a CPU spike and a sluggish UI.
- **Fix:** gate on `t.live` and debounce.

**B3. `item.interrupted` and `item.canceled` are not handled.**
- C:threads.ts:267-273 handles only started, completed and failed.
- **Scenario:** the user presses Stop while the model is thinking. The reasoning item stays `in_progress`, so ThinkingBlock shows a pulsing "Думаю…" forever (C:src/components/chat/Blocks.tsx:31-39). A cancelled compaction also stays in progress.
- The server sends exactly these events (S:runtime_threads.rs:18555-18609, 17451-17467).

**B4. `user_input.required` has no UI, so the turn hangs.**
- It is stored (C:threads.ts:334-339) but never rendered. The answer endpoint `POST /v1/user-input/{thread}/{id}` (S:runtime_api.rs:2641-2644) is never called.
- **Scenario:** in the "full" tool profile the model calls `request_user_input`. The turn waits indefinitely, the status-bar activity shows "thinking", and the composer is locked (C:src/components/composer/Composer.tsx:198). The only way out is Stop.

**B5. Stale approval cards and a misleading optimistic decision.**
- Pending approvals are held in memory on the server (S:runtime_threads.rs:6274, 8285-8295). After a runtime restart, crash or eviction, the replayed `approval.required` has no matching `decided`. The `Ok(Err)` path also emits nothing (S:runtime_threads.rs:18163-18168).
- The client keeps the card pending, and `threadActivity` reports "approval" forever (C:threads.ts:434).
- Clicking the card marks it allowed or denied **before** the POST (C:app.ts:310). The POST then returns 404 "no pending approval" (S:runtime_api.rs:5763-5767), and the card shows "Разрешено" even though nothing was allowed.
- **Fix:** on `turn.completed`, settle that turn's pending approvals and user inputs as cancelled. Reconcile against `ThreadDetail.pending_approvals`. On 404, mark the card "cancelled".

**B6. No gap detection.**
- `previous_seq` is not in the client types (C:src/lib/types.ts:120-130) and is never compared with `lastSeq`.
- **Scenario:** a reconnect after missing more than 4096 events (laptop sleep during a long agent run). Rust still sends `replay_limit=4096` with `since_seq` (C:api.rs:129-131). The server truncates the middle, deltas are lost, and items that never got `started` appear out of order.
- **Fix:** when `previous_seq != lastSeq`, re-snapshot. Omit `replay_limit` on resume.

**B7. A store failure can lock the thread forever.**
- `runtime.store_failure` with `terminal:true` means "this turn will never reach turn.completed" (S:runtime_threads.rs:2339-2343).
- The client ignores the event, so `activeTurnId` never clears and the composer refuses to send (C:Composer.tsx:198; C:threads.ts:232-233). Every replay re-locks it.

**B8. Reconnect policy gaps.**
- Retry after a stream ends is a fixed 800 ms, and backoff resets on every successful connect (C:api.rs:155-156, 189-190). A persistent `stream.end{replay_failed}` therefore loops at about 1 Hz forever, and `retryable:false` would never stop it.
- While the runtime is stopped, every subscribed thread logs "reconnecting" every 2 s with no end (C:api.rs:124-127).
- There is no idle or read timeout (C:src-tauri/src/runtime.rs:54-58, connect timeout only), so a hung server is never detected even though keepalives arrive every 15 s.
- **Fix:** add a watchdog of about 45 s without any bytes.

**B9. Subscriptions are never released.**
- Only archiving unsubscribes (C:app.ts:261). Each opened thread keeps an SSE connection and a broadcast receiver.
- Every event of every thread wakes every connection, and the server filters it per connection (S:runtime_api.rs:9193). Many open threads mean more fan-out and more `Lagged` → catch-up cycles.
- A thread whose stream stopped on 404 is never resubscribed, because `openThread` skips threads it already has (C:app.ts:170-174).

**B10. The reducer is O(n²) during replay.**
- Each event copies `t.items` (C:threads.ts:198, 294) and `t.turns` (C:threads.ts:129-136). A 4096-event replay over hundreds of items is millions of property copies, which shows up as jank when opening a thread.
- **Fix:** use one mutable draft per batch.

**B11. Errors and auto-approvals render twice.**
- An engine error produces both an `item.failed` with kind `error` and `turn.error` (S:runtime_threads.rs:18346-18373, 18690). Turn.tsx renders both: C:src/components/chat/Turn.tsx:36-37 and 129.
- Server auto-approvals (`decided.auto=true`) leave a collapsed "Разрешено" line before every tool call (C:threads.ts:312-319). A posture auto-deny (`decided` with no `required`) shows nothing (C:threads.ts:326).

**B12. Internal items flip after they are already shown.**
- A deferred-tool first call is marked `visibility:"internal"` only on completion (S:runtime_threads.rs:155-161 in the ToolCallComplete arm, around 17264-17271).
- The block was already pushed at `item.started`, and the UI never checks `internal`: C:Turn.tsx:12-45 has no reference, and `grep internal src/components` finds nothing. The noise row stays visible.

**B13. Minor issues.**
- `changedPaths` is overwritten per snapshot instead of unioned, and file counts are counted twice: from snapshots and from tool items (C:threads.ts:259-266, 162-179).
- tok/s uses `duration_ms`. The TUI uses `request_ms` as the throughput denominator (S:crates/tui/src/tui/session_metrics.rs:83-96).
- The `live` flag never returns to false on "replaying" (C:threads.ts:217-219), so the auto-approve check `t.live` (C:useBootstrap.ts:17) also passes during lag catch-up.
- Client-side auto-approve relies on a 60 ms timer racing the rAF flush (C:useBootstrap.ts:15-30; C:threads.ts:422-428). When the webview throttles rAF while still "visible", the approval is not yet in the store and the auto-approve is skipped.
- `find_frame_end` rescans the whole buffer on every chunk, which is quadratic for large frames. A tool `item.started` or `item.completed` can carry file content two or three times (C:api.rs:164-166, 194-206).

---

## 4. Reconnect and resume correctness

1. **Transport cursor is correct.** Rust stores `last_seq` from `id:` before emitting the event (C:api.rs:170-172) and resumes with `since_seq=last_seq` (C:api.rs:130-131). The server replays seq > since (S:runtime_threads.rs:4432-4434, 4466-4468), subscribes live before reading history (S:runtime_api.rs:9075), and dedupes by seq (9193). Transport frames carry no `id:` (9171-9173), so they never move the cursor. **Result: exactly-once delivery, unless `replay_limit` truncates (B6).**
2. **The parser is correct for axum output.** It buffers bytes and splits on `\n\n` or `\r\n\r\n` before UTF-8 decoding, so multibyte characters split across chunks are safe (C:api.rs:162-168). It joins multi-line `data:`, strips one leading space, skips comments, and tolerates CRLF (C:api.rs:209-233). Lone-CR line endings are not supported; axum does not emit them.
3. **Frontend dedupe is valid** because seqs are globally monotonic (S:runtime_threads.rs:4141-4146) and the stream preserves order: `ev.seq <= lastSeq` drops replays (C:threads.ts:383). There are two independent cursors, Rust `last_seq` and store `lastSeq`. They agree only while the store is never reset under a live subscription. Today `reset` happens only for threads that are not yet loaded.
4. **Initial load is incorrect** (B1). The protocol intends snapshot + `latest_seq` (S:runtime_threads.rs:5288-5303, 11926-11933). `Last-Event-ID` does not matter because the client uses `since_seq`, which takes precedence (S:runtime_api.rs:9070).
5. **Runtime restart.** The token and port change, and the loop re-reads `endpoint()` on every attempt (C:api.rs:123-128). Seqs survive restarts because state.json persists. `get_thread` flushes recovered `turn.completed{recovered:true}` before the replay is read (S:runtime_api.rs:9058-9062; S:runtime_threads.rs:9796-9815), so the turn's state recovers. Pending approvals and user inputs **do not recover** (B5).
6. **Termination signals.** `stream.end` is handled as a reconnect, but `retryable` and `reason` are ignored. EOF without `stream.end` is not distinguished, even though the server sends a header for exactly that (S:runtime_api.rs:9107-9112). There is no idle watchdog (B8).

### Target design for the client
`GET /v1/threads/{id}` → hydrate from `ThreadDetail` → `cw_subscribe(id, latest_seq)` with no `replay_limit` → on every journal frame check `previous_seq === lastSeq`, and re-snapshot if not → treat `turn.completed` as the only terminal turn event and settle that turn's leftover approvals and inputs → handle `item.interrupted`, `item.canceled`, `user_input.*` (with UI), `runtime.store_failure`, `sandbox.denied` and `agent.*` → gate side effects on `live` and debounce them → escalate backoff on repeated `stream.end`/EOF and honor `retryable` → add a 45 s keepalive watchdog → unsubscribe threads that are neither active nor running.