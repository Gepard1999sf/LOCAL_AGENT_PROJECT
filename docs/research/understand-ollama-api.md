# Ollama integration audit: `ollama.rs` and the UI that uses it, compared with the Ollama REST API

**Path prefixes used below**
- Paths with no prefix are relative to `/home/user/LOCAL_AGENT_PROJECT/codewhale-desktop`.
- **OL** = `/tmp/claude-0/-home-user-LOCAL-AGENT-PROJECT/dfc6ae91-fcd0-5718-90b8-d857a427b4cb/scratchpad/ollama-src`. This is ollama/ollama `main` at commit 097586b (2026-10-13). Its `docs/api.md` is the same document as github.com/ollama/ollama/blob/main/docs/api.md.
- **CW** = `/tmp/claude-0/-home-user-LOCAL-AGENT-PROJECT/dfc6ae91-fcd0-5718-90b8-d857a427b4cb/scratchpad/cw-src`.

## 0. Summary

- **The `/api/create` body is not outdated.**
  - `{model, from, parameters:{num_ctx}, stream:false}` (src-tauri/src/ollama.rs:147) matches the current `CreateRequest` (OL api/types.go:711-756) and the docs (OL docs/api.md:1188-1212).
  - Inherited parameters such as stop and temperature are merged in, and only `num_ctx` is overridden (OL create/manifest.go:212-249).
  - The OpenAI-compatible endpoint cannot set `num_ctx` (OL docs/api/openai-compatibility.mdx:384-393). So building a model variant with a baked-in context is the right approach, and the comment at ollama.rs:133-134 is correct.
- **The real problems:**
  - The Ollama URL setting never reaches CodeWhale (B1).
  - Model names without a `:tag` are compared against tagged names and never match (B2).
  - Unload swallows errors (B3).
  - "4k by default" is hardcoded in many places and can be wrong (B4).
  - Cloud models look like local models (B5).
  - The URL is not normalized (B6).
  - The model list runs one `/api/show` per model every time the dropdown opens (B8).
- **Missing features:** download (pull) with progress, delete, copy/rename, model details with a VRAM estimate, preload and pin in memory, start Ollama on Windows, and reading `OLLAMA_HOST` / `OLLAMA_MODELS` and the server's real settings.

## 1. What exists today

| Tauri command | Ollama call | UI consumers |
|---|---|---|
| `ollama_status` (ollama.rs:34-66) | `GET /api/version` (3s), then `GET /api/ps` | store app.ts:115-121; polled every 5s in useBootstrap.ts:109-117; App.tsx:18-19, StatusBar.tsx:33-46, SettingsPanel.tsx:118-121, Inspector.tsx:157-159, SidePanels.tsx:204 |
| `ollama_models` (ollama.rs:68-131) | `GET /api/tags`, then `POST /api/show` for **every** model | app.ts:122-128; Toolbar.tsx:77 (on every dropdown open); SidePanels.tsx:176-179 |
| `ollama_create_variant` (ollama.rs:135-156) | `POST /api/create` with `stream:false` | SidePanels.tsx:181-194 |
| `ollama_unload` (ollama.rs:159-169) | `POST /api/generate {model, keep_alive:0}` | SidePanels.tsx:240-252 |

These are registered in lib.rs:38-41, wrapped in src/lib/tauri.ts:27-31, and typed in src/lib/types.ts:132-157.

## 2. Bugs and mismatches

**B1 (High): the Ollama URL setting is never passed to CodeWhale.**
- The sidecar gets only `CODEWHALE_RUNTIME_TOKEN`, `NO_COLOR` and `CODEWHALE_TELEMETRY` (runtime.rs:158-169).
- CodeWhale reads `OLLAMA_BASE_URL` (CW crates/config/src/lib.rs:7575-7577) and honours it as the active route (CW crates/tui/src/config/tests.rs:10727-10737). Its default is `http://localhost:11434/v1` (CW crates/cli/src/lib.rs:4939).
- Changing "Адрес" in Settings (SettingsPanel.tsx:84-90) therefore changes only the desktop's own probing. The agent keeps talking to the old host.
- **Fix:**
  - In runtime.rs, add `cmd.env("OLLAMA_BASE_URL", format!("{}/v1", settings.ollama_url()))`.
  - Restart the runtime when `ollamaUrl` changes.
  - Not verified: which wins when config.toml also sets `[providers.ollama].base_url`.

