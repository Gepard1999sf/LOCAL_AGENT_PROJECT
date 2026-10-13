# CodeWhale runtime API features the desktop client does not use yet, ranked by value

Path abbreviations used below:
- `RA` = `cw-src/crates/tui/src/runtime_api.rs`, and `RA/x.rs` = `cw-src/crates/tui/src/runtime_api/x.rs`
- `RT` = `cw-src/crates/tui/src/runtime_threads.rs`
- `D/` = `/home/user/LOCAL_AGENT_PROJECT/codewhale-desktop/`
- `cw-src` = `/tmp/claude-0/-home-user-LOCAL-AGENT-PROJECT/dfc6ae91-fcd0-5718-90b8-d857a427b4cb/scratchpad/cw-src`

## 0. What the desktop already uses (baseline)

These calls are excluded from the catalog below:
- `GET /health` and `GET /v1/runtime/info` (only the version field is read): D/src-tauri/src/runtime.rs:216, 233-240.
- `GET /v1/threads/summary`: D/src/store/app.ts:137 and D/src/components/layout/Sidebar.tsx:227.
- `GET /v1/sessions`: app.ts:138.
- `GET /v1/threads/{id}/context`: app.ts:153.
- `POST /v1/threads` with `model_provider:"ollama"`, `workspace` and `allowed_tools`: app.ts:181-187.
- `POST .../turns`: app.ts:213. `POST .../interrupt`: app.ts:228.
- `PATCH /v1/threads/{id}` with model, title or archived: app.ts:239, 251, 260.
- `POST .../fork`: app.ts:272. `POST /v1/sessions/{id}/resume-thread`: app.ts:284.
- `POST /v1/approvals/{id}`: app.ts:312. `POST .../compact`: D/src/components/composer/Composer.tsx:110.
- `GET /v1/workspace/files/search`: Composer.tsx:173. `GET /v1/usage?group_by=day`: D/src/components/overlays/SidePanels.tsx:108.
- `GET /v1/threads/{id}` (only for export): D/src/components/overlays/SettingsPanel.tsx:64.
- The SSE stream `/v1/threads/{id}/events?replay_limit=4096&progress=true&since_seq=0`: D/src-tauri/src/api.rs:129-134.

The full router is `build_router`, RA:2364-2856.

---

## 1. How CodeWhale configures Ollama, and whether config and models are reachable over HTTP

### Config file location and format on Windows
- **Directory:** `<home>\.codewhale\`. CodeWhale picks the home directory in this order:
  1. `HOME`
  2. `USERPROFILE`
  3. `HOMEDRIVE`+`HOMEPATH`
  4. the platform resolver

  Code: cw-src/crates/paths/src/lib.rs:94-101, 119-127. `CODEWHALE_HOME` overrides the whole thing (paths/lib.rs:60-62, 120-122).
- **Desktop mismatch:** the desktop checks `USERPROFILE` before `HOME` (D/src-tauri/src/runtime.rs:299-304). If `HOME` is set, for example by Git for Windows, the desktop and CodeWhale can disagree about where `.codewhale` lives.
- **Files:**
  - `config.toml` holds the provider, model and approval policy. It can be overridden with `--config` or `CODEWHALE_CONFIG_PATH` (cw-src/docs/CONFIGURATION.md:257-264).
  - `settings.toml` holds UI and behaviour settings such as `auto_compact` and `locale` (cw-src/crates/tui/src/settings.rs:20, 2445-2470).
- **Format:** TOML. The Ollama table is `[providers.ollama]` (cw-src/crates/config/src/lib.rs:354). Its accepted keys are in `ProviderConfigToml`, config/lib.rs:178-253:
  - `base_url`, `model`, `api_key`
  - `context_window`, with aliases `contextWindow`, `context_length`, `contextLength`, `context_window_tokens`
  - `model_context_windows` (a per-model table)
  - `http_headers`, `allow_insecure_http`, `path_suffix`, `wire`, `auth_mode`
- **Built-in defaults:** `base_url = "http://localhost:11434/v1"`, `default_model = "unknown"`, wire `chat_completions`, key optional (cw-src/crates/config/assets/provider_descriptors.json:720-745).
  - The desktop's own default URL is `http://127.0.0.1:11434` (D/src-tauri/src/settings.rs:11). It is never passed to CodeWhale: the spawn only sets the token, `NO_COLOR` and telemetry env vars (D/src-tauri/src/runtime.rs:158-169).
