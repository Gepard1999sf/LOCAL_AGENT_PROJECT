# Live CodeWhale runtime API fixtures

Real request/response pairs and SSE streams recorded from a running
`codewhale app-server --http`. Use them for contract tests of the desktop
client (`src/store/*.ts`, `src-tauri/src/api.rs`) and for UI replay.

## Provenance

| | |
|---|---|
| CodeWhale source | commit `182bafe6bdeac5c9432ccbc05716ef084104dc53` (2026-10-13, "Merge pull request #6949"), crate version **0.10.2** |
| Binary | `cargo build -p codewhale-cli --bin codewhale` (debug, rustc 1.97.0, 22m35s on 4 cores) |
| Command | `codewhale app-server --http --host 127.0.0.1 --port 18765` (same argv as `src-tauri/src/runtime.rs`) |
| Env | `CODEWHALE_RUNTIME_TOKEN=testtoken NO_COLOR=1 CODEWHALE_TELEMETRY=0 HOME=<tmp> CODEWHALE_HOME=<short tmp path>` |
| Platform | Linux x86_64 (no Windows host available) |
| Recorded | 2026-10-13 |
| `/health` ready | 0.3–0.5 s after spawn (debug build) |

`runtime/info` reports `codewhale_commit: "unknown"` because the local build had no release metadata.

**Model backend.** Real Ollama could not be installed: `ollama.com`, `github.com`
release downloads and `huggingface.co` are blocked (HTTP 403) by the sandbox
egress proxy. So the fixtures come from two phases:

* `offline_*`: no model backend at all. Nothing listens on `:11434`.
* `mock_*`: [`mock_ollama.py`](mock_ollama.py) is a deterministic stand-in that serves
  Ollama's OpenAI-compatible `/v1/chat/completions` (streaming, `reasoning`
  deltas like Ollama ≥ 0.12, tool calls, usage chunk). CodeWhale itself is
  real. Everything CodeWhale emits (items, deltas, approvals, snapshots,
  compaction) is genuine, but the *model text* is canned. The scenario
  depends on a marker in the prompt: `[tool:read]`, `[tool:write]`,
  `[tool:bash]`, `[slow]`.

**Normalisation.** The recorder replaces the server's working directory with
`/work/demo-project` and its HOME with `/home/demo`. IDs, timestamps and seq
numbers are real. Note that **seq is global across threads**: a new thread's
first event can have seq 46 or 178. It is not per-thread and does not start at 1.

## File format

```jsonc
// plain exchange
{ "endpoint": "POST /v1/threads", "phase": "offline|mock",
  "request": { "method", "path", "auth": true|false|"<token>", "body" },
  "status": 201, "response": { "content_type", "body" }, "elapsed_ms", "recorded_at", "note"? }

// SSE capture (GET /v1/threads/{id}/events?replay_limit=4096&progress=true&since_seq=N)
{ "endpoint", "phase", "request", "status", "response_headers",
  "events": [ { "event": "item.delta", "id": "57" | null, "data": { …parsed JSON… } } ],
  "event_names": [ … ] }
```

* `data` is the parsed JSON. On the wire it is one `data:` line. Frame order on the
  wire is `event:`, then `data:`, then `id:` (id **last**). `stream.progress` frames carry
  **no** `id:`. See the byte-exact capture [`mock_92_turn_tool_read__raw.sse`](mock_92_turn_tool_read__raw.sse).
* The recorder shortens `model.tools.snapshot` payloads to `{"_shortened_by_recorder": true, …}`.
  On the wire each one is about 13–14 KB with the 5 lite tools, and more with the full tool set. One arrives
  before every model step and one at turn end, so a turn with 2 model steps has 3. The raw `.sse` file keeps them intact.
* `ollama_upstream_requests.json` holds what CodeWhale sent to Ollama, trimmed. The fields are
  `model`, `stream`, `stream_options.include_usage`, `max_tokens: 2048`, `tool_choice: "auto"` and `tools`.
  There is **no** `num_ctx`/`options`. The 5 lite tools appear as `bash, edit, read, todo_write, write`.

## Endpoints used by the desktop client

