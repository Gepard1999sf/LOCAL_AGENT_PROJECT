# CodeWhale Desktop: what it's missing compared with the best local-LLM and coding-agent desktop apps

**Before you read the line numbers.** Other agents are editing the working tree right now. They have uncommitted changes in `src/store/app.ts`, `src/store/threads.ts`, `src/hooks/useBootstrap.ts`, `src/components/chat/Turn.tsx`, `src/lib/types.ts` and `src/lib/export.ts`. Line numbers for those files refer to the last commit (HEAD 4668336) and are marked **@HEAD**. All other files are unchanged since HEAD, so their numbers match the working tree. Upstream line numbers refer to the `cw-src` clone (`crates/...`).

**Already being built: the answer card for the agent's questions.** When the agent asks the user a question (a `user_input.required` event), HEAD saves the request in the store (`threads.ts@HEAD:334-351`) but never shows it, so the agent waits forever. The uncommitted changes add `input:` blocks, `answerUserInput` and an import of `UserInputCard` in `Turn.tsx`. The file `src/components/chat/UserInputCard.tsx` does not exist yet. Check that it lands; until it does, the agent is stuck.

## Already in the app (no need to rebuild)
- **Model picker:** shows context-size badges and a warning for models that can't call tools (`src/components/layout/Toolbar.tsx:66-124`).
- **Models panel:** creates a copy of a model with a larger context baked in, and unloads models from VRAM (`src/components/overlays/SidePanels.tsx:163-280`, `src-tauri/src/ollama.rs:133-169`).
- **Context bar** with a warning when Ollama's loaded context is smaller than CodeWhale expects (`src/components/layout/Inspector.tsx:137-222`, warning at `:160`, `:179-186`). There is also a context segment in the status bar (`StatusBar.tsx:66-71`).
- **Tokens per second after a turn:** `StatusBar.tsx:72-74`, `Turn.tsx@HEAD:136`, calculated in `threads.ts@HEAD:254`.
- **VRAM in the status bar** via nvidia-smi (`StatusBar.tsx:85-92`, `src-tauri/src/system.rs:14-39`).
- **Approval card** with Allow / Deny / "Всегда в этой сессии" (`src/components/chat/Blocks.tsx:206-266`). Auto-approve rules run in the client (`useBootstrap.ts@HEAD:13-31`, `SettingsPanel.tsx:226-236`).
- **Project picker** with recent projects (`Toolbar.tsx:126-171`). **@-file search** (`Composer.tsx:166-189`). Image paste and attach (`Composer.tsx:276-314`, `:384-390`).
- **Diffs** for each tool call, view-only (`DiffView.tsx:50-131`, `Blocks.tsx:97`).
- **Telemetry off by default** (`src-tauri/src/settings.rs:22`, `runtime.rs:167-169`).

## Gaps, ranked

