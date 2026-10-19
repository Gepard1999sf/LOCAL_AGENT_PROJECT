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

## Final knowledge-drive test (`final_test_water.py`)

End-to-end run of a real `codewhale-app-server` + the `ai-drive` MCP server (`codewhale-desktop mcp-drive`)
+ Godot: the agent writes `water.gdshader` into a copy of `Godot/WaterDemo`, checks it with `godot_check`,
and the script verifies the shader again with the tool itself. Run it from the repository root.

```powershell
# mock mode: the model text comes from mock_ollama.py (scenario "[shader]"), everything else is real
py -I -X utf8 codewhale-desktop\e2e\fixtures\live\final_test_water.py `
  --codewhale <portable>\codewhale-app-server.exe --desktop <portable>\codewhale-desktop.exe `
  --godot <godot .exe> --project Godot\WaterDemo --shader Godot\WaterDemo\water.gdshader `
  --check-project codewhale-desktop\src-tauri\resources\godot-check --out D:\AI\cw-bench\final-mock-check --timeout 240

# live mode: a real Ollama model (no --shader needed); add --render for water.png
py -I -X utf8 codewhale-desktop\e2e\fixtures\live\final_test_water.py ... --ollama-url http://127.0.0.1:11434 `
  --model qwen3-4b-32k --timeout 1500
# live mode with Qwen sampling (proxy) and a model the user's config does not list yet
py -I -X utf8 codewhale-desktop\e2e\fixtures\live\final_test_water.py ... --ollama-url http://127.0.0.1:11434 `
  --model qwen3.5-9b-text-32k --context-window 32768 `
  --sampling "temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens=4096" --timeout 1500
# both modes: --drive F:\AI_Drive | temp, --kiwix-url http://127.0.0.1:8091, --no-always-load,
#             --sampling SPEC [--no-think-to-effort], --context-window N
```

* `--codewhale` must be the `.exe` (a bare name is looked up on `PATH`; `.cmd`/`.bat`/`.ps1` shims are refused,
  because the server is spawned and killed by PID). `--shader` is required only in mock mode.
* `--ollama-url` accepts `http://host`, `host:port`, a `/v1` suffix or a path prefix behind a proxy; the real
  server is probed at `<prefix>/api/version`. If Ollama, the mock or the app-server does not come up, the run
  stops at once with `RESULT: FAIL` instead of waiting out `--timeout`.