| Endpoint (client call site) | Fixture(s) | Status | Notes |
|---|---|---|---|
| `GET /health` (runtime.rs readiness) | offline_01 | 200 | No auth. `{"status":"ok","service":"codewhale-runtime-api","mode":"local"}` |
| `GET /v1/runtime/info` (runtime.rs, **no token**) | offline_02 / 03, mock_01 | 200 | **No auth required.** Has `codewhale_version`, `auth_required:true`, `capabilities{}` |
| any `/v1/*` without or with wrong token | offline_04 / 05, offline_82 | 401 | `{"error":{"message":"runtime API bearer token required","status":401}}` |
| `POST /v1/threads` `{model_provider,model}` | offline_10, mock_02 | 201 | Bare ThreadRecord (`schema_version:2`, `permission_posture:"ask"`, `allow_shell:true`) |
| `POST /v1/threads` client body (+`workspace`,`allowed_tools`) | offline_11, mock_90 | 201 | `allowed_tools` echoed back |
| ⚠️ `POST /v1/threads` **without `model`** (app.ts `newThread` when `defaultModel` is null) | offline_12, mock_03 | **400** | **CONTRACT BREAK.** `"Local Ollama has no fresh model catalog for this endpoint; select an explicit model or refresh its catalog."` This happens even when Ollama is up: app-server never probes `/api/tags`. |
| `POST /v1/threads` unknown tool in `allowed_tools` | offline_13 | 201 | Not validated. Unknown names are silently accepted. |
| `GET /v1/threads/summary?limit=300` / `&search=` | offline_20 / 21, mock_22 | 200 | Bare array of ThreadSummary. Title defaults to `"New Thread"`, then becomes the first prompt. |
| `GET /v1/sessions?limit=50` | offline_22, mock_28 | 200 | `{"sessions":[…]}`. Empty until a thread is forked (fork saves a session). |
| `GET /v1/threads/{id}` | offline_23 / 43, mock_15b / 20 | 200 | `{thread,turns,items,latest_seq,pending_approvals,pending_user_inputs,pending_dynamic_tool_calls,approval_grants}` |
| `GET /v1/threads/{unknown}` | offline_25 | 404 | `Thread not found: …` |
| `GET /v1/threads/{id}/context` | offline_24, mock_21 | 200 | **`window_tokens: 8192` for every Ollama model** (see "Context window"). Takes 144 ms on the first call. |
| `PATCH /v1/threads/{id}` `{title}` | offline_30 | 200 | Returns ThreadRecord |
| `PATCH /v1/threads/{id}` `{model, model_provider:"ollama"}` | offline_31 | 200 | |
| `PATCH /v1/threads/{id}` `{archived:true}` | offline_34 | 200 | |
| `POST /v1/threads/{id}/fork` `{}` | offline_32, mock_24 | 201 | **Bare ThreadRecord** with a top-level `id` and `session_id` (`res.id` path in app.ts is right) |
| `POST /v1/threads/{id}/compact` `{}` | offline_33, mock_25 (+ mock_26 SSE) | **202** | `{thread, turn}`. It runs as its own turn `"Manual context compaction"` with a `context_compaction` item. With history, it calls the model non-streaming with no tools. |
| `POST /v1/threads/{id}/turns` | offline_40, mock_10 … 15, mock_91 | 201 | `{thread, turn}`, with `turn.status:"in_progress"`. Always accepted, even with no backend. |
| `POST …/turns/{turn_id}/interrupt` while running | mock_14b | 200 | Returns TurnRecord (still `in_progress`). The SSE then shows `turn.interrupt_requested`, `item.interrupted` and `turn.completed{status:"interrupted"}`. |
| ⚠️ `POST …/turns/{turn_id}/interrupt` after the turn ended / unknown turn | offline_41 / 42 | **409** | `No active turn on thread …`. The user can press Stop just as the turn finishes. **Treat 409 as success, with no error toast.** |
| `POST /v1/approvals/{approval_id}` `{decision:"allow"\|"deny",remember}` | mock_15c, mock_13c | 200 | `{"ok":true,"approval_id","decision","delivered":true}` |
| `POST /v1/approvals/{unknown}` | offline_51 | 404 | `no pending approval with id '…'` |
| `POST /v1/approvals/{id}` `{decision:"approve"}` | offline_52 | 400 | Only `allow` / `deny` are accepted |
| `GET /v1/approvals` | offline_50, mock_13b | 200 | ⚠️ The list's `approval_id` is the **tool_call_id**, not the `approval_…` id that the decide endpoint needs. Decide with the id from `approval.required` or `GET /v1/threads/{id}`.`pending_approvals[].id`. |
| `GET /v1/workspace/files/search?query=…&limit=60` | offline_60 / 61, mock_27 | 200 | `{"paths":[…]}`. Rooted at the server **cwd**. Includes `.codewhale/state/*` files. |
| `GET /v1/usage?group_by=day` | offline_62, mock_23 | 200 | `totals{…}` plus buckets. Ollama turns show as `nonmetered_turns` and `cost_usd: 0` with `unpriced_reasons:["unknown_provider_route"]`. |
| `POST /v1/sessions/{id}/resume-thread` | offline_70 | 200 | `{thread_id, session_id, message_count, summary}` (`res.thread_id` path in app.ts is right) |
| `POST /v1/sessions/{unknown}/resume-thread` | offline_71 | 404 | |
| `GET /v1/threads/{id}/events?replay_limit=4096&progress=true&since_seq=0` | offline_40 / 80, mock_*__sse, mock_30 | 200 | `content-type: text/event-stream` and `x-codewhale-stream-end: 1`. Keepalive is the comment `: keepalive` every 15 s. |
| events for unknown thread / no token / `replay_limit=5000` | offline_81 / 82 / 83 | 404 / 401 / 400 | `replay_limit cannot exceed 4096`. The client's value of 4096 is the maximum. |
| events at server shutdown (SIGINT) | mock_95 | 200 | Ends with `stream.end {"reason":"runtime_shutdown","retryable":true,"last_seq":…}` |