- **Environment variables:**
  - `OLLAMA_BASE_URL` is applied only when the configured provider is `ollama` (cw-src/crates/tui/src/config.rs:8774-8780).
  - `OLLAMA_MODEL` is applied when the active provider is Ollama (config.rs:8810-8818).
  - `OLLAMA_API_KEY` is optional (descriptor JSON:726-728).
- **Per-run overrides:** `codewhale app-server --http` accepts global flags `--provider`, `--model`, `--approval-policy`, `--sandbox-mode`, `--base-url` and `--set KEY=VALUE` (cw-src/crates/cli/src/lib.rs:91-165). `--set` is refused only for the legacy non-`--http` transport (cli/lib.rs:2242-2248).

### `num_ctx` and `keep_alive`: CodeWhale has no knobs for these
- CodeWhale talks to Ollama only through OpenAI-compatible `/v1/chat/completions`.
- The only Ollama-specific field it sends is `think`:
  - `think:false` for reasoning effort off/none (cw-src/crates/tui/src/client.rs:5699, 5782-5785)
  - `think:true` for low/medium/high (client.rs:5815, 5904-5907) and for max (client.rs:5946, 6012-6015)
- Request code never sends `options`, `num_ctx` or `num_predict`. A grep of `client.rs` and `client/` finds none. The only `num_ctx` mention is an advice string (cw-src/crates/tui/src/core/engine/context.rs:853).
- Therefore:
  - `num_ctx` has to be baked into the model. The desktop already does this with `ollama_create_variant` (D/src-tauri/src/ollama.rs:139-163).
  - `keep_alive` has to be handled on the Ollama side, or by the desktop calling `/api/generate` directly, as it already does for unloading (ollama.rs:166-176).

### Context window: the most important local-model gotcha
- CodeWhale picks the effective window by walking this ladder, top to bottom (cw-src/docs/CONFIGURATION.md:1053-1079):
  1. `[providers.ollama.model_context_windows]` entry for the exact model
  2. `[providers.ollama] context_window`
  3. catalog
  4. model-name `_Nk` hint
  5. fallback, which is **8192 for Ollama** (cw-src/crates/tui/src/config.rs:376-393)
- The desktop's variant naming `${base}-${ctx/1024}k` (D/src/components/overlays/SidePanels.tsx:183) happens to trigger rung 4:
  - `qwen3:8b-32k` is read as 32,000 tokens (×1000, not ×1024), labelled "model-name hint (unverified)" (cw-src/crates/models/src/lib.rs:195-205, 388-428; cw-src/crates/tui/src/route_runtime.rs:183-191).
  - A plain tag such as `qwen2.5-coder:7b` gets **8192**. That drives the compaction trigger and the context meter.
- **No HTTP endpoint writes `context_window` or `model_context_windows`.**
  - `POST /v1/config` only persists a fixed set of keys (RA:11932-11953 and RA:11616-11804). Any other key goes to the `settings.toml` validator (RA:11805-11816), which rejects unknown keys.
  - The settings schema lists `context_window` as a read-only diagnostic row (cw-src/crates/config/src/settings_schema.rs:1336-1340).
- **Workaround:** edit `config.toml` from Rust with a TOML editor, or run `codewhale config set 'providers.ollama.model_context_windows."qwen3:8b"' 32768` (CONFIGURATION.md:1043-1048). Then call `POST /v1/config/reload` (RA:12305-12324), which reloads disk config into live engines without a restart.

### Config and model endpoints that exist
- `GET /v1/config`: a snapshot of model, `model_available`, provider, `base_url`, `approval_mode`, `reasoning_effort`, `auto_compact`, `sandbox_mode`, `memory_enabled`, `subagents_*`, `locale` and more (RA:11359-11394, 11434-11524).
- `POST /v1/config` with `{key, value, persist}`.
  - `model` is validated against the active route (RA:11591-11597).
  - `base_url` writes to the **active** route only (RA:11629-11636). So the Ollama URL can only be set this way after Ollama is the active provider.
  - Every persisted write needs `POST /v1/config/reload` to take effect (RA:11599-11605).
