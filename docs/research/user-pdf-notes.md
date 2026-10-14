# Notes: user's PDF "Автономные локальные ИИ-агенты: от аналога Claude Code до GameDev-пайплайна" (2026)

Source: PDF provided by the user (15 pages). Summary of ideas relevant to CodeWhale Desktop; claims marked (?) need verification.

## Inference / VRAM (8 GB VRAM + 32 GB RAM)
- Ollama env: OLLAMA_FLASH_ATTENTION=1, OLLAMA_KV_CACHE_TYPE=q8_0, OLLAMA_KEEP_ALIVE=30m, OLLAMA_NUM_PARALLEL=1.
- Default-context trap (2048/4096) -> always pin num_ctx via Modelfile/variant.
- "OLLAMA_VRAM_OVERHEAD" to reserve VRAM for Godot/browser (?) — the documented Ollama var is believed to be OLLAMA_GPU_OVERHEAD.
- NVIDIA "CUDA - Sysmem Fallback Policy" -> "Prefer No Sysmem Fallback".
- num_thread = physical cores; Q4_K_M/Q5_K_M for CPU-offloaded parts.
- MoE (30B-A3B) with expert tensors on CPU: llama-server `--override-tensor "\.ffn_.*_exps\.=CPU"` (or --cpu-moe), attention + KV on GPU; claimed 32-48 tok/s (?).
- Auto VRAM<->RAM balancer computing num_gpu from nvidia-smi free memory minus engine reserve.
- Prefix caching (llama-server --cache-reuse), speculative decoding with small draft model (?) gains.

## Agent workflow
- Plan (reasoning model) -> Work (fast coder) dual-model routing; CodeWhale modes Plan/Work/YOLO (?) switch with Tab.
- side-git snapshots, /undo /restore (?) before autonomous runs.
- Zero-mutation prefix: keep first ~4000 tokens static (system prompt + AGENTS.md + tool decls).
- RepoMap via tree-sitter; long-term notes in SQLite/LanceDB via MCP.
- Headless self-check loop: `godot --headless --check-only --script <file.gd>` (?) after every save; `godot --headless --script tool.gd --quit` for EditorScript scene building.
- AGENTS.md Godot 4 template (forbid Godot 3 APIs, static typing, mandatory headless check).
- Skills library (.agents/skills: shaders_ref, fsm_template, scene_builder, daily_ops).
- Example Modelfile with num_ctx 24576, temperature 0.15, SYSTEM prompt.

## Web access
- HTML -> clean Markdown filter before the model (VRAM token guard ~12k chars / ~3k tokens).
- Options: Jina Reader (r.jina.ai / s.jina.ai), Context7 MCP (@upstash/context7-mcp), Tavily/Brave MCP, SearXNG + Crawl4AI (fully local).
- Browser bridge: desktop app WebSocket 127.0.0.1:17420 + Chrome MV3 extension (GET_ACTIVE_TAB, FETCH_URL in background tab); or @playwright/mcp with --cdp-endpoint to the user's Chrome; or raw CDP :9222.
- UI ideas: "Attach active tab" button, "Send Chrome console errors" button, VRAM token guard.

## Reliability patterns (attributed to Anthropic engineering posts)
- Two-store memory: read-only AGENTS.md + read-write .agent_state/ledger.json.
- Bookmark-based log reading (byte offsets in bookmarks.json) -> only new log lines.
- Failed-read visibility: return [SOURCE_UNREADABLE: reason] instead of empty output.
- Run cap: maxSteps (e.g. 10) -> pause with budget_reached.
- Secrets stay outside the model (browser bridge passes URLs, cookies stay in Chrome).
- Confirm before recording: mark task DONE only after headless check exit code 0.

## Offline knowledge drive (later)
- 1 TB drive: models, Zeal/Dash docsets, git_vault (godot sources/demos), Kiwix ZIM (StackOverflow, gamedev.SE), LanceDB vectors; MCP server over it.