**B2 (Med): model names without a tag never match.**
- Ollama always reports names as `model:tag`: `DisplayShortest` always writes the tag (OL types/model/name.go:244-247). This is used by `/api/tags` (OL server/model_list.go:185-186) and `/api/ps` (OL server/routes.go:2783).
- `PREFERRED_MODELS = ["qwen3-4b-32k", …]` (useBootstrap.ts:7) is compared with exact equality (useBootstrap.ts:102). It never matches `qwen3-4b-32k:latest`.
- Variants are created without a tag (SidePanels.tsx:183), so they show up as `…:latest`.
- Inspector.tsx:157-158 compares CodeWhale's `thread.model` by exact string. A tagless default from CodeWhale's config shows as "не загружена" with a 4k context.
- **Fix:** add a `normalizeModelName()` helper that appends `:latest` when there is no `:` after the last `/`, and use it in every comparison.

**B3 (Med): unload reports success on failure.**
- ollama.rs:162-168 ignores the HTTP status. A `404 {"error":"model 'x' not found"}` still counts as success.
- SidePanels.tsx:244-248 has no try/catch. A network error becomes an unhandled rejection, and any HTTP error still shows "Модель выгружена".
- The unload button is hidden for embedding models (SidePanels.tsx:228). Unload works for any model, because the `keep_alive:0` path (OL server/routes.go:480-489) runs before the capability checks (OL server/routes.go:493).
- Expected success response: `{"model":"…","created_at":"…","response":"","done":true,"done_reason":"unload"}` (OL docs/api.md:459-484).

**B4 (Med): "4k by default" is hardcoded.**
- Locations: Toolbar.tsx:83, 108, 110; SidePanels.tsx:205, 218; Inspector.tsx:198; SettingsPanel.tsx:102; useBootstrap.ts:103.
- The real default comes from this chain, highest priority first:
  1. request `num_ctx`
  2. Modelfile `num_ctx`
  3. `OLLAMA_CONTEXT_LENGTH`
  4. a VRAM tier: 4096 / 32768 / 262144

  Sources: OL server/routes.go:200-211, 2541-2551; OL api/types.go:1237; OL docs/context-length.mdx:7-12.
- The Ollama app's context slider writes `OLLAMA_CONTEXT_LENGTH` into the server's environment (OL app/server/server.go:263-265).
- A user who set 32k in the Ollama app therefore gets false warnings and is pushed to create variants.
- **Fix:** effective context = `/api/ps` `context_length` if the model is loaded, else the Modelfile `numCtx`, else the server default discovered as in F9.

**B5 (Med): cloud models look local.**
- `/api/tags` rows carry `remote_model` / `remote_host` for cloud stubs (OL api/types.go:930-940; OL server/model_list.go:187-188).
- ollama.rs:112-125 drops these fields, while the app claims to be "полностью локально" (SidePanels.tsx:63).
- **Fix:**
  - Add a `remote: bool` to `OllamaModel`, then hide those models or badge them.
  - Optionally read `GET /api/status` → `{"cloud":{"disabled":bool,"source":"…"}}` (OL server/routes.go:2362, 2639-2647; OL api/types.go:959-967) and mention `OLLAMA_NO_CLOUD=1` (OL docs/faq.mdx:163-178).

**B6 (Med): the URL is not normalized.**
- settings.rs:107-112 only trims a trailing `/`.
- A CodeWhale-style `http://127.0.0.1:11434/v1` becomes `/v1/api/version`, which returns 404 and is shown as "offline" (ollama.rs:42-48).
- An input without a scheme fails to build a request.
- `0.0.0.0` is what the app's "Expose" option sets (OL app/server/server.go:250-251). It is not usable as a client address on Windows.
- The default ignores `OLLAMA_HOST` (settings.rs:11, 20).
- **Fix:** parse the way OL envconfig/config.go:20-60 does, strip `/v1` the way CW crates/tui/src/local_ollama.rs:168-175 does, and map `0.0.0.0` / `[::]` to `127.0.0.1`.

**B7 (Low): online detection does not identify Ollama.**
- Any 2xx on `/api/version` counts as online.
- Use `GET /` instead: the body is `Ollama is running` (OL server/routes.go:2358-2359). This tells "another program holds port 11434" apart from "Ollama is down".

**B8 (Med, performance): too many `/api/show` calls.**
- ollama.rs:81-128 sends N parallel `/api/show` requests with no concurrency limit. This runs on every model-dropdown open (Toolbar.tsx:77) and every time the panel opens (SidePanels.tsx:176-179).
- Current Ollama `/api/tags` already returns `capabilities` and `details.context_length` (OL api/types.go:939, 1049; OL server/model_list.go:198, 210-214). Only a baked-in `num_ctx` needs `/api/show` `parameters`.
- **Fix:** cache by `digest` (present in tags, OL api/types.go:937), call `/api/show` only for new digests, and cap concurrency at about 4 (CW caps at 8: CW crates/tui/src/local_ollama.rs:22).