- `POST /v1/providers/ollama/switch` with `{model?}`: persists `provider = "ollama"` plus `[providers.ollama].model`, reloads, rolls back if the reload is rejected, and returns `{provider, model, model_available, message, persisted}` (RA:11094-11126, 11163-11420).
- `GET /v1/providers/ollama/models` and `POST .../models/refresh`. See section 3.

---

## 2. High value

### H1. Make Ollama the global provider and manage config
**Routes:** `GET /v1/config`, `POST /v1/providers/{id}/switch`, `POST /v1/config`, `POST /v1/config/reload`, `GET /v1/settings/schema`

- **What they do:** see section 1. The schema endpoint returns `{version, tabs[], settings[]}`. Each row carries `key`, `kind` (bool/int/enum/string), `label` and `description` localized via `settings.locale` (Russian is supported, cw-src/crates/localization/src/lib.rs:28), plus `default`, `value`, `persisted`, `editable`, `visible` and `options[]` (RA:11851-11929, 11965+).
- **Why high:**
  - The desktop pins `model_provider:"ollama"` on every thread (app.ts:181, 239), but the runtime's global provider stays whatever `config.toml` says, which on a fresh install is DeepSeek. Anything not thread-scoped resolves against the global route. That includes `/v1/stream` (RA:9386) and `GET /v1/config` reporting `model_available:false` (RA:11440-11442).
  - One switch call at first run makes CodeWhale Ollama-first on disk, matching what TUI users get.
  - The schema gives a ready-made settings UI, for example `auto_compact` (default **false**, settings_schema.rs:873-882) and `auto_compact_threshold_percent` (settings_schema.rs:884-893). Both are writable through `POST /v1/config` (RA:11666-11685, 11805).

### H2. Per-thread and per-turn `reasoning_effort` (thinking on/off)
**Not a route:** a field on `POST /v1/threads`, `PATCH`, and `POST .../turns`

- **Shape:** `reasoning_effort: "off" | "low" | "medium" | "high" | "max" | "auto"` (RT:4911-4913 for threads, RT:4994-4996 for turns).
- **Effect:** for Ollama this becomes `think:false` or `think:true` (client.rs:5782-5785, 5904-5907, 6012-6015).
- **Why high:** for qwen3 and deepseek-r1 style models, turning thinking off is the single biggest latency lever. The desktop never sends this field. A toolbar toggle is cheap to add.

### H3. Thread snapshot for opening threads
**Route:** `GET /v1/threads/{id}`, already allowed but only used for export

- **Returns:** `ThreadDetail {thread, turns[], items[], latest_seq, pending_approvals[], pending_user_inputs[], pending_dynamic_tool_calls[], approval_grants[]}` (RT:5284-5308).
- **Why high (reliability):**
  - The desktop rebuilds history from SSE replay with `since_seq=0`. Replay is a *tail* limited to `MAX_RUNTIME_EVENT_REPLAY_TAIL = 4096` events (RT:75, RT:15427-15433).
  - Every streamed text chunk is a durable `item.delta` event (RT:16851-16857).
  - So one long answer can produce thousands of events, and opening an older thread can silently lose its beginning.
  - The fix: render from the snapshot, then subscribe with `since_seq=latest_seq`. This also restores pending approvals and questions after reconnecting.

### H4. Answering the agent's questions
**Route:** `POST /v1/user-input/{thread_id}/{input_id}`

- **Request body:** `{answers:[{id,label,value}]}` (RA:1200-1209).
- **Response:** `{ok, input_id, delivered}`, or 404 if nothing is pending (RA:5797-5828).
- **The event payload:** `request.questions[]` with `header, id, question, options[{label,description}], allow_free_text, multi_select` (cw-src/crates/tui/src/tools/user_input.rs:90-114).
- **Why high:** the desktop records `user_input.required` events in its store (D/src/store/threads.ts:334-348), but no component renders them and nothing calls this route. With the "full" tool profile, `request_user_input` is exposed (user_input.rs:244-246). A turn that asks a question will hang. LITE_TOOLS excludes it (D/src/lib/tools.ts:68).

