import { create } from "zustand";
import { toast } from "sonner";
import { api } from "../lib/tauri";
import type {
  AppSettings,
  ContextInfo,
  GpuStats,
  OllamaModel,
  OllamaStatus,
  RuntimeInfo,
  SavedSession,
  ThreadRecord,
  ThreadSummary,
  TurnRecord,
  UserInputAnswer,
} from "../lib/types";
import { LITE_TOOLS } from "../lib/tools";
import { markStreamReplaying, useThreads } from "./threads";

export type Overlay = null | "settings" | "palette" | "shortcuts" | "about" | "logs" | "models" | "usage" | "drive";
export type InspectorTab = "tools" | "files" | "context";

interface AppState {
  runtime: RuntimeInfo;
  runtimeLogs: string[];
  ollama: OllamaStatus | null;
  models: OllamaModel[];
  gpu: GpuStats | null;
  settings: AppSettings | null;
  summaries: ThreadSummary[];
  sessions: SavedSession[];
  summariesLoaded: boolean;
  activeThreadId: string | null;
  contexts: Record<string, ContextInfo>;
  branch: string | null;

  sidebarCollapsed: boolean;
  inspectorCollapsed: boolean;
  inspectorTab: InspectorTab;
  overlay: Overlay;
  search: string;
  /** Thread ids in the order the sidebar lists them (Ctrl+1..9); null while it is not mounted. */
  sidebarOrder: string[] | null;
  composerFocusTick: number;
  windowWidth: number;

  setRuntime: (r: RuntimeInfo) => void;
  pushLog: (l: string) => void;
  loadSettings: () => Promise<void>;
  updateSettings: (patch: Partial<AppSettings>) => Promise<void>;
  refreshOllama: () => Promise<void>;
  refreshModels: () => Promise<void>;
  refreshGpu: () => Promise<void>;
  refreshThreads: () => Promise<void>;
  refreshContext: (threadId: string) => Promise<void>;
  refreshBranch: () => Promise<void>;
  openThread: (threadId: string | null) => Promise<void>;
  /** Drops the local copy of a thread and replays it from scratch (retry after a stream error). */
  reloadThread: (threadId: string) => Promise<void>;
  /** Closes event streams of background threads that have no running turn. */
  releaseIdleStreams: () => void;
  newThread: () => Promise<string | null>;
  sendPrompt: (prompt: string, images?: Array<{ mime: string; dataBase64: string }>) => Promise<void>;
  interrupt: () => Promise<void>;
  setModel: (model: string) => Promise<void>;
  renameThread: (threadId: string, title: string) => Promise<void>;
  archiveThread: (threadId: string) => Promise<void>;
  forkThread: (threadId: string) => Promise<void>;
  resumeSession: (sessionId: string) => Promise<void>;
  setWorkspace: (path: string) => Promise<void>;
  decideApproval: (threadId: string, approvalId: string, allow: boolean, remember?: boolean) => Promise<void>;
  answerUserInput: (threadId: string, inputId: string, answers: UserInputAnswer[]) => Promise<void>;
  setOverlay: (o: Overlay) => void;
  toggleSidebar: () => void;
  toggleInspector: () => void;
  setInspectorTab: (t: InspectorTab) => void;
  setSearch: (s: string) => void;
  setSidebarOrder: (ids: string[] | null) => void;
  focusComposer: () => void;
  setWindowWidth: (w: number) => void;
}

function errMsg(e: unknown): string {
  return typeof e === "string" ? e : (e as any)?.message ?? JSON.stringify(e);
}

/** cw_request reports HTTP failures as "HTTP <status>: <message>". */
function isNotFound(e: unknown): boolean {
  return errMsg(e).startsWith("HTTP 404");
}

/** Normalized workspace path for comparison (Windows paths are case-insensitive). */
function samePath(a: string | null | undefined, b: string | null | undefined): boolean {
  const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return !!a && !!b && norm(a) === norm(b);
}

// Event streams (Rust `cw_subscribe`) this window holds, by thread id. Only the active
// thread and threads with a running turn keep one; the rest are released.
const streams = new Map<string, "open" | "closed">();

