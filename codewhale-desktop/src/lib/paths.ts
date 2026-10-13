import { useApp } from "../store/app";
import { useThreads } from "../store/threads";

const isSep = (c: string | undefined) => c === "\\" || c === "/";
// Windows paths: either separator, case-insensitive.
const norm = (p: string) => p.replace(/\//g, "\\").toLowerCase();

/** Workspace of a thread (its tool paths are relative to it), else the selected project. */
export function threadWorkspace(threadId: string | null | undefined): string {
  const app = useApp.getState();
  const ws = threadId
    ? (useThreads.getState().threads[threadId]?.thread?.workspace ?? app.summaries.find((s) => s.id === threadId)?.workspace)
    : undefined;
  return ws || app.settings?.workspace || "";
}

/** Absolute path for `p`; relative paths are resolved against `ws` (default: the active thread's workspace). */
export function resolvePath(p: string, ws = threadWorkspace(useApp.getState().activeThreadId)): string {
  if (/^[A-Za-z]:[\\/]/.test(p) || /^[\\/]{2}/.test(p) || !ws) return p;
  return `${ws.replace(/\//g, "\\").replace(/\\+$/, "")}\\${p.replace(/\//g, "\\")}`;
}

/** `path` relative to `ws` with "/" separators, or null when it is not inside `ws`. */
export function relativeTo(path: string, ws: string | null | undefined): string | null {
  const root = (ws ?? "").replace(/[\\/]+$/, "");
  if (!root || !isSep(path[root.length]) || norm(path.slice(0, root.length)) !== norm(root)) return null;
  return path.slice(root.length + 1).replace(/\\/g, "/");
}