### H5. Edit, retry and undo with clean context
- **Routes:**
  - `POST /v1/threads/{id}/retry` with `{depth?, prompt?, dynamic_tools?}`. It forks the thread without the last N turns and starts a new turn. Returns `{thread, turn}` (RA:8380-8460).
  - `POST /v1/threads/{id}/undo` with `{depth?}`. Returns `{thread, original_user_text, original_user_images}` (RA:7449-7486).
  - `POST /v1/threads/{id}/fork-at-turn` with `{turn_id}`. Keeps that turn and everything before it, and returns the next prompt for editing (RA:7488-7530).
- **Why high:**
  - The desktop's retry button simply sends the prompt again (D/src/components/chat/Turn.tsx:104, 129). The failed or bad turn stays in history and eats the small local context window.
  - These routes give real "regenerate" and "edit message" behaviour.

### H6. Undoing the agent's file changes
- **Routes:**
  - `POST /v1/threads/{id}/patch-undo` with `{depth?}`. Restores files from the pre-turn snapshot and forks the thread. Returns `{patch_result:{files_restored, summary, snapshot_label}, thread, original_user_text}` (RA:7520-7550, 7551+).
  - `POST /v1/threads/{id}/file-revert` with `{path, snapshot_id, expected_hash}`. Reverts one file. Returns `{path, action, snapshot_id, snapshot_label}` (RA:8173-8195, 8211+).
  - `GET /v1/snapshots?limit`, returning `[{id, label, timestamp}]` (RA:10149-10169).
  - `POST /v1/snapshots/{id}/restore`, restoring the whole workspace (RA:10171-10196).
- **Conditions:** file-revert needs trust mode or Full Access (RA:8230-8234). Each returns 409 if a turn is active.
- **Why high:** small local models break files fairly often. One-click "undo this turn's edits" is a core safety feature, and the desktop already gets `turn.workspace_snapshot` events (threads.ts:259-266).

### H7. Steering a running turn
**Route:** `POST /v1/threads/{id}/turns/{turn_id}/steer`

- **Request body:** `{prompt}` (RT:5273-5275). Returns the `TurnRecord` (RA:8609-8620).
- **Behaviour:** queued into the active engine. Refused if the turn is stopping (RT:15068-15105).
- **Why high:** local turns are slow, and the composer is disabled while a turn runs (Composer.tsx:316, 448). Steering lets the user correct course without stopping the turn.

### H8. Mode and permission posture on the server, plus approval history
- **Fields** on `POST /v1/threads`, `PATCH /v1/threads/{id}` and turns:
  - `mode`: `plan`, `agent`/`act`, or `operate` (cw-src/crates/config/src/app_mode.rs:22-32)
  - `permission_posture`: `ask`, `auto_review` or `full_access` (cw-src/crates/runtime/src/runtime_policy.rs:26-32, 62-87)
  - also `trust_mode`, `allow_shell`, `auto_approve` (RT:4942-4966)
- **Routes:**
  - `GET /v1/approvals?limit` returns `[{approval_id, tool_name, target, summary, outcome, standing_rule, decided_by, asked_at, decided_at}]` (RA:5623-5645, 5712-5740).
  - `DELETE /v1/threads/{id}/approval-grants/{grant_id}` revokes a "remember" grant (RA:5776-5795). Live grants appear in `ThreadDetail.approval_grants` (RT:5304-5307).
- **Why high:**
  - Plan mode (read-only planning) suits weak models.
  - The desktop's client-side auto-approve rules (D/src-tauri/src/settings.rs:33-43) only work while a window is subscribed. A server posture is authoritative.
  - Grant revocation is needed because the desktop already sends `remember` (app.ts:312).

### H9. Git review panel
- **Routes:**
  - `GET /v1/git`: `{branch, head, head_oid, index_token, revision, ahead, behind, staged, unstaged, untracked, files[{path, status, staged, rev}], branches, remotes}` (RA/git.rs:211-253, 255).
  - `GET /v1/changes` (git.rs:1040), `GET /v1/diff?path` (git.rs:1091-1100), `GET /v1/workspace/diff?limit` (git.rs:1160-1168).
  - `POST /v1/git/stage|unstage|discard` with `{paths[], all, expect?}` (git.rs:1344-1353, 1788-1820).
  - `POST /v1/git/commit` with `{message, all, expect?}` (git.rs:1357-1364, 1828).
  - `POST /v1/git/push` and `POST /v1/git/branch` (git.rs:1327-1340, 1919, 1931). `GET /v1/git/graph` (git.rs:1243).
