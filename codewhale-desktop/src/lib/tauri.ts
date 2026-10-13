import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettings,
  CwEventEnvelope,
  GpuStats,
  OllamaModel,
  OllamaStatus,
  RuntimeInfo,
} from "./types";

export const api = {
  settingsGet: () => invoke<AppSettings>("settings_get"),
  settingsSet: (patch: Partial<AppSettings>) => invoke<AppSettings>("settings_set", { patch }),

  runtimeStart: () => invoke<RuntimeInfo>("runtime_start"),
  runtimeRestart: () => invoke<RuntimeInfo>("runtime_restart"),
  runtimeStatus: () => invoke<RuntimeInfo>("runtime_status"),
  runtimeLogs: () => invoke<string[]>("runtime_logs"),

  request: <T = any>(method: string, path: string, body?: unknown) =>
    invoke<T>("cw_request", { method, path, body: body ?? null }),
  subscribe: (threadId: string, sinceSeq?: number | null) =>
    invoke<void>("cw_subscribe", { threadId, sinceSeq: sinceSeq ?? null }),
  unsubscribe: (threadId: string) => invoke<void>("cw_unsubscribe", { threadId }),

  ollamaStatus: () => invoke<OllamaStatus>("ollama_status"),
  ollamaModels: () => invoke<OllamaModel[]>("ollama_models"),
  ollamaUnload: (model: string) => invoke<void>("ollama_unload", { model }),
  ollamaCreateVariant: (from: string, name: string, numCtx: number) =>
    invoke<void>("ollama_create_variant", { from, name, numCtx }),

  gpuStats: () => invoke<GpuStats | null>("gpu_stats"),
  gitBranch: (path: string) => invoke<string | null>("git_branch", { path }),
  pathInfo: (path: string) => invoke<{ exists: boolean; isDir: boolean; name?: string }>("path_info", { path }),
  codewhaleHome: () => invoke<string>("codewhale_home_path"),
  openInEditor: (path: string, line?: number) => invoke<string>("open_in_editor", { path, line: line ?? null }),
  writeExport: (path: string, content: string) => invoke<void>("write_export", { path, content }),
  readImage: (path: string) => invoke<{ mime: string; dataBase64: string; name?: string }>("read_image_b64", { path }),
};

export function onCwEvent(cb: (e: CwEventEnvelope) => void): Promise<UnlistenFn> {
  return listen<CwEventEnvelope>("cw-event", (e) => cb(e.payload));
}

export function onCwStream(
  cb: (e: { threadId: string; state: string; message?: string | null }) => void,
): Promise<UnlistenFn> {
  return listen("cw-stream", (e) => cb(e.payload as any));
}

export function onRuntimeStatus(cb: (s: RuntimeInfo) => void): Promise<UnlistenFn> {
  return listen<RuntimeInfo>("runtime-status", (e) => cb(e.payload));
}

export function onRuntimeLog(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>("runtime-log", (e) => cb(e.payload));
}

/** Runs async listener registration safely under React StrictMode double-mount. */
export function subscribeAll(regs: Array<Promise<UnlistenFn>>): () => void {
  let disposed = false;
  const fns: UnlistenFn[] = [];
  for (const r of regs) {
    r.then((fn) => {
      if (disposed) fn();
      else fns.push(fn);
    });
  }
  return () => {
    disposed = true;
    fns.forEach((f) => f());
  };
}