| # | Feature | Effort | Where it goes |
|---|---|---|---|
| 1 | Review a turn's changes, undo and revert | L (M for a first version) | `Turn.tsx`, new `chat/TurnChanges.tsx`, new tab in `Inspector.tsx`, `store/app.ts` |
| 2 | Plan/Act mode and permission level, plus better approvals | S/M | `Composer.tsx`, `store/app.ts`, `SettingsPanel.tsx`, `useShortcuts.ts` |
| 3 | Crash recovery and resilience | S/M | `runtime.rs`, `api.rs`, `App.tsx`, `StatusScreens.tsx`, `Composer.tsx` |
| 4 | Model picker that shows whether a model fits in VRAM | M | `ollama.rs`, `types.ts`, new `lib/vram.ts`, `Toolbar.tsx`, `SidePanels.tsx` |
| 5 | Model download manager and first-run setup | M | `ollama.rs`, `SidePanels.tsx` (ModelsPanel), `useBootstrap.ts` |
| 6 | Model per thread, preloading, one model on the GPU at a time | S | `Toolbar.tsx`, `StatusBar.tsx`, `Composer.tsx`, `store/app.ts`, `ollama.rs` |
| 7 | Thinking on/off per turn | S | `Composer.tsx`, `store/app.ts` |
| 8 | Windows notifications and taskbar progress | S | `Cargo.toml`, `lib.rs`, `useBootstrap.ts`, `SettingsPanel.tsx` |
| 9 | Queue or redirect messages while the agent works | S/M | `Composer.tsx`, `store/app.ts` |
| 10 | Accurate speed numbers: prompt vs generation, time to first token, live speed | S | `threads.ts`, `Turn.tsx`, `StatusBar.tsx` |
| 11 | Managing the context budget on small context windows | S/M | `SidePanels.tsx`, `SettingsPanel.tsx`, `Composer.tsx`, `system.rs` |
| 12 | Task-list / plan panel | S | new tab in `Inspector.tsx` |
| 13 | Real Edit / Retry / branch behaviour | S/M | `Turn.tsx`, `store/app.ts` |
| 14 | Drag-and-drop and richer file mentions | S/M | `Composer.tsx`, `system.rs` |
| 15 | Sessions organised by project | S/M | `Sidebar.tsx`, `ChatArea.tsx`, `settings.rs` |
| 16 | Hardening for offline use | S | `runtime.rs`, `SettingsPanel.tsx`, `tauri.conf.json` |
| 17 | Single instance and keep running in the tray | S | `lib.rs`, `Cargo.toml` |
| 18 | Better GPU and system monitor | S/M | `system.rs`, `StatusBar.tsx` |
| 19 | Editor for project instructions / system prompt | S/M | `SettingsPanel.tsx` or `Inspector.tsx` |
| 20 | Diagnostics: runtime version check and Ollama logs | S | `runtime.rs`, `SidePanels.tsx` (LogsPanel) |
| 21 | Git panel | L | new tab in `Inspector.tsx` |
| 22 | Background jobs and terminal panel | M | new bottom panel |

### 1. Review a turn's changes, undo and revert (checkpoints). Effort: L (M for a first version)
- **Why users value it:** every coding-agent app builds trust around this:
  - Claude Code desktop: a `+12 -1` change indicator, a diff viewer and comments on lines.
  - Zed: a "Review Changes" view with accept/reject for each change block, plus "Restore Checkpoint".
  - Cursor: checkpoints that revert files.
  - Codex: a review pane that can stage or revert chunks and files.
  - OpenCode: `/undo` reverts the agent's file changes.
  - Small local models make more bad edits, so a one-click revert matters even more here.
- **What's missing:**
  - Diffs are view-only.
  - The list of files changed in a turn is stored (`threads.ts@HEAD:259-265`) but never shown.
  - The upstream API already has the pieces, and none are used:
    - `/patch-undo` and `/file-revert` (`runtime_api.rs:2564-2565`);
    - the per-tool-call changes endpoint `/calls/{id}/changes` (`:2593`);
    - `/v1/workspace/diff` (`:2463`).
  - Reverting files requires the thread to be in trusted mode or full access (`runtime_api.rs:7585`, `:8233-8235`). The app never sets trusted mode (`app.ts@HEAD:179-185`), so it needs a "Trust this project" switch that sends `PATCH trust_mode` (`runtime_threads.rs:4945`).
- **Where:**
  - a new `components/chat/TurnChanges.tsx` shown at the end of each turn (`Turn.tsx`);
  - a new "Изменения" tab next to the existing ones (`Inspector.tsx:255-260`);
  - `undoTurn` and `revertFile` actions in `store/app.ts`;
  - Revert and Open buttons in `DiffView`.
- **First version (M):** an "Undo turn and its file changes" button. It calls `POST /patch-undo`, opens the new branched thread it returns, and puts `original_user_text` (`runtime_api.rs:7456-7461`) back into the input box.