- **Why high:** "review what the model did, then commit" is the natural end of every agent session. Today the desktop only shows the branch name, via its own `git_branch` command (D/src-tauri/src/lib.rs:45).

### H10. Per-tool-call diffs and turn artifacts
- **Routes:**
  - `GET /v1/threads/{id}/turns/{turn_id}/calls/{tool_call_id}/changes?limit` returns `{state: captured|pending|unavailable, reason, files[]}` (RA/turn_artifacts.rs:325-335, 362).
  - `GET .../turns/{turn_id}/artifacts` and `.../artifacts/{artifact_id}?offset&limit&revision` (turn_artifacts.rs:28, 50-77).
- **Why high:** accurate per-call diffs, including writes made by shell commands, instead of guessing diffs from tool payloads (D/src/components/chat/Blocks.tsx:97, 185).

---

## 3. Medium value

- **Background shell jobs.** `GET /v1/jobs` and `GET|POST /v1/threads/{id}/jobs`.
  - Create body: `{command, cwd?, timeout_ms?, tty, env}` (RA/jobs.rs:160-172, 256).
  - Output: `GET .../jobs/{job_id}/output?stream&cursor&max_bytes&wait_ms≤30000&format`, returning `{data, next_cursor, status, exit_code, done}` (jobs.rs:380-420, 422).
  - Also `stdin`, `kill` and `resize` (jobs.rs:520, 574, 596).
  - Works on Windows (jobs.rs:183-194). Useful for seeing and killing a dev server the model launched (owner `agent`, jobs.rs:42-50).
- **Workspace file browser and editor.**
  - `GET /v1/workspace/files?path&limit` returns `{entries[{name, path, kind, size, modified}], truncated}` (RA/workspace.rs:260-284, 480).
  - `GET /v1/workspace/files/read?path&offset&limit` returns `{size, revision, encoding, content, truncated}` (workspace.rs:287-306, 655).
  - `PUT /v1/workspace/files` with `{path, content, encoding?, expected_revision?}` does optimistic-concurrency writes (workspace.rs:310-327, 688).
  - `GET /v1/workspace/status` (workspace.rs:88-121).
- **Instruction files.** `GET /v1/workspace/instructions` returns `{sources[], generated_fallback, aggregate_budget_bytes, warnings}` (workspace.rs:814-829, 863). This shows which AGENTS.md and rule files consume the small context window.
- **Skills.**
  - `GET /v1/skills` returns `{directory, directories, warnings, skills[{name, description, enabled, invocation, source, ...}]}` (RA:1014-1040, 5034).
  - `POST /v1/skills/{name}` with `{enabled}` (RA:1079-1087, 5073).
  - `GET /v1/skills/{name}` returns the entry plus `body` (RA:1047-1052, 5120).
  - Install, update, trust, audit and uninstall: RA:2683-2686, 1092-1115.
  - Medium value: disabling unneeded skills shrinks prompts for local models.
- **MCP servers.**
  - `GET /v1/apps/mcp/servers` returns `{revision, servers[{name, origin, writable, enabled, connected, command, url, enabled_tools, disabled_tools}]}` (RA:1296-1314, 6194).
  - Create, patch and delete: body `McpServerWriteRequest {command, args, env, url, transport, timeouts...}` (RA:1354+, 6374, 6438, 6497).
  - Enable, disable and reconnect (RA:6524, 6554, 6585). `GET /v1/apps/mcp/tools?server&connect` (RA:1317-1336, 6233). Import from other clients: `/v1/apps/mcp/imports` (RA/mcp_import.rs).
  - **Mutations require an `If-Match: mcp-v1-<sha256>` header** (RA:5997-6024). `cw_request` cannot send custom headers (D/src-tauri/src/api.rs:62-70), so the proxy needs extending first.
