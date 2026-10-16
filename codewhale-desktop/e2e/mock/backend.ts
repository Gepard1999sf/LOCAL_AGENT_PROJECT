/**
 * Fake desktop backend: answers every Tauri command of src/lib/tauri.ts and the
 * CodeWhale runtime HTTP API behind `cw_request`, and streams thread events
 * behind `cw_subscribe`, so the React UI runs in plain Chromium.
 *
 * It runs in the Playwright (Node) process. The page reaches it through the
 * binding `__cwMockInvoke` (see tauri-shim.ts); events go back through
 * `page.evaluate(window.__CW_MOCK__.emitMany)`.
 *
 * State is per test: threads, turns, items and an event journal per thread with
 * monotonically increasing `seq`, so reconnects (`since_seq`) replay exactly
 * like the real server.
 */
import type { Page } from "@playwright/test";
import { FixtureStore, clone, shaped, type Recording } from "./fixtures";
import { installTauriShim } from "./tauri-shim";
import {
  journalEvent,
  progressEnvelope,
  recordId,
  toEnvelope,
  type CwEventEnvelope,
  type JournalEvent,
} from "./events";
import { TurnCtx, TurnInterrupted, defaultTurnScript, type PendingApproval, type TurnHost, type TurnScript } from "./turn";
import * as D from "./defaults";

export const BINDING = "__cwMockInvoke";

export interface InvokeCall {
  cmd: string;
  args: any;
  at: number;
}

export interface HttpCall {
  method: string;
  path: string;
  body: any;
  status: number;
  at: number;
}

export class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

type Reply = { ok: true; value: unknown } | { ok: false; error: unknown };
type CommandHandler = (args: any) => unknown | Promise<unknown>;
type RouteHandler = (m: RegExpMatchArray, body: any, path: string) => unknown | Promise<unknown>;

interface Route {
  method: string;
  re: RegExp;
  handle: RouteHandler;
}

export interface MockThread {
  record: Record<string, any>;
  turns: Array<Record<string, any>>;
  journal: JournalEvent[];
  seq: number;
  subscribed: boolean;
  activeTurn?: TurnCtx;
}

interface Waiter<T> {
  promise: Promise<T>;
  resolve: (v: T) => void;
}