### 2. Plan/Act mode and permission level, plus better approvals. Effort: S/M
- **Why users value it:**
  - Claude Code desktop has a mode selector next to the send button (Manual / Accept edits / Plan / Auto / Bypass).
  - OpenCode switches between Plan and Build with Tab.
  - Zed has Write / Ask / Minimal profiles.
  - Plan mode is read-only, which suits weak local models well: they explore and plan, then a separate step makes the changes.
- **What's missing:**
  - Upstream supports three modes (plan / act / operate) and three permission levels (ask / auto_review / full_access) (`crates/runtime/src/runtime_policy.rs:1-7`, `:25-31`, `:60-82`). Thread creation, thread update and each turn all accept them (`runtime_threads.rs:4918-4922`, `:4948-4949`, `:5005-5009`).
  - The app never sends either setting (`app.ts@HEAD:179-185`, `:201-211`).
  - Auto-approve is imitated in the client with a 60 ms timer, so it only works for threads the window is subscribed to (`useBootstrap.ts@HEAD:13-31`).
  - "Всегда в этой сессии" grants can't be listed or revoked, although the API exists (`runtime_api.rs:2638`).
  - Approval buttons work only with the mouse (`Blocks.tsx:253-263`). There is no keyboard shortcut (for example Y/Enter to allow, N to deny), and no way to jump to the next pending approval.
  - There are no command-pattern rules (for example "always allow `cargo test*`").
- **Where:**
  - a segmented control in the input box's toolbar row (`Composer.tsx:393-437`);
  - a `setThreadPolicy` action (PATCH) in `store/app.ts`;
  - `SettingsPanel.tsx:226-236` becomes "default permission level";
  - a Tab or Shift+Tab hotkey in `useShortcuts.ts`.

### 3. Crash recovery and resilience. Effort: S/M
- **Why users value it:** local setups crash (Ollama out of memory, CUDA errors, runtime panics). An app that still says "running" after the runtime has died destroys trust.
- **What's missing:**
  - **No watchdog for the runtime process.**
    - `endpoint()` returns the old port without checking whether the process is still alive (`runtime.rs:91-96`).
    - Liveness is only checked in `ensure_started` (`:98-111`), which `cw_request` calls only when there is no endpoint at all (`api.rs:54-60`).
    - So after a crash the UI still shows "running" and the event stream retries forever (`api.rs:148-153`).
    - Fix: watch the child process, emit `failed`, restart automatically with backoff, and resubscribe from the last event number seen.
  - **Full-screen error screens:** when Ollama goes offline or the runtime fails, the whole chat is replaced by a full screen (`App.tsx:19-20`), so the history can't even be read. A banner would be better.
  - **Raw errors:** Ollama out-of-memory errors are shown as raw text (`Turn.tsx@HEAD:129`, `Blocks.tsx:122-148`). They should be turned into actions such as "Unload other models" or "Use the 16k version".
  - **Drafts leak between threads:** the input box keeps its text in one state variable (`Composer.tsx:47`) and isn't reset per thread (`ChatArea.tsx:146`). The draft carries over to the next thread opened and is lost on a crash. Save a draft per thread.
  - **Unused recovery endpoints:** `/v1/sessions/repair`, `/v1/thread-history/operations/recover` and `/v1/crashes` exist (`runtime_api.rs:2374`, `:2495`, `:2473`). They could be offered on the "CodeWhale failed" screen (`StatusScreens.tsx:37-77`).

### 4. Model picker that shows whether a model fits in VRAM. Effort: M
- **Why users value it:**
  - Jan labels each model "Fits / May be slow / Won't fit" based on the user's hardware.
  - LM Studio labels "Full / Partial GPU Offload Possible / Likely too large", and its `--estimate-only` check takes context length into account.
  - On one 8 GB card, spilling part of a model to the CPU drops speed from about 40 to about 1-2 tokens/s.