No endpoint used by the client returned **404 or 405 for its route**, so every
route exists. The only 4xx contract break is the **400 for `POST /v1/threads`
without `model`**. The 409 from interrupt and the approval-id mismatch in `GET /v1/approvals`
are pitfalls.

## SSE semantics observed (important for `src/store/threads.ts`)

1. **There is no `turn.failed`, `turn.interrupted` or `turn.canceled` event.** The runtime ends every
   turn with **`turn.completed`**. Check `payload.turn.status` ∈ `completed | failed | interrupted`;
   on failure, `payload.turn.error` is a string. A grep of `runtime_threads*.rs` / `runtime_api*.rs`
   confirms this. Today the client sets `lastError` only on `turn.failed`, so **errors are never shown**.
2. On interrupt, the partial `agent_message` ends with **`item.interrupted`** (`item.status:"interrupted"`).
   `item.canceled` also exists. The client handles neither, so the bubble stays `in_progress`.
3. A turn with no model backend (offline_40) is accepted (201). The SSE then streams **16 `status` items**
   (`"Retry attempt: transport 1/3; network error; waiting 0.99s"`, …, then `"Retry exhaustion: stream-open stopped after 3 retries…"`).
   After that come an `item.failed` with `kind:"error"` (`"Network error: Cannot reach localhost:11434 … Connection refused"`)
   and `turn.completed{status:"failed"}`. This takes **about 28 s**. The desktop should check Ollama before `POST …/turns`.
4. Item kinds seen: `user_message`, `agent_reasoning`, `agent_message`, `tool_call` (read, bash),
   `file_change` (write, with `metadata.mutation.{diff,files[]}`), `status` (retries; `"Continuing — tool results"`
   with `metadata.visibility:"internal"`), `context_compaction` and `error`.
   Tool items carry `metadata.tool_name` and `metadata.tool_input` (a JSON **string**), plus `tool_use_id`.
5. `item.delta` payload is `{"delta":"…","kind":"agent_message"|"agent_reasoning"}`. It is preceded by `item.started`
   for the same `item_id` and followed by `item.completed` with the full text in `summary`/`detail`.