* The temporary CodeWhale home gets the bundled skills, its own `mcp.json` for `ai-drive` (the user's `mcp.json` is
  never copied) and a `config.toml` (a copy is saved as `codewhale-config.toml` in `--out`):
  * **live mode** builds it from the user's config, read only: `--base-config PATH`, by default the file the desktop
    app edits (`CODEWHALE_CONFIG_PATH`, `$CODEWHALE_HOME\config.toml`, else `%USERPROFILE%\.codewhale\config.toml`;
    skipped when missing, `--base-config none` disables it). Without it CodeWhale gives every Ollama model an
    8192-token window: run 2 of `qwen3-4b-32k` compacted the context 60 times (every ~20 s at ~7.5 K tokens) and
    lost the task. The base is parsed with `tomllib` and written back (comments dropped), so no table or key can
    appear twice (TOML duplicates are fatal); `[providers.ollama] base_url` is set to `--ollama-url`.
  * **mock mode** writes only the `[stream]` and `[tools]` tables (unless `--base-config` is given).
  * Both get `[stream] open_timeout_secs = <--stream-open-timeout>` exactly once (default 300, the budget the desktop
    app sets; Ollama emits a tool call only when it is fully generated, so CodeWhale's 45 s default aborts long
    writes). The child env sets `CODEWHALE_CONFIG_PATH` to the temporary file, so an inherited value cannot point
    CodeWhale at the real config. The step `context window (informational)` shows what CodeWhale made of it
    (`GET /v1/threads/{id}/context`: `window_tokens` against the window the config gives `--model`).
  * `--always-load` (default on; `--no-always-load` turns it off) merges
    `[tools] always_load = ["mcp_ai-drive_godot_check", "mcp_ai-drive_ledger_record", "mcp_ai-drive_math_eval"]`
    into that file (added to the base config's list, no duplicates). CodeWhale 0.10.1 connects MCP servers lazily:
    until `ai-drive` is connected `tool_search` cannot find its tools, so without this table the model of run 2 never
    saw `godot_check`. The names take **one** hyphen (the registry name; the model is shown `mcp_ai--drive_*`):
    double-hyphen names, the wildcard `mcp_ai-drive_*` and the bare server name `ai-drive` load nothing. The step
    `always_load tools at step 0` fails when the first `model.tools.snapshot` (`payload.snapshot.tools[].name.value`,
    `step: 0`) lacks any of them; with `--no-always-load` it becomes `drive tools at step 0 (informational)`.
* The `ai-drive` server reads the knowledge drive given by `--drive` (`AI_DRIVE_PATH`): in live mode `F:\AI_Drive`
  when it exists, in mock mode (and with `--drive temp`) an empty temporary drive. The drive is only read:
  `AI_DRIVE_STATE_DIR` (ledger, log bookmarks) is always `<tmp>\agent_state`, so a run adds nothing to the user's
  ledger; `SYMPY_HELPER` is `<drive>\bin\sympy_helper.py` when present, else a copy of the bundled helper in the
  temporary folder. `--kiwix-url` (default `http://127.0.0.1:8091`; port 8090 is taken by Docker on the user's
  machine) is passed as `KIWIX_URL`. The step `knowledge drive (informational)` lists the drive's sections and
  whether kiwix-serve answers. `report.json` → `settings` records `drive`, `drive_kind` (`user`/`temp`),
  `agent_state_dir`, `sympy_helper`, `kiwix_url` and the merged `always_load` list; `summary.drive_tools_step0` lists
  the drive tools of the first model request.
* `--context-window N` sets `[providers.ollama.model_context_windows] "<--model>" = N` in the temporary `config.toml`
  (plus `"<model>:latest"` for an untagged name, replacing the base config's value for those keys), and
  `[providers.ollama] base_url` is then written too, so the file and `OLLAMA_BASE_URL` agree. Use it for a new
  model (`qwen3.5-9b-text-32k`): without an entry CodeWhale falls back to `[providers.ollama] context_window`, or
  8192 with no base config. N must equal the model's `num_ctx`. CodeWhale's `max_tokens` follows the window
  (window/4: 2048 at 8k, 4096 at 16k, 8192 at 32k). `report.json` → `settings.context_window`.
* `--sampling SPEC` puts [`ollama_proxy.py`](ollama_proxy.py) between CodeWhale and Ollama (see the next
  section): the proxy runs inside the driver on a port the OS picks, both `OLLAMA_BASE_URL` and
  `[providers.ollama] base_url` point at it, and every request is logged to `ollama_proxy.jsonl` in `--out`.
  `SPEC` is the proxy's rule list (`default` = `temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens=4096`,
  `none` = log only); `--no-think-to-effort` turns off the `think` → `reasoning_effort` fix. Steps: `sampling proxy`
  (it answers `/api/version` from the upstream) and `sampling proxy saw the model requests` (fails when no
  chat request went through it, i.e. the sampling did not apply). `report.json` → `settings.sampling` (spec,
  parsed rules, warnings, proxy URL, upstream) and `summary.sampling_proxy` (chat requests, how often each field
  was injected / lowered / overridden, the values CodeWhale sent itself under `kept`, finish reasons with
  `length_stops`, max prompt size, upstream errors, client hang-ups). Meant for live mode; in mock mode it
  tests the chain, and the mock's `ollama_requests.jsonl` shows what reached the model.
* Approvals are answered `allow`, except the tools in `--deny-tools` (comma-separated, `none` for none), which get
  `deny`. Default: `Web` in live mode (the desktop app does not auto-approve it either; run 2 spent 18 approvals on
  Web searches), nothing in mock mode.
* `godot_check called through MCP` counts only real calls: an `item.completed` whose `item.metadata.tool_name`
  (on `item.started` also `payload.tool.name`) is `mcp_ai-drive_godot_check` or `mcp_ai--drive_godot_check`.
  Free text does not count: in run 2 a Web search result that quoted "godot_check" passed the old check.
  `ledger_record called through MCP` is the same check for `ledger_record`, informational.
* The SSE reader survives a dropped connection: it reconnects from the last `seq` (up to 5 failed attempts in a row,
  replayed duplicates are skipped) and records `stream_error`; a close after the turn ended is normal
  (`closed_after_turn`). When `--timeout` runs out, the turn is interrupted (`POST …/interrupt`) and the step
  `turn finished` says `timed_out: true`.
* `report.json` → `summary`: `model_requests` (one `turn.usage` per finished request), `model_requests_started`
  (the runtime's `modelRequestDiagnostics`), `model_request_ms` per request plus total/max/avg, `first_token_ms_avg`,
  `input_tokens_max`, `approvals` (+ `approvals_denied`, `approvals_by_tool`), `compactions` (`item.completed` of kind
  `context_compaction`, + `first_compaction_at_s`), `tools_called` in call order, `mcp_calls`, `tool_calls_failed`
  and the stream's reconnects/errors. `details` keeps the per-call lists (tool calls with status and seconds,
  approvals with the decision, model requests, compactions).
* `--out` gets `report.json` (always, also after a crash of the script or Ctrl+C), `events.jsonl`, `codewhale.log`,
  `codewhale-config.toml`, `ollama_requests.jsonl` (mock mode), `ollama_proxy.jsonl` (`--sampling`) and `water.png`
  (`--render`: Godot directly on Windows, `xvfb-run` on Linux). Stale copies are deleted at start. `RESULT: PASS`
  means every step in `report.json` is `ok`.
* On exit the whole process tree (app-server, MCP server, Godot) is killed (`taskkill /T /F` on Windows), the
  sampling proxy is shut down and the temporary folder is removed; `--keep-tmp` keeps it and prints the path.

## Sampling proxy (`ollama_proxy.py`)

CodeWhale 0.10.1 sends `model`, `messages`, `tools`, `stream`, `stream_options`, `tool_choice`, `max_tokens` and,
with a `reasoning_effort` in its config, `think`. It never sends temperature, top_p or penalties. In live run 3
`qwen3-4b-32k` looped on one sentence until `max_tokens` (8192 tokens, 204 s). The proxy adds the sampling fields.
It uses the standard library only (`ThreadingHTTPServer`).

```powershell
py -I -X utf8 codewhale-desktop\e2e\fixtures\live\ollama_proxy.py --port 11435 --upstream http://127.0.0.1:11434 `
  --sampling "temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens<=4096" --log D:\AI\cw-bench\proxy.jsonl
# then OLLAMA_BASE_URL=http://127.0.0.1:11435/v1 and [providers.ollama] base_url = "http://127.0.0.1:11435/v1"
py -I -X utf8 codewhale-desktop\e2e\fixtures\live\ollama_proxy.py --self-test   # fake upstream, no Ollama, no GPU
```

* **Rules** (comma-separated; values are JSON, other words are strings, commas inside `[...]` are kept):
  `key=value` sets the field only when the request lacks it (or sends `null`). `key<=value` is a ceiling: it sets a
  missing field and lowers a larger number. `key:=value` always replaces. A key Ollama's `/v1` does not read gets a
  warning (`top_k`, `min_p`, `repeat_penalty`, `repeat_last_n`, `num_predict` and `num_ctx` belong in the Modelfile).
  CodeWhale **always** sends `max_tokens`, so `max_tokens=4096` never fires against it. Use `max_tokens<=4096` to
  cap it. A capped reply ends with `finish_reason: "length"`, and thinking tokens count towards the cap.
* **`think` → `reasoning_effort`** (on by default): Ollama's `/v1` drops `think`. So `reasoning_effort = "none"` in
  CodeWhale's config (it sends `"think": false`) does **not** switch thinking off on its own. The proxy turns
  `"think": false` (with no `reasoning_effort`/`reasoning`) into `"reasoning_effort": "none"`, which Ollama maps to
  `think=false`. `"think": true` is left alone, so the model's default applies, as it would without the proxy.
* **Pass-through:** every other path and every other byte is forwarded unchanged. A body is re-serialised only when
  a rule changed it. Hop-by-hop headers are dropped, and a chunked request body is de-chunked. Responses go out
  piece by piece as `read1` returns them (`TCP_NODELAY`, unbuffered writes; SSE is re-chunked, never collected). A
  watcher thread shuts the upstream socket as soon as the client hangs up, so an interrupted turn stops Ollama
  even while Ollama is still silent (prompt processing, or a tool call being built). If the upstream is down, the
  client gets a 502 with a JSON `error`.
* **Log** (`--log`, JSONL, one line per request): `method`, `path`, `status`, `first_byte_ms`, `duration_ms`,
  `response_bytes`. Chat requests also get `model`, `stream`, `messages`, `tools`, `prompt_chars` (the messages as
  JSON), `tools_chars`, `body_bytes_in/out`, `injected`, `lowered`, `overridden` and `kept`. They also get
  `finish_reason`, `tool_calls` (names) and `usage`, which are scanned from the SSE as it passes. A line gets
  `client_disconnected` only when a hang-up cut the exchange short. `summarize_log()` is what the driver puts into
  `summary.sampling_proxy`.
* **`--self-test`** (29 checks) runs against a fake upstream on loopback:
  * rule parsing;
  * byte-exact pass-through on one keep-alive connection;
  * injected, kept, lowered and overridden fields;
  * every SSE piece arriving within 100 ms of being sent, with 250 ms gaps upstream. A buffering relay fails this
    check with lags of 0.25–1.5 s;
  * chunked request bodies, close-delimited and HEAD responses;
  * hang-ups before the first byte and mid-stream reaching the upstream in under 1.5 s;
  * 502 when the upstream is down;
  * the log and its summary.

### What Ollama v0.40.4 does with a `/v1/chat/completions` body

Read from `https://raw.githubusercontent.com/ollama/ollama/v0.40.4/...` on 2026-10-20:

| Source (v0.40.4) | What it shows |
|---|---|
| `openai/openai.go` L111–132 `ChatCompletionRequest` | The only fields read are `model`, `messages`, `stream`, `stream_options`, `max_tokens`, `seed`, `stop`, `temperature`, `frequency_penalty`, `presence_penalty`, `top_p`, `response_format`, `tools`, `reasoning`, `reasoning_effort`, `logprobs`, `top_logprobs`, `_debug_render_only` and `keep_alive`. There is **no** `top_k`, `min_p`, `repeat_penalty`, `max_completion_tokens`, `options` or `think`. |
| `middleware/openai.go` L452, L466, L472–477 | `c.ShouldBindJSON(&req)`: gin ignores unknown fields (no `DisallowUnknownFields` in openai/, middleware/ or server/routes.go). `FromChatRequest` builds an `api.ChatRequest`, and its JSON replaces the body. Unknown fields such as `options` are therefore lost, not passed through. |
| `openai/openai.go` L732–773 | `stop` → `stop` (L734–745); `max_tokens` → `num_predict` (L747–749); `temperature` → `temperature`, **else 1.0** (L751–755); `seed` (L757–759); `frequency_penalty` (L761–763) and `presence_penalty` (L765–767), only when sent; `top_p` → `top_p`, **else 1.0** (L769–773). |
| `openai/openai.go` L788–796, L585–591 | `reasoning.effort`, else `reasoning_effort`, goes to `ThinkingFromReasoningEffort`. `""` → nil (the model's default, thinking **on** for qwen3.5), `"none"` → `think=false`. |
| `server/routes.go` L139–161 `modelOptionsWithEmbeddingBatchDefault` | Options are layered `api.DefaultOptions()` (L140), then `model.GenerationDefaults` (L151), then `model.Options`, the Modelfile `PARAMETER`s (L154), then the request options (L160), with the last layer winning. |
| `api/types.go` L1218–1232 `DefaultOptions` | num_predict −1, temperature 0.8, top_k 40, top_p 0.9, repeat_last_n 64, repeat_penalty 1.0, presence/frequency_penalty 0. |
| `llm/llama_server.go` L1662, L1677–1695 | `boundedNumPredict` (−1 → 10 × num_ctx, L124–135, multiplier L81). top_k, top_p, min_p, repeat_penalty, repeat_last_n, frequency_penalty and presence_penalty all reach llama-server. Its penalties look at the last `repeat_last_n` tokens only. |

Consequences for CodeWhale → Ollama `/v1`:

* The Modelfile's `PARAMETER temperature`/`top_p` **never apply**: `/v1` always sends both and uses 1.0 when they are
  missing. `qwen3-4b-32k` has 0.7/0.8 in its Modelfile and still ran at 1.0/1.0.
* A Modelfile `presence_penalty`, `frequency_penalty`, `top_k`, `min_p`, `repeat_penalty` or `repeat_last_n` **does
  apply** when the request omits the field, and CodeWhale always omits them. `qwen3.5:9b` already ships
  `presence_penalty 1.5`; `qwen3-4b-32k` has none (and `repeat_penalty 1`), hence the loop.
* The Modelfile's `num_predict` never applies to CodeWhale, because it always sends `max_tokens` (window/4).
* `think` from CodeWhale is ignored. Only `reasoning_effort` switches thinking.

### Text-only `qwen3.5-9b-text-32k`

The model was built on 2026-10-20 from `qwen3.5:9b` (`56671c2ab938`) without using the GPU:

* `ollama show --modelfile qwen3.5:9b` gives two `FROM` blobs: `sha256-02d45dc…` (5.63 GB, the text weights) and
  `sha256-f836f08…` (0.92 GB, the CLIP projector).
* The new Modelfile keeps the first `FROM`, `TEMPLATE {{ .Prompt }}`, `RENDERER qwen3.5`, `PARSER qwen3.5` and the
  license. It adds `PARAMETER num_ctx 32768`, `num_predict 4096`, `temperature 0.6`, `top_p 0.95`, `top_k 20` and
  `presence_penalty 1.5`.
* The file is `D:\AI\cw-bench\qwen-max\qwen3.5-9b-text-32k.Modelfile`, built with
  `ollama create qwen3.5-9b-text-32k -f <file>`.
* `ollama show qwen3.5-9b-text-32k` lists the capabilities `completion`, `tools` and `thinking`, with **no** `vision`
  and no Projector section. Its size is 5.6 GB, against 6.6 GB for the source.
* Over `/v1`, only `num_ctx`, `top_k` and `presence_penalty` from that list take effect. Temperature and top_p come
  from the proxy, or are 1.0 without it.