- **What's missing:**
  - The picker shows size, quantization and context (`Toolbar.tsx:93-113`) but never compares the model against the GPU's memory, even though the app already reads it (`StatusBar.tsx:85-92`).
  - `ollama.rs:101-105` reads only `*.context_length`. To estimate the memory a given context needs, it also needs the layer count (`block_count`), the KV head count (`attention.head_count_kv`) and the head size (`attention.key_length`).
    - Context memory ≈ 2 × layers × KV heads × head size × context length × bytes per value.
  - The actual GPU/CPU split is only visible after the model loads (`SidePanels.tsx:221-226`, `Inspector.tsx:199-202`).
- **Where:**
  - extend the model data in `ollama.rs` and `types.ts`;
  - a new `lib/vram.ts` estimator;
  - fit badges in the toolbar picker and ModelsPanel (`SidePanels.tsx:203-276`);
  - estimated VRAM next to each context option, plus "largest context that fits fully on the GPU" (`SidePanels.tsx:255-273`).

### 5. Model download manager and first-run setup. Effort: M
- **Why users value it:**
  - Open WebUI downloads by model tag with a progress bar and offers "Pull 'x' from Ollama.com" right in the model selector.
  - Jan and LM Studio have model catalogues with fit labels.
  - Today users have to go back to PowerShell, which is exactly what this app is meant to replace.
- **What's missing:**
  - The Models panel tells users to run `ollama pull имя` (`SidePanels.tsx:199-200`).
  - `ollama.rs` has no download or delete commands (the registered commands are at `lib.rs:38-41`).
  - Ollama's `/api/pull` streams `status`, `total` and `completed` and resumes a cancelled download.
- **Where:**
  - `ollama.rs`:
    - add `ollama_pull`, which forwards progress as an `ollama-pull` app event and is cancelled by aborting the task;
    - add `ollama_delete`.
  - ModelsPanel:
    - a Download field;
    - a short curated list of agent-capable models with fit badges;
    - after a download, offer to create a 32k version automatically (reusing `ollama_create_variant`).
  - Replace the hard-coded `PREFERRED_MODELS` (`useBootstrap.ts@HEAD:7`) with a first-run setup: detect the GPU, recommend a model, download it, create the larger-context version.

### 6. Model per thread, preloading, one model on the GPU at a time. Effort: S
- **Why users value it:**
  - Claude Code desktop has a model picker next to the send button that works mid-session.
  - AnythingLLM sets a model per workspace.
  - LM Studio loads models on demand and evicts the previous one.
  - On one GPU, two loaded models fight over VRAM.
- **What's missing:**
  - **The wrong model is shown:**
    - the toolbar, status bar and input box all show the global default model, not the open thread's model (`Toolbar.tsx:68`, `StatusBar.tsx:35`, `Composer.tsx:61-70`);
    - so an older thread can show the wrong model and the wrong image support.
    - `setModel` rewrites the global default *and* changes the active thread (`app.ts@HEAD:234-247`).
  - **No preloading:** the first turn sits on "Модель загружается…" (`Turn.tsx@HEAD:126`).
  - **Old model stays loaded:** after a switch, the previous model stays loaded until Ollama's 5-minute keep-alive expires, so the new one may land partly on the CPU.
- **Fix:**
  - show the thread's own model, and keep "default for new sessions" separate;
  - when switching:
    - unload the previous model (`ollama.rs:158-169`);
    - preload the new one with a loading indicator;
  - add a keep-alive setting (for example, unload on exit to free VRAM for games);
  - add a model-switch hotkey.

### 7. Thinking on/off per turn. Effort: S
- **Why users value it:** with qwen3 and deepseek-r1, hidden reasoning adds tens of seconds to every tool step on a consumer GPU. Turning it off for mechanical edits is the single biggest speed win.
- **What's missing:**
  - Upstream accepts `reasoning_effort` on each turn and on the thread (`runtime_threads.rs:5000`, `:4912`).
  - The value `"off"` becomes Ollama's `think:false` (`client.rs:5699`, `:5782-5785`).
  - The app only hides thinking on screen and never sends the setting (`app.ts@HEAD:201-211`).