async function openStream(threadId: string, fresh: boolean) {
  const threads = useThreads.getState();
  if (fresh || !threads.threads[threadId]) threads.reset(threadId);
  // Resume after the last applied event; duplicates are dropped by seq anyway.
  const since = useThreads.getState().threads[threadId]?.lastSeq ?? 0;
  markStreamReplaying(threadId);
  streams.set(threadId, "open");
  try {
    await api.subscribe(threadId, since);
  } catch (e) {
    streams.delete(threadId);
    throw e;
  }
}

function closeStream(threadId: string) {
  streams.set(threadId, "closed");
  api.unsubscribe(threadId).catch(() => {});
}

/** `latest_turn_status` values of a turn that is still working. The summary endpoint spells
 *  a running turn "inprogress" (lowercased Debug name); the serde form is accepted too. */
const ACTIVE_SUMMARY_STATUS = new Set(["queued", "inprogress", "in_progress"]);

export function summaryTurnActive(s: ThreadSummary): boolean {
  return ACTIVE_SUMMARY_STATUS.has(s.latest_turn_status ?? "");
}

function anyTurnRunning(summaries: ThreadSummary[]): boolean {
  const local = Object.values(useThreads.getState().threads).some((t) => !!t.activeTurnId);
  return local || summaries.some(summaryTurnActive);
}