**B9 (Low): fragile native-context extraction.**
- `find(k.ends_with(".context_length")).and_then(as_u64)` (ollama.rs:101-105) takes the first matching key and gives up if its value is not a u64.
- **Fix:** read `model_info[general.architecture + ".context_length"]`, falling back to `details.context_length`.

**B10 (Low): raw error bodies and unchecked status codes.**
- `ollama_create_variant` returns the raw body text (ollama.rs:152-154), so the toast shows `{"error":"…"}`.
- Ollama errors are always `{"error": "..."}` (OL docs/api/errors.mdx:18-22). Parse the `error` field.
- ollama.rs:71-79 and 55-58 parse bodies without checking the status code.

**B11 (Low): creating a variant silently overwrites.**
- `/api/create` resolves an existing name and rewrites it with no conflict error (OL server/create.go:126-130).
- SidePanels.tsx:181-193 does not warn when the name already exists.
- The variant-name regex is tailored to one model (SidePanels.tsx:182).

**B12 (UX): offline handling.**
- The offline state replaces the whole UI (App.tsx:18-19). It only says "start it from the Start menu" and shows `ollama serve`, with no button (StatusScreens.tsx:15-20).
- The panel tells the user to run `ollama pull` in the CLI (SidePanels.tsx:200).
- Both are covered by F1 and F8.

## 3. Missing features, with exact API shapes

### F1. Download (pull) with progress
- **Request:** `POST /api/pull {"model":"qwen3:8b","stream":true}`. Fields: `model`, `stream`; `insecure` is deprecated and ignored (OL api/types.go:855-865).
- **Response:** NDJSON, `Content-Type: application/x-ndjson` (OL server/routes.go:2590-2591). Each line is a `ProgressResponse {status, digest?, total?, completed?}` (OL api/types.go:869-874):
  ```
  {"status":"pulling manifest"}
  {"status":"pulling <digest>","digest":"<digest>","total":2142590208,"completed":241970}   // one series per layer
  {"status":"verifying sha256 digest"}
  {"status":"writing manifest"}
  {"status":"success"}
  ```
  Source: OL docs/api.md:1529-1600.
- **Errors:**
  - Before streaming starts: `400 {"error":…}` (OL server/routes.go:1476-1507).
  - During the stream: an `{"error":"…"}` line with HTTP **200** (OL server/routes.go:1523-1525, 2609-2614; OL docs/api/errors.mdx:28-35). The stream reader must check every line for `error`.
- **Cancel:** drop the connection. The server-side context is cancelled (OL server/routes.go:1520-1521), and the pull resumes later (OL docs/api.md:1535).
- **Implementation:**
  - `reqwest::Response::bytes_stream()` (the `stream` feature is already on in src-tauri/Cargo.toml).
  - Split on `\n`, aggregate `total` / `completed` per digest, emit a throttled `ollama-pull {model,status,completed,total}` event.
  - Do not use `.timeout()`, because it is a total-time limit. Wrap each chunk read in `tokio::time::timeout` instead.
  - Keep an abort handle per model for `ollama_pull_cancel`.

### F2. Delete
- **Request:** `DELETE /api/delete {"model":"x"}` (OL api/types.go:781-786).
- **Responses:**
  - `200` with an empty body.
  - `404 {"error":"model 'x' not found"}`.
  - `400` for an invalid name.

  Source: OL server/routes.go:1695-1735; OL docs/api.md:1503-1527.
- Blob files are deleted only if no other manifest references them (OL manifest/manifest.go:259-275). Variants share the base model's blobs, so deleting the base frees no disk space while a variant exists. Say so in the UI.
- Unload the model first if it appears in `/api/ps`. If it was `settings.defaultModel`, pick another.

### F3. Copy and rename
- **Request:** `POST /api/copy {"source":"a","destination":"b"}` (OL api/types.go:849-852).
- **Responses:**
  - `200 {"status":"success"}` (or a warning string in `status`).
  - `404 {"error":"model \"a\" not found"}`.
  - `400` for an invalid name.

  Source: OL server/routes.go:2130-2177.
- Rename = copy, then delete.

### F4. Model details
- **Request:** `POST /api/show {"model":"x"}`. `verbose` is optional (OL api/types.go:789-800).
- **Response** (OL api/types.go:806-826; OL docs/api.md:1409-1472):
  - `parameters` (string), `template`, `system`, `license`, `modified_at`, `remote_host`, `remote_model`
  - `details{family, parameter_size, quantization_level, context_length}`
  - `capabilities` (for example `completion`, `tools`, `thinking`, `vision`, `embedding`)
  - `model_info{general.architecture, general.parameter_count, <arch>.context_length, <arch>.block_count, <arch>.attention.head_count, <arch>.attention.head_count_kv, <arch>.embedding_length, …}`