- **Where:** a toggle in the input box's toolbar, shown only for models that can think (`Toolbar.tsx:47-51`). Add `reasoning_effort` to the body sent by `sendPrompt`.

### 8. Windows notifications and taskbar progress. Effort: S
- **Why users value it:** Claude Code desktop sends an OS notification when a session finishes and you aren't viewing it. Zed offers a notification and a sound. Local turns take minutes, so people switch to other windows.
- **What's missing:**
  - The only Tauri plugins are opener and dialog (`Cargo.toml`).
  - An approval waiting in a background thread shows only a small icon, and only after the session list refreshes (`Sidebar.tsx:110-114`).
- **Where:**
  - add the notification plugin (`Cargo.toml` and `lib.rs`);
  - in `useBootstrap` `onEvent`, notify on turn end, `approval.required` and `user_input.required` when the window is unfocused or the thread isn't open;
  - show progress on the Windows taskbar icon;
  - add a toggle in Settings.

### 9. Queue or redirect messages while the agent works. Effort: S/M
- **Why users value it:**
  - Cursor queues follow-up messages; "send now" redirects the agent at its next tool call.
  - Zed also queues messages and has "Send Now".
  - Correcting a small model mid-run is better than stopping and starting over.
- **What's missing:**
  - The input box refuses to send while a turn is running (`Composer.tsx:198`, `:448`).
  - Upstream has an endpoint for redirecting a running turn: `POST /turns/{turn_id}/steer` (`runtime_api.rs:2578`, handler at `:8609-8620`).
- **Where:** in `Composer` `submit()`:
  - Ctrl+Enter redirects the running turn;
  - Enter queues the message, shown as chips above the input box, and sends it when the turn ends.

### 10. Accurate speed numbers. Effort: S
- **Why users value it:** for agents, processing a 10k-token prompt often dominates on consumer GPUs. Users need to see whether slowness comes from prompt processing (fix: smaller context or fewer tools) or from generation (fix: smaller model or quantization).
- **What's missing:**
  - Tokens/s divides output tokens by the whole duration, including the wait for the first token (`threads.ts@HEAD:254`). This understates generation speed.
  - Time to first token is stored (`:255`) but never displayed. Upstream sends `first_token_ms` and `request_ms` (`runtime_threads.rs:18375-18392`).
  - There is no live speed while the answer is streaming.
  - The status bar doesn't warn when part of the model runs on the CPU.
- **Where:**
  - the `turn.usage` handler in `threads.ts`;
  - the turn footer (`Turn.tsx@HEAD:131-139`);
  - `StatusBar.tsx:72-74` and `:85-92`: add a "⚠ N% CPU" segment from the loaded-model data.

### 11. Managing the context budget on small context windows. Effort: S/M
- **Why users value it:**
  - Ollama defaults to a 4k context on GPUs with less than 24 GiB of VRAM.
  - Its `q8_0` context cache uses about half the memory of the default, but only when Flash Attention is on. On 8 GB that roughly doubles the usable context.
- **What's missing:**
  - The "bigger context" options are a fixed 16k-64k list with no memory estimate (`SidePanels.tsx:163`, `:255-273`).
  - There is no view of, or help with, the Ollama settings that matter:
    - `OLLAMA_FLASH_ATTENTION`, `OLLAMA_KV_CACHE_TYPE` and `OLLAMA_CONTEXT_LENGTH`;
    - on Windows these are user environment variables and need an Ollama restart.
  - File mentions don't show how many tokens they add, and nothing warns before a send that will trigger history compaction. The app already gets `available_input_tokens` (`types.ts:107`).
  - The larger-context versions can't be deleted from the app.
- **Where:**
  - ModelsPanel;
  - a new "Ollama server" section in Settings (next to the Ollama section at `SettingsPanel.tsx:84-122`);
  - mention chips in the input box;
  - a Rust command that sets the environment variables and restarts Ollama, after the user confirms.