- **Memory.**
  - `GET /v1/memory?scope&q&limit` returns `{entries[{id, scope, summary, stale, status}], total}` (RA:12342-12376, 12482).
  - `POST /v1/memory` with `{text, scope}` (RA:12379-12386, 12571). `DELETE /v1/memory?scope` (RA:12619). `GET /v1/memory/{id}` (RA:12553).
  - Lens review and actions: `GET /v1/memory/lens`, `POST /v1/memory/lens/actions` (approve, reject, forget, remember, replace), `GET /v1/memory/events` (RA/memory_lens.rs:47-87, 154-159). The lens returns 503 if memory is disabled (memory_lens.rs:109-111).
- **Plan and checklist panel.** `GET /v1/threads/{id}/plan` and `/todo`, plus cross-thread `/v1/plan`, `/v1/todo`, `/v1/plans`, `/v1/todos` (RA/plans.rs:40-110, 266, 288). The desktop allows `todo_write` (tools.ts:68), so a live checklist is a natural fit.
- **Slash-command palette.** `GET /v1/commands` returns `{commands[...]}` with builtins and user commands (RA:4771, 4813-4815, 4892-4910).
- **Idempotent send.** `GET /v1/threads/{id}/turn-operations/{operation_key}` returns the `TurnRecord` or 404 (RA:8496-8516). Resending with the **same** `operation_key` returns 200 with `idempotent_replay:true` instead of a duplicate turn (RA:8462-8494). The desktop generates a fresh key on every attempt (app.ts:208), so a retry after an ambiguous error can double-send.
- **Per-thread system prompt override.** `PATCH /v1/threads/{id}` with `{system_prompt}` (RT:4942-4966). It **replaces** the standard prompt (`system_prompt_override: true`, RT:11622-11638), which can mean much smaller prompts for 7–8B models. Caveat: you must preserve the compaction-summary markers (RT:243-251).
- **Output cap per turn.** `maxOutputTokens` on turns (RT:4979-4985). Only accepted when the route supports output limits (RA:10600-10615).
- **Usage and cost.**
  - `GET /v1/usage?since&until&group_by=model|provider|thread` (RA:10096-10147). The desktop only uses `day`.
  - `GET /v1/threads/{id}/usage` returns `{thread_id, totals}` (RA:7347-7366).
  - Receipts: `GET .../receipt` and `.../turns/{turn_id}/receipt` (RA:7371-7404).
  - Tokens per model per day is useful for local throughput stats even though cost is zero.
- **Running threads and notices.** `GET /v1/threads/running` (RA:3146-3156). `GET /v1/threads/{id}/notices` and `DELETE .../notices/{notice_id}` for subagent-terminal, elevation-needed and model-notify notices (RA:3160-3190). Useful for a tray icon and "turn still running" warnings on quit.
- **Archive view and session management.**
  - `GET /v1/threads?archived_only=true` (RA:954-960, 3131), plus `PATCH archived:false` to restore.
  - `PATCH /v1/sessions/{id}` with `{title?, archived?}` and `DELETE /v1/sessions/{id}` (RA/sessions.rs:199, 955).
  - There is **no** `DELETE /v1/threads/{id}`; the route is GET and PATCH only (RA:2485).
- **Diagnostics.** `GET /v1/logs`, `/v1/logs/{name}?offset&limit&tail`, `/v1/crashes`, `/v1/crashes/{name}`, `/v1/process` (RA/diagnostics.rs:9-14, 256, 297, 320, 339, 379). Good for a "collect logs" button, beyond the desktop's stdout tail.
- **Hooks.** `GET /v1/hooks?thread_id` returns `{enabled, hooks[{event, command (redacted), source}], problems}` (RA:4921-4940, 4981).
- **Runtime capabilities.** The `capabilities` object in `/v1/runtime/info` (RA:1219-1236, 1238-1270, 5830) can drive feature gating. The desktop reads only the version.
- **Client-side tools.** `dynamic_tools` on turns, results via `POST .../tool-calls/{call_id}/result` with `{success, content[]}` (cw-src/crates/protocol/src/runtime/mod.rs:195-263; RA:8634-8656). Example: an "open in editor" tool the desktop executes itself.
- **Provider model catalog** (low to medium).
  - `GET /v1/providers` returns `{current, current_provider_id, providers[{id, display_name, default_model, has_model_catalog, credential_state, ...}]}` (RA:10249-10336, 10947).
  - `GET /v1/providers/ollama/models?filter&cursor&limit` returns `{models[{id, image_input, output_token_limit, reasoning_effort, reasoning_effort_levels}], total, nextCursor}` (RA:10340-10362, 11020).
  - For Ollama this lists only the configured or cached live catalog, with `unknown` filtered out (RA:10528-10585). `POST .../models/refresh` returns a `CatalogUpdateReceipt` (RA:11064-11085; cw-src/crates/tui/src/provider_lake.rs:1608-1730).
  - The desktop's direct `/api/tags` and `/api/show` reads are richer. The value here is checking what CodeWhale believes, for example image input support.