function waiter<T>(): Waiter<T> {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

const ACTIVE = new Set(["queued", "in_progress"]);

export class MockBackend implements TurnHost {
  readonly fx: FixtureStore;

  // ---- Rust-side state (src-tauri) ----
  settings: Record<string, any> = clone(D.DEFAULT_SETTINGS);
  runtime: Record<string, any> = clone(D.RUNNING_RUNTIME);
  ollama: Record<string, any> = clone(D.OLLAMA_ONLINE);
  models: Array<Record<string, any>> = clone(D.OLLAMA_MODELS);
  gpu: Record<string, any> | null = clone(D.GPU);
  logs: string[] = clone(D.RUNTIME_LOGS);
  branch: string | null = "main";
  /** What `runtime_restart` / `runtime_start` switch the runtime to. */
  restartTo: Record<string, any> = clone(D.RUNNING_RUNTIME);
  /** Result of `plugin:dialog|open` (a path, several paths or null = cancelled). */
  dialogOpenResult: string | string[] | null = null;
  dialogSaveResult: string | null = "C:\\Users\\dev\\Documents\\codewhale-export.md";
  readonly exports = new Map<string, string>();
  /** Knowledge drive (src-tauri/src/drive.rs): mutated by the drive_* commands below. */
  drive: Record<string, any> = clone(D.DRIVE_STATUS);

  // ---- CodeWhale runtime state ----
  readonly threads = new Map<string, MockThread>();
  private readonly approvals = new Map<string, PendingApproval & { wait: Waiter<{ decision: string; remember: boolean }> }>();
  private readonly userInputs = new Map<string, { threadId: string; turnId: string; request: unknown; wait: Waiter<unknown> }>();
  private readonly turnQueue: TurnScript[] = [];
  defaultTurn: TurnScript = defaultTurnScript;
  /** Delay before a scripted turn starts emitting (lets POST .../turns return first). */
  turnStartDelayMs = 20;

  // ---- Recording ----
  readonly invokes: InvokeCall[] = [];
  readonly http: HttpCall[] = [];
  /** Commands/routes the mock did not know (answered with null / 404). */
  readonly unhandled: string[] = [];
  /** Errors thrown inside turn scripts. */
  readonly scriptErrors: unknown[] = [];

  private page?: Page;
  private readonly commandOverrides = new Map<string, CommandHandler>();
  private readonly extraRoutes: Route[] = [];
  private readonly gates = new Map<string, Waiter<void>>();
  private releasedAll = false;
  private readonly runningScripts = new Set<Promise<void>>();
  private outbox: Array<[string, unknown]> = [];
  private flushing: Promise<void> | null = null;

  constructor(opts: { fixtures?: FixtureStore } = {}) {
    this.fx = opts.fixtures ?? FixtureStore.load();
  }

  // =====================================================================
  // Wiring
  // =====================================================================

  /** Installs the IPC shim and the binding. Call before `page.goto`. */
  async attach(page: Page): Promise<void> {
    this.page = page;
    await page.exposeBinding(BINDING, async (_source, cmd: string, args: any) => this.handle(cmd, args));
    await page.addInitScript(installTauriShim, { bindingName: BINDING, windowLabel: "main" });
  }

  /** Emits a Tauri event to the page (e.g. "runtime-status", "runtime-log", "cw-event"). */
  emit(event: string, payload: unknown): Promise<void> {
    this.outbox.push([event, payload]);
    return this.flush();
  }

  private flush(): Promise<void> {
    if (!this.flushing) {
      this.flushing = (async () => {
        while (this.outbox.length) {
          const batch = this.outbox.splice(0);
          const page = this.page;
          if (!page || page.isClosed()) continue;
          await page
            .evaluate((b) => (window as any).__CW_MOCK__?.emitMany(b), batch)
            .catch(() => {
              /* page navigating or closed: events are lost, as with a real reload */
            });
        }
        this.flushing = null;
      })();
    }
    return this.flushing;
  }

  /** Overrides a Tauri command. The handler may throw a string to reject like a Rust `Err`. */
  onCommand(cmd: string, handler: CommandHandler): this {
    this.commandOverrides.set(cmd, handler);
    return this;
  }

  /** Adds an HTTP route checked before the built-in ones. Throw `HttpError` for non-2xx. */
  route(method: string, re: RegExp, handle: RouteHandler): this {
    this.extraRoutes.push({ method: method.toUpperCase(), re, handle });
    return this;
  }

  /** Queues a script for the next POST /v1/threads/{id}/turns. */
  scriptNextTurn(script: TurnScript): this {
    this.turnQueue.push(script);
    return this;
  }

  /** Opens a named gate a turn script waits on (see TurnCtx.gate). */
  release(name: string): void {
    this.gateWaiter(name).resolve();
  }

  /** Opens every gate, including ones scripts have not reached yet. */
  releaseAll(): void {
    this.releasedAll = true;
    for (const w of this.gates.values()) w.resolve();
  }

  gate(name: string): Promise<void> {
    if (this.releasedAll) return Promise.resolve();
    return this.gateWaiter(name).promise;
  }

  private gateWaiter(name: string): Waiter<void> {
    let w = this.gates.get(name);
    if (!w) {
      w = waiter<void>();
      this.gates.set(name, w);
    }
    return w;
  }

  /** Waits until every running turn script finished (or the timeout passed). */
  async settle(timeoutMs = 5000): Promise<void> {
    const all = Promise.all([...this.runningScripts]);
    await Promise.race([all, new Promise((r) => setTimeout(r, timeoutMs))]);
    await this.flush();
  }

  // ---- Test conveniences ----

  calls(cmd: string): InvokeCall[] {
    return this.invokes.filter((c) => c.cmd === cmd);
  }

  requests(method?: string, path?: RegExp | string): HttpCall[] {
    return this.http.filter(
      (h) =>
        (!method || h.method === method.toUpperCase()) &&
        (!path || (typeof path === "string" ? h.path === path : path.test(h.path))),
    );
  }

  /** Switches the runtime state and tells the UI (like runtime.rs emitting "runtime-status"). */
  async setRuntime(patch: Record<string, any>, notify = true): Promise<void> {
    this.runtime = { ...this.runtime, ...patch };
    if (notify) await this.emit("runtime-status", clone(this.runtime));
  }

  setOllamaOnline(online: boolean): void {
    this.ollama = online ? clone(D.OLLAMA_ONLINE) : { online: false, url: this.settings.ollamaUrl };
  }

  async log(line: string): Promise<void> {
    this.logs.push(line);
    await this.emit("runtime-log", line);
  }

  /**
   * Creates a thread whose history is already in the journal (as if it ran
   * earlier). `turns` run through the same scripts with no delays.
   */
  async seedThread(opts: {
    title?: string;
    model?: string;
    turns?: Array<{ prompt: string; script?: TurnScript }>;
  } = {}): Promise<string> {
    const record = this.createThreadRecord({ model: opts.model ?? this.settings.defaultModel ?? "qwen3-8b-16k" });
    if (opts.title) record.title = opts.title;
    const thread = this.addThread(record);
    for (const t of opts.turns ?? []) {
      const turn = this.createTurnRecord(thread, t.prompt);
      const ctx = new TurnCtx(this, record.id, turn.id, t.prompt, record.model, { fast: true });
      await (t.script ?? this.defaultTurn)(ctx);
    }
    return record.id;
  }

  /**
   * Creates a thread whose journal is a recorded SSE capture from the real server
   * (fixtures/live/*__sse.json), frames kept byte-for-byte except that, like
   * api.rs, `model.tools.snapshot` frames are dropped. Returns the thread id.
   */
  seedFromCapture(rec: Recording, opts: { title?: string } = {}): string {
    const frames = (rec.events ?? []).filter(
      (f) => f.id != null && f.event !== "model.tools.snapshot" && f.data && typeof f.data === "object",
    );
    if (!frames.length) throw new Error(`seedFromCapture: ${rec.file} has no journal frames`);
    const threadId: string = frames[0].data.thread_id;
    const started = frames.find((f) => f.event === "thread.started")?.data?.payload?.thread;
    const record = started
      ? clone(started)
      : this.createThreadRecord({ model: frames.find((f) => f.data?.payload?.turn)?.data.payload.turn.effective_model ?? "qwen3:4b" });
    record.id = threadId;
    if (opts.title) record.title = opts.title;
    const thread: MockThread = { record, turns: [], journal: [], seq: 0, subscribed: false };
    for (const f of frames) {
      const ev = clone(f.data) as JournalEvent;
      thread.journal.push(ev);
      thread.seq = Math.max(thread.seq, Number(f.id));
      const turn = ev.payload?.turn;
      if (turn?.id) {
        const i = thread.turns.findIndex((t) => t.id === turn.id);
        if (i >= 0) thread.turns[i] = { ...thread.turns[i], ...turn };
        else thread.turns.push(turn);
      }
    }
    record.updated_at = new Date().toISOString();
    this.threads.set(threadId, thread);
    return threadId;
  }

  // =====================================================================
  // Command dispatch
  // =====================================================================

  async handle(cmd: string, args: any): Promise<Reply> {
    this.invokes.push({ cmd, args: clone(args), at: Date.now() });
    try {
      const override = this.commandOverrides.get(cmd);
      const value = override ? await override(args) : await this.builtin(cmd, args ?? {});
      return { ok: true, value: value === undefined ? null : clone(value) };
    } catch (e) {
      return { ok: false, error: e instanceof Error ? e.message : e };
    }
  }

  private async builtin(cmd: string, args: any): Promise<unknown> {
    switch (cmd) {
      case "settings_get":
        return this.settings;
      case "settings_set": {
        const patch = { ...(args.patch ?? {}) };
        delete patch._file;
        this.settings = { ...this.settings, ...patch };
        return this.settings;
      }

      case "runtime_status":
        return this.runtime;
      case "runtime_start":
        if (this.runtime.state === "running") return this.runtime;
        await this.setRuntime(clone(this.restartTo));
        return this.runtime;
      case "runtime_restart":
        await this.setRuntime({ ...clone(this.restartTo), startedAtMs: Date.now() });
        return this.runtime;
      case "runtime_logs":
        return this.logs;

      case "cw_request":
        return this.request(String(args.method ?? "GET"), String(args.path ?? ""), args.body ?? null);
      case "cw_subscribe":
        return this.subscribe(String(args.threadId), args.sinceSeq ?? null);
      case "cw_unsubscribe": {
        const t = this.threads.get(String(args.threadId));
        if (t) t.subscribed = false;
        return null;
      }

      case "ollama_status":
        return this.ollama.online ? this.ollama : { online: false, url: this.ollama.url ?? this.settings.ollamaUrl };
      case "ollama_models":
        if (!this.ollama.online) throw `Ollama недоступна: ${this.settings.ollamaUrl}`;
        return this.models;
      case "ollama_unload":
        if (Array.isArray(this.ollama.loaded)) {
          this.ollama.loaded = this.ollama.loaded.filter((m: any) => m.name !== args.model && m.model !== args.model);
        }
        return null;
      case "ollama_create_variant": {
        const from = this.models.find((m) => m.name === args.from);
        if (!from) throw `Модель ${args.from} не найдена`;
        this.models.push({ ...clone(from), name: args.name, numCtx: args.numCtx });
        return null;
      }

      case "gpu_stats":
        return this.gpu;

      // Knowledge drive (src-tauri/src/drive.rs).
      case "drive_status":
        return this.drive;
      case "drive_register_mcp":
        this.drive.mcpRegistered = true;
        return this.drive.mcpPath;
      case "drive_unregister_mcp":
        this.drive.mcpRegistered = false;
        return this.drive.mcpPath;
      case "drive_set_ollama_models":
        this.drive.ollamaModelsEnv = `${this.drive.root}\\models`;
        this.drive.ollamaModelsOnDrive = true;
        return this.drive.ollamaModelsEnv;
      case "drive_restart_ollama":
        return "C:\\Users\\dev\\AppData\\Local\\Programs\\Ollama\\ollama app.exe";
      case "drive_index_build": {
        const summary = { path: `${this.drive.root}\\index\\git_vault.sqlite`, files: 1234, bytes: 55_000_000, skipped: 10, elapsedMs: 4200 };
        this.drive.index = { path: summary.path, bytes: 80_000_000, files: summary.files, builtAt: "2026-10-16T10:00:00Z" };
        return summary;
      }
      case "drive_kiwix_start": {
        const books = [{ name: "gamedev.stackexchange.com_en_all", title: "Game Development Stack Exchange" }];
        this.drive.kiwixRunning = true;
        this.drive.kiwixOnline = true;
        this.drive.kiwixBooks = books;
        return { url: this.drive.kiwixUrl, running: true, books };
      }
      case "drive_kiwix_stop":
        this.drive.kiwixRunning = false;
        this.drive.kiwixOnline = false;
        this.drive.kiwixBooks = [];
        return true;
      case "drive_install_skills": {
        const base = args.toWorkspace ? `${this.settings.workspace}\\.agents\\skills` : "C:\\Users\\dev\\.codewhale\\skills";
        this.drive.skillsInstalled = ["knowledge-drive", "math-physics-godot"];
        return this.drive.skillsInstalled.map((n: string) => `${base}\\${n}\\SKILL.md`);
      }
      case "drive_install_helpers":
        this.drive.helpersInstalled = true;
        this.drive.sympyHelper = `${this.drive.root}\\bin\\sympy_helper.py`;
        return [this.drive.sympyHelper, `${this.drive.root}\\bin\\init-ai-drive.ps1`];
      case "drive_golden_models":
        return D.GOLDEN_MODELS;
      case "git_branch":
        return this.branch;
      case "path_info": {
        const p = String(args.path ?? "");
        const name = p.split(/[\\/]/).filter(Boolean).pop();
        return { exists: true, isDir: !/\.[A-Za-z0-9]{1,8}$/.test(p), name };
      }
      case "codewhale_home_path":
        return "C:\\Users\\dev\\.codewhale";
      case "open_in_editor":
        return "code";
      case "write_export":
        this.exports.set(String(args.path), String(args.content));
        return null;
      case "read_image_b64":
        return {
          mime: "image/png",
          // 1x1 transparent PNG
          dataBase64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=",
          name: String(args.path ?? "image.png").split(/[\\/]/).pop(),
        };

      // Official plugins used by the UI.
      case "plugin:dialog|open":
        return this.dialogOpenResult;
      case "plugin:dialog|save":
        return this.dialogSaveResult;
      case "plugin:dialog|message":
      case "plugin:dialog|ask":
      case "plugin:dialog|confirm":
        return true;
      case "plugin:opener|open_url":
      case "plugin:opener|open_path":
      case "plugin:opener|reveal_item_in_dir":
      case "plugin:shell|open":
        return null;
      case "plugin:window|get_all_windows":
        return ["main"];
    }
    this.unhandled.push(`invoke ${cmd}`);
    return null;
  }

  // =====================================================================
  // Event streams (cw_subscribe)
  // =====================================================================

  private async subscribe(threadId: string, sinceSeq: number | null): Promise<null> {
    if (!/^[A-Za-z0-9_-]+$/.test(threadId)) throw "Неверный id треда";
    const thread = this.threads.get(threadId);
    // Rust spawns the stream task and returns at once; frames follow asynchronously.
    setTimeout(() => {
      if (!thread) {
        void this.emit("cw-stream", { threadId, state: "error", message: "Тред не найден" });
        return;
      }
      if (this.runtime.state !== "running") {
        void this.emit("cw-stream", { threadId, state: "reconnecting", message: "CodeWhale не запущен" });
        return;
      }
      thread.subscribed = true;
      const since = sinceSeq ?? 0;
      const frames: Array<[string, unknown]> = [["cw-stream", { threadId, state: "connected", message: null }]];
      frames.push(["cw-event", progressEnvelope(threadId, since, false)]);
      for (const ev of thread.journal) if (ev.seq > since) frames.push(["cw-event", toEnvelope(ev)]);
      frames.push(["cw-event", progressEnvelope(threadId, thread.seq, true)]);
      this.outbox.push(...frames);
      void this.flush();
    }, 5);
    return null;
  }

  /** Appends an event to a thread journal and delivers it when the UI holds the stream. */
  async journal(
    threadId: string,
    event: string,
    payload: unknown,
    ids: { turnId?: string | null; itemId?: string | null } = {},
  ): Promise<void> {
    const thread = this.threads.get(threadId);
    if (!thread) throw new Error(`journal: unknown thread ${threadId}`);
    const previousSeq = thread.seq;
    thread.seq += 1;
    const ev = journalEvent({
      seq: thread.seq,
      previousSeq,
      event,
      threadId,
      turnId: ids.turnId ?? null,
      itemId: ids.itemId ?? null,
      payload: clone(payload),
    });
    thread.journal.push(ev);
    thread.record.updated_at = ev.timestamp.replace("+00:00", "Z");
    if (thread.subscribed) await this.emit("cw-event", toEnvelope(ev));
  }

  /** Delivers a raw envelope (e.g. replayed from a live SSE capture) without touching the journal. */
  async pushRaw(env: CwEventEnvelope): Promise<void> {
    await this.emit("cw-event", env);
  }

  turnRecord(threadId: string, turnId: string): Record<string, any> {
    const t = this.threads.get(threadId)?.turns.find((x) => x.id === turnId);
    if (!t) throw new Error(`turnRecord: unknown turn ${turnId}`);
    return t;
  }

  registerApproval(a: PendingApproval): Promise<{ decision: string; remember: boolean }> {
    const wait = waiter<{ decision: string; remember: boolean }>();
    this.approvals.set(a.id, { ...a, wait });
    return wait.promise;
  }

  registerUserInput(threadId: string, turnId: string, inputId: string, request: unknown): Promise<unknown> {
    const wait = waiter<unknown>();
    this.userInputs.set(inputId, { threadId, turnId, request, wait });
    return wait.promise;
  }

  pendingApprovals(threadId?: string) {
    return [...this.approvals.values()].filter((a) => !threadId || a.threadId === threadId);
  }

  // =====================================================================
  // Runtime HTTP API (cw_request)
  // =====================================================================

  private async request(method: string, path: string, body: any): Promise<unknown> {
    method = method.toUpperCase();
    // Same guard as api.rs `allowed_path`.
    if (!((path.startsWith("/v1/") || path === "/health") && !path.includes(".."))) {
      throw `Запрещённый путь API: ${path}`;
    }
    if (this.runtime.state !== "running") throw "CodeWhale не запущен";
    const call: HttpCall = { method, path, body: clone(body), status: 0, at: Date.now() };
    this.http.push(call);
    const pathOnly = path.split("?")[0];
    for (const r of [...this.extraRoutes, ...this.routes]) {
      if (r.method !== method) continue;
      const m = pathOnly.match(r.re);
      if (!m) continue;
      try {
        const res = await r.handle(m, body, path);
        call.status = 200;
        return res;
      } catch (e) {
        if (e instanceof HttpError) {
          call.status = e.status;
          throw `HTTP ${e.status}: ${e.message}`;
        }
        throw e;
      }
    }
    call.status = 404;
    this.unhandled.push(`${method} ${path}`);
    throw `HTTP 404: Not Found`;
  }

  private readonly routes: Route[] = [
    { method: "GET", re: /^\/health$/, handle: () => this.fx.body("GET /health", { status: "ok" }) },
    { method: "GET", re: /^\/v1\/threads\/summary$/, handle: (_m, _b, p) => this.listSummaries(p) },
    { method: "GET", re: /^\/v1\/threads$/, handle: () => [...this.threads.values()].map((t) => t.record) },
    { method: "POST", re: /^\/v1\/threads$/, handle: (_m, b) => this.createThread(b ?? {}) },
    { method: "GET", re: /^\/v1\/threads\/([\w-]+)$/, handle: (m) => this.threadDetail(m[1]) },
    { method: "PATCH", re: /^\/v1\/threads\/([\w-]+)$/, handle: (m, b) => this.patchThread(m[1], b ?? {}) },
    { method: "GET", re: /^\/v1\/threads\/([\w-]+)\/context$/, handle: (m) => this.context(m[1]) },
    { method: "POST", re: /^\/v1\/threads\/([\w-]+)\/turns$/, handle: (m, b) => this.startTurn(m[1], b ?? {}) },
    {
      method: "POST",
      re: /^\/v1\/threads\/([\w-]+)\/turns\/([\w-]+)\/interrupt$/,
      handle: (m) => this.interrupt(m[1], m[2]),
    },
    { method: "POST", re: /^\/v1\/threads\/([\w-]+)\/fork$/, handle: (m) => this.fork(m[1]) },
    { method: "POST", re: /^\/v1\/threads\/([\w-]+)\/compact$/, handle: (m) => this.compact(m[1]) },
    { method: "GET", re: /^\/v1\/approvals$/, handle: () => this.listApprovals() },
    { method: "POST", re: /^\/v1\/approvals\/([\w-]+)$/, handle: (m, b) => this.decideApproval(m[1], b ?? {}) },
    { method: "POST", re: /^\/v1\/user-input\/([\w-]+)\/([\w-]+)$/, handle: (m, b) => this.answerInput(m[1], m[2], b ?? {}) },
    { method: "GET", re: /^\/v1\/workspace\/files\/search$/, handle: (_m, _b, p) => this.searchFiles(p) },
    { method: "GET", re: /^\/v1\/usage$/, handle: () => this.fx.body("GET /v1/usage", { totals: {}, buckets: [] }) },
    { method: "GET", re: /^\/v1\/sessions$/, handle: () => this.fx.body("GET /v1/sessions", { sessions: [] }) },
    {
      method: "POST",
      re: /^\/v1\/sessions\/([\w-]+)\/resume-thread$/,
      handle: (m) => {
        const tpl = this.fx.body<any>("POST /v1/sessions/{id}/resume-thread", {});
        const first = [...this.threads.keys()][0];
        if (!first) throw new HttpError(404, `Session not found: ${m[1]}`);
        return { ...tpl, thread_id: first, session_id: m[1] };
      },
    },
  ];

  private requireThread(id: string): MockThread {
    const t = this.threads.get(id);
    if (!t) throw new HttpError(404, this.fx.errorMessage("GET /v1/threads/{id}", 404, `Thread not found: ${id}`).replace(/thr_\w+/, id));
    return t;
  }

  private createThreadRecord(body: Record<string, any>): Record<string, any> {
    const now = new Date().toISOString();
    const tpl = this.fx.body<Record<string, any>>("POST /v1/threads");
    // Drop per-instance fields of the recorded thread.
    for (const k of ["title", "latest_turn_id", "session_id", "saved_session_checkpoint"]) delete tpl[k];
    return shaped(tpl, {
      id: recordId("thr"),
      created_at: now,
      updated_at: now,
      model: body.model,
      model_provider: body.model_provider ?? "ollama",
      model_provider_id: body.model_provider ?? "ollama",
      workspace: body.workspace ?? String(this.settings.workspace ?? "C:/Users/dev").replace(/\\/g, "/"),
      mode: body.mode ?? "agent",
      archived: false,
      ...(body.allowed_tools ? { allowed_tools: body.allowed_tools } : {}),
    });
  }

  private addThread(record: Record<string, any>): MockThread {
    const thread: MockThread = { record, turns: [], journal: [], seq: 0, subscribed: false };
    this.threads.set(record.id, thread);
    void this.journal(record.id, "thread.started", { thread: clone(record) });
    return thread;
  }

  private createThread(body: Record<string, any>) {
    if (!body.model) {
      throw new HttpError(
        400,
        this.fx.errorMessage(
          "POST /v1/threads",
          400,
          "Local Ollama has no fresh model catalog for this endpoint; select an explicit model or refresh its catalog.",
        ),
      );
    }
    const record = this.createThreadRecord(body);
    this.addThread(record);
    return record;
  }

  private summaryOf(t: MockThread): Record<string, any> {
    const tplList = this.fx.body<any[]>("GET /v1/threads/summary", []);
    const tpl = Array.isArray(tplList) && tplList[0] ? tplList[0] : undefined;
    const last = t.turns.at(-1);
    const firstPrompt = t.turns[0]?.input_summary as string | undefined;
    // Like the runtime: title = explicit title or the first prompt; preview = latest answer.
    const title = t.record.title ?? (firstPrompt ? firstPrompt.slice(0, 60) : "New Thread");
    const lastAnswer = [...t.journal]
      .reverse()
      .find((e) => e.event === "item.completed" && e.payload?.item?.kind === "agent_message")?.payload.item.detail;
    return shaped(tpl, {
      id: t.record.id,
      title,
      preview: lastAnswer ?? last?.input_summary ?? title,
      model: t.record.model,
      mode: t.record.mode,
      workspace: t.record.workspace,
      branch: this.branch,
      head: this.branch ? "4f2c1d0" : null,
      dirty: false,
      archived: !!t.record.archived,
      updated_at: t.record.updated_at,
      latest_turn_id: last?.id ?? null,
      latest_turn_status: last?.status ?? null,
      pending_attention_count: this.pendingApprovals(t.record.id).length,
    });
  }

  private listSummaries(path: string) {
    const q = new URL(path, "http://x").searchParams;
    const search = q.get("search")?.toLowerCase();
    const limit = Number(q.get("limit") ?? 50);
    return [...this.threads.values()]
      .map((t) => this.summaryOf(t))
      .filter((s) => !search || `${s.title} ${s.preview}`.toLowerCase().includes(search))
      .sort((a, b) => String(b.updated_at).localeCompare(String(a.updated_at)))
      .slice(0, limit);
  }

  private threadDetail(id: string) {
    const t = this.requireThread(id);
    const items = new Map<string, any>();
    for (const ev of t.journal) {
      const it = ev.payload?.item;
      if (it?.id) items.set(it.id, it);
    }
    return {
      thread: t.record,
      turns: t.turns,
      items: [...items.values()],
      latest_seq: t.seq,
      pending_approvals: this.pendingApprovals(id).map((a) => ({
        id: a.id,
        turn_id: a.turnId,
        tool_name: a.toolName,
        tool_call_id: a.toolCallId,
        description: a.description,
        intent_summary: a.intentSummary,
        summary: a.summary,
      })),
      pending_user_inputs: [...this.userInputs.entries()]
        .filter(([, u]) => u.threadId === id)
        .map(([inputId, u]) => ({ id: inputId, turn_id: u.turnId, request: u.request })),
      pending_dynamic_tool_calls: [],
      approval_grants: [],
    };
  }

  private patchThread(id: string, body: Record<string, any>) {
    const t = this.requireThread(id);
    for (const k of ["title", "model", "model_provider", "archived", "mode", "allowed_tools", "system_prompt"]) {
      if (k in body) t.record[k] = body[k];
    }
    t.record.updated_at = new Date().toISOString();
    void this.journal(id, "thread.updated", { thread: clone(t.record) });
    return t.record;
  }

  private context(id: string) {
    const t = this.requireThread(id);
    const tpl = this.fx.body<Record<string, any>>("GET /v1/threads/{id}/context");
    return shaped(tpl, { thread_id: id, model: t.record.model });
  }

  private createTurnRecord(t: MockThread, prompt: string, extra: Record<string, any> = {}): Record<string, any> {
    const now = new Date().toISOString();
    const tpl = this.fx.body<any>("POST /v1/threads/{id}/turns")?.turn;
    for (const k of ["ended_at", "duration_ms", "usage", "effective_route_usage", "error"]) delete tpl?.[k];
    const turn = shaped(tpl, {
      id: recordId("turn"),
      thread_id: t.record.id,
      status: "in_progress",
      input_summary: prompt,
      created_at: now,
      started_at: now,
      effective_model: t.record.model,
      item_ids: [],
      ...extra,
    });
    t.turns.push(turn);
    t.record.latest_turn_id = turn.id;
    return turn;
  }

  private startTurn(id: string, body: Record<string, any>) {
    const t = this.requireThread(id);
    if (t.activeTurn) throw new HttpError(409, `Thread ${id} already has an active turn`);
    const prompt = String(body.prompt ?? "");
    const turn = this.createTurnRecord(t, prompt);
    const script = this.turnQueue.shift() ?? this.defaultTurn;
    this.runTurn(t, turn, prompt, script);
    return { thread: clone(t.record), turn: clone(turn) };
  }

  private runTurn(t: MockThread, turn: Record<string, any>, prompt: string, script: TurnScript) {
    const ctx = new TurnCtx(this, t.record.id, turn.id, prompt, t.record.model);
    t.activeTurn = ctx;
    const run = (async () => {
      await new Promise((r) => setTimeout(r, this.turnStartDelayMs));
      try {
        await script(ctx);
        if (ACTIVE.has(turn.status)) await ctx.complete();
      } catch (e) {
        if (e instanceof TurnInterrupted) {
          await ctx.markInterrupted();
        } else {
          this.scriptErrors.push(e);
          if (ACTIVE.has(turn.status)) await ctx.fail(String(e instanceof Error ? e.message : e));
        }
      } finally {
        if (t.activeTurn === ctx) t.activeTurn = undefined;
      }
    })();
    this.runningScripts.add(run);
    void run.finally(() => this.runningScripts.delete(run));
  }

  private interrupt(id: string, turnId: string) {
    const t = this.requireThread(id);
    const ctx = t.activeTurn;
    if (!ctx || ctx.turnId !== turnId) {
      throw new HttpError(409, this.fx.errorMessage("POST /v1/threads/{id}/turns/{turn_id}/interrupt", 409, `No active turn on thread ${id}`).replace(/thr_\w+/, id));
    }
    ctx.requestInterrupt();
    void this.journal(id, "turn.interrupt_requested", { thread_id: id, turn_id: turnId }, { turnId });
    // Unblock a script parked on an approval or a gate.
    for (const [aid, a] of this.approvals) {
      if (a.turnId === turnId) {
        this.approvals.delete(aid);
        a.wait.resolve({ decision: "deny", remember: false });
      }
    }
    return { ok: true, thread_id: id, turn_id: turnId };
  }

  private fork(id: string) {
    const src = this.requireThread(id);
    const record = shaped(clone(src.record), { id: recordId("thr"), created_at: new Date().toISOString(), latest_turn_id: undefined });
    const thread = this.addThread(record);
    for (const turn of src.turns) thread.turns.push({ ...clone(turn), thread_id: record.id });
    return record;
  }

  private compact(id: string) {
    const t = this.requireThread(id);
    if (t.activeTurn) throw new HttpError(409, `Thread ${id} already has an active turn`);
    const turn = this.createTurnRecord(t, "Manual context compaction");
    this.runTurn(t, turn, "Manual context compaction", async (ctx) => {
      const rec = ctx.record;
      await ctx.emit("turn.started", { turn: { ...rec }, manual_compaction: true });
      await ctx.status("Сжатие контекста…");
      await ctx.complete({ input_tokens: 0, output_tokens: 0 });
    });
    return { thread: clone(t.record), turn: clone(turn) };
  }

  private listApprovals() {
    return this.pendingApprovals().map((a) => ({
      id: a.id,
      thread_id: a.threadId,
      turn_id: a.turnId,
      tool_name: a.toolName,
      tool_call_id: a.toolCallId,
      summary: a.summary,
      description: a.description,
    }));
  }

  private decideApproval(id: string, body: Record<string, any>) {
    const decision = String(body.decision ?? "");
    if (decision !== "allow" && decision !== "deny") {
      throw new HttpError(400, `invalid decision '${decision}', expected 'allow' or 'deny'`);
    }
    const a = this.approvals.get(id);
    if (!a) {
      throw new HttpError(404, this.fx.errorMessage("POST /v1/approvals/{approval_id}", 404, `no pending approval with id '${id}'`).replace(/'[^']*'/, `'${id}'`));
    }
    this.approvals.delete(id);
    a.wait.resolve({ decision, remember: !!body.remember });
    const tpl = this.fx.body<Record<string, any>>("POST /v1/approvals/{approval_id}", {});
    return shaped(tpl, { ok: true, approval_id: id, decision, delivered: true });
  }

  private answerInput(threadId: string, inputId: string, body: Record<string, any>) {
    const u = this.userInputs.get(inputId);
    if (!u || u.threadId !== threadId) throw new HttpError(404, `no pending user input with id '${inputId}'`);
    this.userInputs.delete(inputId);
    u.wait.resolve(body.answers ?? []);
    return { ok: true, input_id: inputId, delivered: true };
  }

  private searchFiles(path: string) {
    const q = (new URL(path, "http://x").searchParams.get("query") ?? ".").toLowerCase();
    const tpl = this.fx.body<{ paths: string[] }>("GET /v1/workspace/files/search", { paths: [] });
    const paths = (tpl.paths ?? []).filter((p) => q === "." || p.toLowerCase().includes(q));
    return { ...tpl, paths };
  }
}