- **Use:** show a VRAM estimate before the user picks a context size:

  KV bytes ≈ 2 × block_count × head_count_kv × (embedding_length / head_count) × num_ctx × bytes per element

  where bytes per element is 2 for f16, about 1 for q8_0 and about 0.5 for q4_0 (fields at OL docs/api.md:1453-1456). Compare the result with the GPU's VRAM from `gpu_stats` (src-tauri/src/system.rs:14-19).

### F5. Running models and VRAM
- **Request:** `GET /api/ps`.
- **Response:** `{"models":[{name, model, size, size_vram, digest, details, expires_at, context_length, runner}]}` (OL api/types.go:943-953; OL server/routes.go:2778-2824).
- Show:
  - the GPU percentage as `size_vram / size`, the same as the PROCESSOR column of `ollama ps` (OL docs/context-length.mdx:33-41)
  - the actual context
  - a countdown to `expires_at`
- The desktop already polls this every 5s (useBootstrap.ts:109-117). `expires_at` is fetched but never shown (types.ts:155).

### F6. Preload, keep in memory, unload
- **Load:** `POST /api/generate {"model":"x"}` → `{"model","created_at","response":"","done":true}` (OL docs/api.md:434-457; OL docs/faq.mdx:267-281).
- **Keep in memory:** add `"keep_alive":-1` or `"30m"` (OL docs/faq.mdx:297-307).
  - This setting survives later CodeWhale requests that send no `keep_alive`. A reused runner keeps its current duration when the request's duration is nil (OL server/sched.go:532-534); the default applies only on a fresh load (OL server/sched.go:561-564). The OpenAI-compatible layer passes nil when the field is absent (OL openai/openai.go:131, 812).
  - Do **not** send `options.num_ctx` when preloading. Different runner options force a reload (OL server/sched.go:1428-1447).
- **Unload:** `keep_alive:0`; the response has `done_reason:"unload"`.
- **Use:** warm the model in `setModel` (app.ts:234-247), so the first agent turn does not pay the load time.

### F7. Version and heartbeat
- `GET /api/version` → `{"version":"0.x.y"}`; `HEAD /` works as a heartbeat (OL server/routes.go:2358-2361; OL api/client.go:439-444).
- Use the version to decide feature support, such as tags-level `capabilities` and `/api/ps` `context_length`.

### F8. Detect Ollama and start it on Windows
- **Is it installed?** Look for `%LOCALAPPDATA%\Programs\Ollama\ollama app.exe`, the installer's default location (OL app/ollama.iss:16, 34; OL docs/windows.mdx:70). Then try `%LOCALAPPDATA%\Ollama\ollama app.exe`, then `PATH` (OL cmd/start_windows.go:28-47).
- **Is it updating?** If `OllamaSetup.exe` is running, show "updating" (OL cmd/start_windows.go:25-27).
- **Start it the way the Ollama CLI does:** `cmd.exe /c "<dir>\ollama app.exe" --hide --fast-startup` with `CREATE_NO_WINDOW` (0x08000000) (OL cmd/start_windows.go:49-51).
  - Reuse `winproc::CREATE_NO_WINDOW` (runtime.rs:171).
  - Do **not** put it in the kill-on-close Job object (winproc.rs:32; runtime.rs:189-194), so Ollama keeps running after the desktop app exits.
  - Then poll `HEAD /` every 500ms. The CLI waits only 5s (OL cmd/start.go:13-27); allow 20-30s for a cold start.
- **Prefer `ollama app.exe` over `ollama serve`.** The app passes its own settings into the server's environment: Expose → `OLLAMA_HOST`, Models → `OLLAMA_MODELS`, Context → `OLLAMA_CONTEXT_LENGTH`, plus cloud (OL app/server/server.go:240-269). A bare `serve` can use a different models folder and a different context.
- **Fallbacks:**
  - `ollama.exe serve` with `CREATE_NO_WINDOW`, output written to a log file. This covers standalone zip installs (OL docs/windows.mdx:85-103).
  - Not installed: show a download link.
  - Remote URL: do not offer to start anything.
- **UI:** replace the full-screen block (App.tsx:18-19, StatusScreens.tsx:7-35) with a banner and a "Запустить Ollama" button.