- **LSP.** `GET /v1/lsp`, `/v1/diagnostics?path`, `/v1/definition`, `/v1/references`, `/v1/symbols` (RA/lsp.rs:1-19). Needs language servers installed.

---

## 4. Low value for this use case

- **Terminal PTY.** `/v1/terminal/{name}/output|input|resize|kill`. **Unix only. On Windows these return 501** (RA/terminal.rs:28-29, 344-352).
- **Multi-agent features.** Fleet (RA:2412-2454), agent-runs (RA:2409-2411, 3435), operate (RA:2753-2763), agent-mail (RA:2612-2626). A single local GPU runs workers one at a time, so these add complexity with little benefit.
- **Tasks and automations.**
  - `POST /v1/tasks`, body `NewTaskRequest {prompt, name, model, model_provider, workspace, mode, permission_posture...}` (cw-src/crates/tui/src/task_manager.rs:453-473; RA:3087, 9004-9050).
  - Automations: `{name, prompt, rrule, cwds, model...}` (cw-src/crates/tui/src/automation_manager.rs:283-307; RA:6729-7090), with an optional `Idempotency-Key` header (RA:6761).
  - Possibly medium if you want "overnight runs".
- **Goals.** `PUT /v1/threads/{id}/goal` with `{objective, token_budget?}`, plus `complete` and `block` (RA:8776-8960).
- **Voice.** `/v1/voice*`: records on the host, with ASR chosen in order `CODEWHALE_ASR_MODEL`, local whisper, Groq, then the provider (RA/voice.rs:1-18; cw-src/crates/tui/src/voice.rs:591-611).
- **Remote and cloud targets.** `/v1/targets`, `/remote`, `/ssh`, `/cloud`: these mostly return `supported:false` or 501 (RA/targets.rs:1-20).
- **Plugins, marketplaces, avatars.** RA:2688-2738.
- **Cloud credentials.** `/v1/account/model-access` and `/v1/providers/{id}/key` (RA:2768-2790). Ollama is keyless.
- **Constitution.** `/v1/constitution` and `/v1/constitution/preview` (RA/constitution.rs:11-48).
- **History migration.** `/v1/thread-history/*` (RA:2487-2505; RA/thread_history.rs:1-2).
- **Notification text.** `/v1/threads/{id}/notifications/prepare` with `{seq, focused, unfocused_for_ms, locale}` returns `{status, headline, body, sound}` (RA/notification_delivery.rs:13-30). Could give native toasts their wording.
- **One-shot stream.** `POST /v1/stream` creates a thread and streams in one call, using the global provider (RA:919-932, 9376+).
- **Built-in web UI.** Served at `/` (RA:2821).

---

## 5. Gaps in the desktop proxy that block some of the above

1. **No custom request headers.** `cw_request` sends only bearer and JSON (D/src-tauri/src/api.rs:62-70). This blocks MCP mutations, which need `If-Match` (RA:5997-6024), and optional `Idempotency-Key` for automations (RA:6761).
2. **Only thread SSE is proxied** (api.rs:121-186). Fleet event streams (RA:2423) are not reachable.
3. **The 120 s timeout** (api.rs:67) is fine for every route listed above. Job output long-polls cap at 30 s (jobs.rs:35).
4. **The desktop's Ollama URL setting never reaches CodeWhale** (runtime.rs:158-169). Fixes:
   - Persist it into `[providers.ollama] base_url`, or
   - switch to the ollama provider and set `OLLAMA_BASE_URL` (config.rs:8774-8780).

   Without this, the desktop's Ollama panel and the agent can point at different servers.