### 12. Task-list / plan panel. Effort: S
- **Why users value it:** Claude Code desktop has a tasks pane for background work in the session. A visible task list shows how far a multi-step job has got and reveals when a small model is going in circles.
- **What's missing:**
  - The light tool set includes the agent's task-list tool, `todo_write` (`tools.ts:68`), but its output is shown as a generic tool block.
  - Upstream has `GET /v1/threads/{id}/todo` and `/plan` (`runtime_api.rs:2528-2529`).
- **Where:**
  - a new "План" tab in the Inspector;
  - a compact checklist pinned above the input box while a turn runs.

### 13. Real Edit / Retry / branch behaviour. Effort: S/M
- **Why users value it:**
  - In Zed every sent message can be edited and resubmitted.
  - OpenCode has `/undo` and `/redo`.
  - Msty runs models side by side and branches conversations.
- **What's missing:**
  - **Edit** only copies the text into the input box and adds a new turn (`Turn.tsx@HEAD:61`).
  - **Retry** resends the prompt as a new turn and keeps the failed one in context (`Turn.tsx@HEAD:104`, `:129`). This wastes a small context window.
  - Upstream has the right endpoints (`runtime_api.rs:2562-2566`):
    - `/undo`;
    - `/fork-at-turn`, which branches the thread at a given turn;
    - `/retry`.
- **Where:**
  - the message actions in `Turn.tsx`;
  - new actions in `store/app.ts`;
  - a "Retry with model…" menu: branch the thread, switch its model and retry. Useful for comparing local models.

### 14. Drag-and-drop and richer file mentions. Effort: S/M
- **Why users value it:**
  - Claude Code desktop accepts files dragged into the prompt.
  - In Zed, @ can mention files, folders, symbols, earlier threads, diagnostics and branch diffs.
- **What's missing:**
  - **No drag-and-drop:** nothing in `src` handles dropped files (`tauri.conf.json:13-25` is the window config).
  - **Mentions are bare:**
    - they are plain text with no chip and no size;
    - a folder mention is inserted as `@dir/` and not expanded;
    - there is no `@diff` or `@errors`, although upstream has LSP diagnostics (`runtime_api.rs:2551-2555`) and the workspace diff (`:2463`).