### F9. `OLLAMA_HOST`, `OLLAMA_MODELS` and the server's real settings
- **Default URL:** derive it from `OLLAMA_HOST` (OL envconfig/config.go:20-60; default `127.0.0.1:11434`).
- **Models folder:** `OLLAMA_MODELS`, otherwise `%USERPROFILE%\.ollama\models` (OL envconfig/config.go:111-124; OL docs/faq.mdx:233-238). The app's "Model location" setting lives in the app's own settings, not in the environment (OL app/server/server.go:256-258).
- **Where the truth is:** `%LOCALAPPDATA%\Ollama\server.log` (OL docs/windows.mdx:66-68). It contains:
  - a `server config env=map[…]` line listing `OLLAMA_MODELS`, `OLLAMA_CONTEXT_LENGTH`, `OLLAMA_KV_CACHE_TYPE`, `OLLAMA_FLASH_ATTENTION`, `OLLAMA_HOST` (OL server/routes.go:2439; OL envconfig/config.go:318-338)
  - a `vram-based default context … default_num_ctx=N` line (OL server/routes.go:2551)

  The Ollama app parses this same file with the regexes `vram-based default context` and `default_num_ctx=(\d+)` (OL app/server/server.go:303-307, 390-397).
- **Use:** show the real default context (fixes B4), the models folder, and free disk space before a pull.
- On Windows, a changed environment variable takes effect only after the tray app is quit and started again (OL docs/windows.mdx:51; OL docs/faq.mdx:113-127). Offer "apply and restart Ollama".

### F10. Settings for small GPUs
These are global and need a server restart.
- `OLLAMA_FLASH_ATTENTION=1` plus `OLLAMA_KV_CACHE_TYPE=q8_0`: about half the KV-cache memory (OL docs/faq.mdx:344-366; OL envconfig/config.go:216, 222).
- `OLLAMA_CONTEXT_LENGTH`: a global alternative to per-model variants (OL docs/context-length.mdx:29-33).
- `OLLAMA_KEEP_ALIVE` (OL docs/faq.mdx:316).
- `OLLAMA_NUM_PARALLEL=1`: memory grows with parallel × context (OL docs/faq.mdx:335).

### F11. Align CodeWhale's context window with the variant
- CodeWhale config supports `[providers.ollama] model_context_windows = {"qwen3-4b-32k:latest" = 32768}` (CW crates/config/src/lib.rs:189-208; CW crates/tui/src/config.rs:3164-3176).
- When the thread is not live, `/v1/threads/{id}/context` takes the window from static `provider_capability` (CW crates/tui/src/runtime_api/context.rs:73-78; CW crates/tui/src/route_budget.rs:42-52).
- Not verified: whether this override reaches the Inspector's mismatch warning (Inspector.tsx:159-186).

### F12. Model catalog for "download"
- There is no public library search API.
- `GET /api/experimental/model-recommendations` → `{"recommendations":[{model, description, context_length, vram_bytes, …}]}` exists (OL server/routes.go:2390, 2419-2435; OL api/types.go:894-917). It is experimental and the default entries are mostly `:cloud` models (OL server/model_recommendations.go:373-410).
- A curated list sized for 8 GB of VRAM is the better choice.

## 4. Proposed command surface

| New or changed command | Ollama call | Returns / emits |
|---|---|---|
| `ollama_pull(model)`, `ollama_pull_cancel(model)` | `POST /api/pull` (stream) | `ollama-pull` events, then `Ok` or an error |
| `ollama_delete(model)` | `DELETE /api/delete` | `Ok` / `{error}` |
| `ollama_copy(src,dst)` | `POST /api/copy` | `Ok` |
| `ollama_show(model)` | `POST /api/show` | full details, cached by digest |
| `ollama_load(model, keepAlive)` | `POST /api/generate {model, keep_alive}` | `Ok` |
| `ollama_unload` (fix) | same as today, but check status | `Err` on non-2xx |
| `ollama_status` (extend) | `GET /`, `/api/version`, `/api/ps`, `/api/status` | `{online, isOllama, version, loaded[], cloudDisabled}` |
| `ollama_detect()`, `ollama_start()` | filesystem / process checks, spawn `ollama app.exe` | `{installed, appPath, cliPath, updating}` |
| `ollama_server_config()` | parse `%LOCALAPPDATA%\Ollama\server.log` | `{models, contextLength, defaultNumCtx, kvCacheType, flashAttention, host}` |
| `ollama_models` (fix) | tags plus show only for new digests, concurrency ≤ 4 | adds `remote`, `digest`, `details.contextLength` |

I edited no project files. While gathering sources, one command accidentally switched the existing `scratchpad/ollama-src` clone to a sparse checkout. I undid it with `git sparse-checkout disable` and `git status` there is clean.