6. `approval.required` payload: `{id, approval_id, tool_call_id, tool_name, summary, description, intent_summary}`.
   `approval.decided` payload: `{approval_id, tool_call_id, decision:"allow"|"deny", remember, grant_id?}`.
   In posture `ask`, **`write` inside the workspace needs no approval, but `bash` does**. A denied call ends as
   `item.failed` (`is_error:true`). An approved call's output is prefixed with `[approval] … approved by the user…\n\n`.
7. Other events the client ignores today: `turn.interrupt_requested`, `turn.artifacts`, `turn.steered`,
   `turn.steer_dropped`, `operation.activity_started/completed`, `operation.heartbeat`,
   `approval.grant_added/revoked`, `thread.permissions_changed` and `item.canceled`.
8. `stream.progress` (`state: "replaying"` → `"live"`) carries `seq` equal to the cursor and no `id:`.

## Context window

`codewhale app-server` never calls Ollama's native `/api/tags` or `/api/show`. The mock
logged only `/v1/chat/completions`. As a result:

* every Ollama model gets `window_tokens: 8192`, `output_cap_tokens: 2048` and
  `compaction_trigger_tokens: 5120`. With the system prompt (~8.9 K chars) and 5 lite tools, input is already
  about 3–4.2 K tokens. Automatic compaction therefore starts after a few turns.
* Setting `[providers.ollama] context_window = 32768` in the config file **works**. With
  `codewhale --config <file> app-server --http …`, `/context` shows `window_tokens: 32768`,
  `output_cap_tokens: 8192` and `compaction_trigger_tokens: 23552`. `--set providers.ollama.context_window=…`
  is **rejected**. `--set` only takes provider, model, default_text_model, verbosity,
  approval_policy, sandbox_mode and telemetry.
* CodeWhale sends no `num_ctx`. Ollama's own context, `OLLAMA_CONTEXT_LENGTH` on the Ollama server,
  must be **at least** the `context_window` you give CodeWhale. Otherwise Ollama silently truncates the prompt.

`OLLAMA_BASE_URL=http://127.0.0.1:11435/v1` (the desktop's custom-URL path) works: see mock_90/91.

## Runtime process notes

* **Owner receipt:** `<CODEWHALE_HOME>/run/daemon.owner.json` on Windows and `run/daemon.sock.owner.json` on Unix.
  It can survive SIGTERM/SIGINT; it did survive while an SSE client was connected. A stale receipt with the
  **same** config path is tolerated. A stale receipt with a **different** `--config` makes startup fail with
  `selected owner config does not match; refusing another store`.
* **Unix only:** the daemon socket path must be ≤ 107 bytes. With a long HOME the server exits with
  `daemon socket path … is 127 bytes; this platform allows at most 107`.
* On exit the server writes terminal-reset escape sequences to stdout
  (`ESC[<1u ESC[?1007l … ESC[?1049l`), even with `NO_COLOR=1` and piped stdout. The log panel should strip CSI.

## Re-recording

```bash
# 1. build
cd <codewhale-src> && cargo build -p codewhale-cli --bin codewhale
# 2. run (workspace = any dir with hello.txt; short CODEWHALE_HOME on Unix)
cd /path/to/workspace && CODEWHALE_RUNTIME_TOKEN=testtoken NO_COLOR=1 CODEWHALE_TELEMETRY=0 \
  HOME=$TMPHOME CODEWHALE_HOME=/tmp/cwh codewhale app-server --http --host 127.0.0.1 --port 18765 &
# 3. offline phase (nothing on :11434)
python3 -I record_live.py --phase offline --out . --server-home $TMPHOME
# 4. mock phase
python3 -I mock_ollama.py --port 11434 --log /tmp/mock-requests.jsonl &
python3 -I record_live.py --phase mock  --out . --server-home $TMPHOME
python3 -I record_live.py --phase extra --out . --server-home $TMPHOME     # bash approval -> allow
# 5. custom URL: restart the server with OLLAMA_BASE_URL=http://127.0.0.1:11435/v1, mock on :11435
python3 -I record_live.py --phase custom_url --out . --server-home $TMPHOME
```

Run `record_live.py` from the server's working directory, or pass `--workspace`.
