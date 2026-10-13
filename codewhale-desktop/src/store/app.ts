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
} from "../lib/types";
import { LITE_TOOLS } from "../lib/tools";
import { useThreads } from "./threads";

export type Overlay = null | "settings" | "palette" | "shortcuts" | "about" | "logs" | "models" | "usage";
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
  newThread: (opts?: { prompt?: string }) => Promise<string | null>;
  sendPrompt: (prompt: string, images?: Array<{ mime: string; dataBase64: string }>) => Promise<void>;
  interrupt: () => Promise<void>;
  setModel: (model: string) => Promise<void>;
  renameThread: (threadId: string, title: string) => Promise<void>;
  archiveThread: (threadId: string) => Promise<void>;
  forkThread: (threadId: string) => Promise<void>;
  resumeSession: (sessionId: string) => Promise<void>;
  setWorkspace: (path: string) => Promise<void>;
  decideApproval: (threadId: string, approvalId: string, allow: boolean, remember?: boolean) => Promise<void>;
  setOverlay: (o: Overlay) => void;
  toggleSidebar: () => void;
  toggleInspector: () => void;
  setInspectorTab: (t: InspectorTab) => void;
  setSearch: (s: string) => void;
  focusComposer: () => void;
  setWindowWidth: (w: number) => void;
}

function errMsg(e: unknown): string {
  return typeof e === "string" ? e : (e as any)?.message ?? JSON.stringify(e);
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
    if (!threadId) return;
    const existing = useThreads.getState().threads[threadId];
    if (!existing) {
      useThreads.getState().reset(threadId);
      await api.subscribe(threadId, 0).catch((e) => toast.error("Не удалось открыть сессию", { description: errMsg(e) }));
    }
    get().refreshContext(threadId);
    get().focusComposer();
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
      useThreads.getState().reset(thread.id);
      await api.subscribe(thread.id, 0);
      set({ activeThreadId: thread.id });
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
      await api.request("POST", `/v1/threads/${tid}/turns`, body);
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
      await api.unsubscribe(threadId);
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
    const recent = [path, ...(s?.recentWorkspaces ?? []).filter((p) => p !== path)].slice(0, 8);
    await get().updateSettings({ workspace: path, recentWorkspaces: recent });
    get().refreshBranch();
    set({ activeThreadId: null });
    // File search (@-mentions) is rooted at the server's working directory.
    try {
      set({ runtime: await api.runtimeRestart() });
    } catch (e) {
      toast.error("CodeWhale не перезапустился", { description: errMsg(e) });
    }
    await get().refreshThreads();
    toast.success("Проект выбран", { description: path });
  },

  decideApproval: async (threadId, approvalId, allow, remember = false) => {
    useThreads.getState().markApproval(threadId, approvalId, allow ? "allowed" : "denied");
    try {
      await api.request("POST", `/v1/approvals/${approvalId}`, { decision: allow ? "allow" : "deny", remember });
    } catch (e) {
      toast.error("Решение не отправлено", { description: errMsg(e) });
    }
  },

  setOverlay: (overlay) => set({ overlay }),
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
  toggleInspector: () => set((s) => ({ inspectorCollapsed: !s.inspectorCollapsed })),
  setInspectorTab: (inspectorTab) => set({ inspectorTab, inspectorCollapsed: false }),
  setSearch: (search) => set({ search }),
  focusComposer: () => set((s) => ({ composerFocusTick: s.composerFocusTick + 1 })),
  setWindowWidth: (windowWidth) => set({ windowWidth }),
}));