- **Where:** `Composer.tsx` (Tauri's drag-drop event and chips); file sizes via `path_info` (`system.rs:56-60`).

### 15. Sessions organised by project. Effort: S/M
- **Why users value it:**
  - Claude Code desktop groups sessions by project.
  - AnythingLLM gives each workspace its own model.
- **What's missing:**
  - Sessions are grouped only by date (`Sidebar.tsx:257-264`, `:325-339`).
  - Switching project restarts CodeWhale (`app.ts@HEAD:293-307`; the uncommitted changes add a confirmation).
  - @-file search always uses the server's working folder (`runtime_api/workspace.rs:28-48`). So a thread from another project silently gets wrong file suggestions.
  - There are no per-project defaults (model, permission level, instructions).
- **Where:**
  - a project filter or grouping in `Sidebar.tsx`;
  - a workspace badge with a "switch project?" prompt in `SessionHeader` (`ChatArea.tsx:18-132`);
  - per-project settings in `settings.rs:18-46`.

### 16. Hardening for offline use. Effort: S
- **What's missing:**
  - **Environment passed to CodeWhale:** `runtime.rs:158-169` sets only the token, `NO_COLOR` and the telemetry switch.
    - Add `CODEWHALE_NO_UPDATE_CHECK=1` (`release/src/check.rs:33`).
    - Pass the Ollama address from Settings as `OLLAMA_BASE_URL` (`config/src/lib.rs:7575`). Today the "Адрес" field (`SettingsPanel.tsx:84-91`) only affects the app's own Ollama calls (`ollama.rs:28-32`), not the agent's.
  - **Offline mode:** add a toggle that removes the network tools (Web, web search, fetch URL) from the agent's allowed tools (`tools.ts:40-42`, `:64`).
  - **Installer:** it downloads WebView2 during install (`tauri.conf.json:45`). Offer an offline installer build.

### 17. Single instance and keep running in the tray. Effort: S
- **What's missing:**
  - Nothing stops a second copy of the app. A second launch would start another CodeWhale server, which competes for the runtime's lock file (`runtime.rs:356-386`) and for VRAM.
  - Closing the window kills CodeWhale and any running turns (`lib.rs:53-59`).
- **Where:** add single-instance and tray plugins in `lib.rs`. On close, confirm if the agent is still running.

### 18. Better GPU and system monitor. Effort: S/M
- **What's missing:**
  - Stats come only from nvidia-smi and only for the first GPU (`system.rs:14-39`), so AMD and Intel users see nothing.
  - There is no history or sparkline and no RAM figure.
  - Ollama's unload time (`expires_at`, `types.ts:155`) is never shown.
- **Where:** `system.rs` (a Windows fallback), a popover on `StatusBar.tsx:85-92`.

### 19. Editor for project instructions / system prompt. Effort: S/M
- **Why users value it:** small models need explicit house rules, for example the reply language, the test command, and that commands run in PowerShell.
- **What's missing:**
  - The thread's `system_prompt` can be changed via the API (`runtime_threads.rs:4951`).
  - `GET /v1/workspace/instructions` exists (`runtime_api.rs:2408`).
  - Neither has any UI in the app.

### 20. Diagnostics. Effort: S
- **What's missing:**
  - At startup the app reads only `codewhale_version` (`runtime.rs:234-241`) and ignores `runtime_api_version` (`runtime_api.rs:1219-1221`). A different npm-installed CodeWhale could break the app silently.
  - Ollama's own log isn't viewable.
  - The log panel shows only the runtime's output (`SidePanels.tsx:80-102`).

### 21. Git panel. Effort: L
- **Why users value it:** the Codex app commits, pushes and opens PRs.
- **API already there:** `/v1/git`, `/v1/changes`, `/v1/diff`, stage, unstage, discard and commit (`runtime_api.rs:2460-2470`).

### 22. Background jobs and terminal panel. Effort: M
- **Why users value it:** Claude Code desktop has a built-in terminal (Ctrl+`).
- **API already there:** thread jobs (`runtime_api.rs:2507-2526`) and the terminal endpoints (`:2391-2397`).

**About the sources.** WebFetch could not resolve ollama.com, jan.ai, zed.dev, cursor.com or lmstudio.ai. The Ollama and Zed points come from their docs on GitHub. The Jan and LM Studio points come from search-result summaries, so treat them as secondary.

Sources:
- [Ollama context length doc](https://raw.githubusercontent.com/ollama/ollama/main/docs/context-length.mdx) · [Ollama FAQ](https://raw.githubusercontent.com/ollama/ollama/main/docs/faq.mdx) · [Ollama API](https://raw.githubusercontent.com/ollama/ollama/main/docs/api.md) · [Ollama new app](https://registry.ollama.ai/blog/new-app)
- [LM Studio lms load](https://www.lmstudio.ai/docs/cli/local-models/load) · [LM Studio TTL and auto-evict](https://lmstudio.ai/docs/app/api/ttl-and-auto-evict) · [LM Studio "Likely too large" issue](https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/651)
- [Jan manage models](https://www.jan.ai/docs/desktop/manage-models) · [Msty features](https://msty.ai/studio/features) · [Open WebUI with Ollama](https://docs.openwebui.com/getting-started/quick-start/connect-a-provider/starting-with-ollama.txt) · [AnythingLLM agents](https://docs.anythingllm.com/agent/setup)
- [Claude Code desktop](https://code.claude.com/docs/en/desktop) · [Zed agent panel](https://raw.githubusercontent.com/zed-industries/zed/main/docs/src/ai/agent-panel.md) · [Cursor checkpoints](https://cursor.com/docs/agent/chat/checkpoints) · [Codex app features](https://developers.openai.com/codex/app/features) · [OpenCode cheatsheet](https://opencode.school/cheatsheet)