export const useApp = create<AppState>((set, get) => ({
  runtime: { state: "stopped" },
  runtimeLogs: [],
  ollama: null,
  models: [],
  gpu: null,
  settings: null,
  summaries: [],
  sessions: [],
  summariesLoaded: false,
  activeThreadId: null,
  contexts: {},
  branch: null,

  sidebarCollapsed: false,
  inspectorCollapsed: window.innerWidth < 1400,
  inspectorTab: "tools",
  overlay: null,
  search: "",
  sidebarOrder: null,
  composerFocusTick: 0,
  windowWidth: window.innerWidth,

  setRuntime: (r) => set({ runtime: r }),
  pushLog: (l) => set((s) => ({ runtimeLogs: [...s.runtimeLogs.slice(-499), l] })),

  loadSettings: async () => {
    const settings = await api.settingsGet();
    set({ settings });
  },
  updateSettings: async (patch) => {
    try {
      const settings = await api.settingsSet(patch);
      set({ settings });
    } catch (e) {
      toast.error("Не удалось сохранить настройки", { description: errMsg(e) });
    }
  },

  refreshOllama: async () => {
    try {
      set({ ollama: await api.ollamaStatus() });
    } catch {
      set({ ollama: { online: false, url: get().settings?.ollamaUrl ?? "" } });
    }
  },
  refreshModels: async () => {
    try {
      set({ models: await api.ollamaModels() });
    } catch {
      /* offline: keep previous list */
    }
  },
  refreshGpu: async () => {
    set({ gpu: await api.gpuStats().catch(() => null) });
  },

  refreshThreads: async () => {
    if (get().runtime.state !== "running") return;
    try {
      const [summaries, sess] = await Promise.all([
        api.request<ThreadSummary[]>("GET", "/v1/threads/summary?limit=300"),
        api.request<{ sessions: SavedSession[] }>("GET", "/v1/sessions?limit=50").catch(() => ({ sessions: [] })),
      ]);
      set({
        summaries: (summaries ?? []).filter((s) => !s.archived),
        sessions: sess.sessions ?? [],
        summariesLoaded: true,
      });
    } catch (e) {
      set({ summariesLoaded: true });
      console.warn("threads", e);
    }
  },

  refreshContext: async (threadId) => {
    try {
      const ctx = await api.request<ContextInfo>("GET", `/v1/threads/${threadId}/context`);
      set((s) => ({ contexts: { ...s.contexts, [threadId]: ctx } }));
    } catch {
      /* ignore */
    }
  },

  refreshBranch: async () => {
    const ws = get().settings?.workspace;
    set({ branch: ws ? await api.gitBranch(ws).catch(() => null) : null });
  },

  openThread: async (threadId) => {
    const prev = get().activeThreadId;
    if (prev === threadId) return;
    set({ activeThreadId: threadId });
    get().releaseIdleStreams();
    if (!threadId) return;
    const existing = useThreads.getState().threads[threadId];
    // A thread seen before keeps its events: re-open its stream from the last seq.
    if (!existing || existing.streamState === "error" || streams.get(threadId) !== "open") {
      await openStream(threadId, !existing || existing.streamState === "error").catch((e) =>
        toast.error("Не удалось открыть сессию", { description: errMsg(e) }),
      );
    }
    get().refreshContext(threadId);
    get().focusComposer();
  },

  reloadThread: async (threadId) => {
    try {
      await openStream(threadId, true);
    } catch (e) {
      toast.error("Не удалось открыть сессию", { description: errMsg(e) });
    }
    get().refreshThreads();
  },

  releaseIdleStreams: () => {
    const active = get().activeThreadId;
    const threads = useThreads.getState().threads;
    // Also covers streams opened outside this store (they show up in the threads store).
    const ids = new Set([...streams.keys(), ...Object.keys(threads)]);
    for (const id of ids) {
      if (id === active || streams.get(id) === "closed" || threads[id]?.activeTurnId) continue;
      closeStream(id);
    }
  },

  newThread: async () => {
    const s = get().settings;
    const body: Record<string, unknown> = { model_provider: "ollama" };
    const model = s?.defaultModel;
    if (model) body.model = model;
    if (s?.workspace) body.workspace = s.workspace.replace(/\\/g, "/");
    if (s?.toolProfile !== "full") body.allowed_tools = LITE_TOOLS;
    try {
      const thread = await api.request<ThreadRecord>("POST", "/v1/threads", body);
      // Active first (as in openThread): a releaseIdleStreams() firing while the
      // subscribe is pending must not close the new thread's stream.
      set({ activeThreadId: thread.id });
      get().releaseIdleStreams();
      await openStream(thread.id, true);
      get().refreshThreads();
      get().refreshContext(thread.id);
      get().focusComposer();
      return thread.id;
    } catch (e) {
      toast.error("Не удалось создать сессию", { description: errMsg(e) });
      return null;
    }
  },

  sendPrompt: async (prompt, images) => {
    let tid = get().activeThreadId;
    if (!tid) tid = await get().newThread();
    if (!tid) return;
    const s = get().settings;
    const body: Record<string, unknown> = {
      prompt,
      operation_key: `desk-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    };
    if (s?.toolProfile !== "full") body.allowed_tools = LITE_TOOLS;
    if (images?.length) body.images = images;
    try {
      const res = await api.request<{ turn?: TurnRecord }>("POST", `/v1/threads/${tid}/turns`, body);
      // Mark the turn running right away so switching threads keeps its stream open.
      if (res?.turn) useThreads.getState().noteTurnStarted(tid, res.turn);
      if (streams.get(tid) !== "open") openStream(tid, false).catch(() => {});
      setTimeout(() => get().refreshThreads(), 400);
    } catch (e) {
      toast.error("Сообщение не отправлено", { description: errMsg(e) });
      throw e;
    }
  },

  interrupt: async () => {
    const tid = get().activeThreadId;
    if (!tid) return;
    const t = useThreads.getState().threads[tid];
    const turnId = t?.activeTurnId;
    if (!turnId) return;
    try {
      await api.request("POST", `/v1/threads/${tid}/turns/${turnId}/interrupt`, {});
    } catch (e) {
      toast.error("Не удалось остановить", { description: errMsg(e) });
    }
  },

  setModel: async (model) => {
    await get().updateSettings({ defaultModel: model });
    const tid = get().activeThreadId;
    if (tid) {
      try {
        await api.request("PATCH", `/v1/threads/${tid}`, { model, model_provider: "ollama" });
        get().refreshContext(tid);
        get().refreshThreads();
      } catch (e) {
        toast.error("Не удалось сменить модель", { description: errMsg(e) });
      }
    }
    toast.success(`Модель: ${model}`);
  },

  renameThread: async (threadId, title) => {
    try {
      await api.request("PATCH", `/v1/threads/${threadId}`, { title });
      get().refreshThreads();
    } catch (e) {
      toast.error("Не удалось переименовать", { description: errMsg(e) });
    }
  },

  archiveThread: async (threadId) => {
    try {
      await api.request("PATCH", `/v1/threads/${threadId}`, { archived: true });
      closeStream(threadId);
      if (get().activeThreadId === threadId) set({ activeThreadId: null });
      get().refreshThreads();
      toast.success("Сессия перемещена в архив");
    } catch (e) {
      toast.error("Не удалось удалить", { description: errMsg(e) });
    }
  },

  forkThread: async (threadId) => {
    try {
      const res = await api.request<any>("POST", `/v1/threads/${threadId}/fork`, {});
      const id = res?.thread?.id ?? res?.id;
      await get().refreshThreads();
      if (id) await get().openThread(id);
      toast.success("Сессия продублирована");
    } catch (e) {
      toast.error("Не удалось продублировать", { description: errMsg(e) });
    }
  },

  resumeSession: async (sessionId) => {
    try {
      const res = await api.request<any>("POST", `/v1/sessions/${sessionId}/resume-thread`, {});
      const id = res?.thread?.id ?? res?.id ?? res?.thread_id;
      await get().refreshThreads();
      if (id) await get().openThread(id);
    } catch (e) {
      toast.error("Не удалось открыть сессию терминала", { description: errMsg(e) });
    }
  },

  setWorkspace: async (path) => {
    const s = get().settings;
    if (samePath(path, s?.workspace)) return;
    const apply = async () => {
      const recent = [path, ...(get().settings?.recentWorkspaces ?? []).filter((p) => !samePath(p, path))].slice(0, 8);
      await get().updateSettings({ workspace: path, recentWorkspaces: recent });
      get().refreshBranch();
      await get().openThread(null);
      // File search (@-mentions) is rooted at the server's working directory.
      try {
        set({ runtime: await api.runtimeRestart() });
      } catch (e) {
        toast.error("CodeWhale не перезапустился", { description: errMsg(e) });
      }
      await get().refreshThreads();
      toast.success("Проект выбран", { description: path });
    };
    // The restart kills every running turn: ask first.
    if (anyTurnRunning(get().summaries)) {
      toast.warning("Агент ещё работает", {
        description: "Смена проекта перезапустит CodeWhale и прервёт текущую работу.",
        duration: 15000,
        action: { label: "Сменить проект", onClick: () => void apply() },
        cancel: { label: "Отмена", onClick: () => {} },
      });
      return;
    }
    await apply();
  },

  decideApproval: async (threadId, approvalId, allow, remember = false) => {
    const status = allow ? "allowed" : "denied";
    const prev = useThreads.getState().threads[threadId]?.approvals[approvalId]?.status;
    useThreads.getState().markApproval(threadId, approvalId, status);
    try {
      await api.request("POST", `/v1/approvals/${approvalId}`, { decision: allow ? "allow" : "deny", remember });
    } catch (e) {
      // 404: nothing is pending under this id any more; the approval.* event settles the card.
      const cur = useThreads.getState().threads[threadId]?.approvals[approvalId]?.status;
      if (!isNotFound(e) && prev && cur === status) useThreads.getState().markApproval(threadId, approvalId, prev);
      toast.error(isNotFound(e) ? "Запрос уже закрыт" : "Решение не отправлено", { description: errMsg(e) });
    }
  },

  answerUserInput: async (threadId, inputId, answers) => {
    const threads = useThreads.getState();
    const prev = threads.threads[threadId]?.userInputs[inputId]?.status;
    threads.markUserInput(threadId, inputId, "answered", answers);
    try {
      await api.request("POST", `/v1/user-input/${threadId}/${inputId}`, { answers });
    } catch (e) {
      const cur = useThreads.getState().threads[threadId]?.userInputs[inputId]?.status;
      if (!isNotFound(e) && prev && cur === "answered") useThreads.getState().markUserInput(threadId, inputId, prev);
      toast.error(isNotFound(e) ? "Вопрос уже закрыт" : "Ответ не отправлен", { description: errMsg(e) });
    }
  },

  setOverlay: (overlay) => set({ overlay }),
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
  toggleInspector: () => set((s) => ({ inspectorCollapsed: !s.inspectorCollapsed })),
  setInspectorTab: (inspectorTab) => set({ inspectorTab, inspectorCollapsed: false }),
  setSearch: (search) => set({ search }),
  setSidebarOrder: (sidebarOrder) => set({ sidebarOrder }),
  focusComposer: () => set((s) => ({ composerFocusTick: s.composerFocusTick + 1 })),
  setWindowWidth: (windowWidth) => set({ windowWidth }),
}));
