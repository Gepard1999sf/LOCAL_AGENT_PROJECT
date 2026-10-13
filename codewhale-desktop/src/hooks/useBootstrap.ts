import { useEffect } from "react";
import { api, onCwEvent, onCwStream, onRuntimeLog, onRuntimeStatus, subscribeAll } from "../lib/tauri";
import { useApp } from "../store/app";
import { enqueueEvent, useThreads } from "../store/threads";
import type { CwEventEnvelope } from "../lib/types";

const PREFERRED_MODELS = ["qwen3-4b-32k", "qwen3.5-4b-64k", "qwen3-8b-16k"];

function onEvent(env: CwEventEnvelope) {
  enqueueEvent(env);
  const app = useApp.getState();
  switch (env.event) {
    case "approval.required": {
      // Auto-approve only live requests (never replayed history) and only tools the user allowed.
      setTimeout(() => {
        const t = useThreads.getState().threads[env.threadId];
        if (!t?.live) return;
        let p: any;
        try {
          p = JSON.parse(env.data).payload;
        } catch {
          return;
        }
        const tool: string | undefined = p?.tool_name;
        const id: string | undefined = p?.approval_id ?? p?.id;
        const rules = useApp.getState().settings?.autoApprove ?? {};
        if (tool && id && rules[tool] && t.approvals[id]?.status === "pending") {
          app.decideApproval(env.threadId, id, true);
        }
      }, 60);
      break;
    }
    case "turn.completed":
    case "turn.failed":
    case "turn.interrupted":
      app.refreshThreads();
      app.refreshContext(env.threadId);
      break;
    case "thread.updated":
      app.refreshThreads();
      break;
  }
}

function applyTheme() {
  const s = useApp.getState().settings;
  const root = document.documentElement;
  const theme = s?.theme ?? "dark";
  const resolved = theme === "auto" ? (window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark") : theme;
  root.dataset.theme = resolved;
  root.style.setProperty(
    "--font-mono",
    `"${s?.fontMono ?? "JetBrains Mono"}", "JetBrains Mono", "Cascadia Code", "Cascadia Mono", Consolas, monospace`,
  );
  root.style.setProperty("--chat-font-size", `${s?.fontSize ?? 14}px`);
  root.dataset.density = s?.density ?? "comfortable";
}

function applyResponsive(width: number, prev: number) {
  const app = useApp.getState();
  const crossed = (t: number) => (prev > t) !== (width > t);
  if (crossed(1400)) useApp.setState({ inspectorCollapsed: width <= 1400 });
  if (crossed(1000)) useApp.setState({ sidebarCollapsed: width <= 1000 });
  if (width < 700 && !app.inspectorCollapsed) useApp.setState({ inspectorCollapsed: true });
  app.setWindowWidth(width);
}

export function useBootstrap() {
  useEffect(() => {
    const app = useApp.getState();
    let disposed = false;

    const unlisten = subscribeAll([
      onRuntimeStatus((r) => {
        const was = useApp.getState().runtime.state;
        useApp.getState().setRuntime(r);
        if (r.state === "running" && was !== "running") useApp.getState().refreshThreads();
      }),
      onRuntimeLog((l) => useApp.getState().pushLog(l)),
      onCwEvent(onEvent),
      onCwStream((s) => useThreads.getState().setStream(s.threadId, s.state, s.message)),
    ]);

    (async () => {
      await app.loadSettings().catch(() => {});
      applyTheme();
      const r = await api.runtimeStatus().catch(() => null);
      if (r && !disposed) {
        app.setRuntime(r);
        if (r.state === "running") app.refreshThreads();
        if (r.state === "stopped") api.runtimeStart().then(app.setRuntime).catch(() => {});
      }
      api.runtimeLogs().then((logs) => useApp.setState({ runtimeLogs: logs })).catch(() => {});
      await app.refreshOllama();
      await app.refreshModels();
      app.refreshGpu();
      app.refreshBranch();
      // Pick an agent-ready model on first run.
      const st = useApp.getState();
      if (!st.settings?.defaultModel || !st.models.some((m) => m.name === st.settings?.defaultModel)) {
        const pick =
          PREFERRED_MODELS.find((n) => st.models.some((m) => m.name === n)) ??
          st.models.find((m) => m.capabilities.includes("tools") && (m.numCtx ?? 0) >= 16384)?.name ??
          st.models.find((m) => m.capabilities.includes("tools"))?.name;
        if (pick) await st.updateSettings({ defaultModel: pick });
      }
    })();

    const poll = setInterval(() => {
      const s = useApp.getState();
      const wasOnline = s.ollama?.online;
      s.refreshOllama().then(() => {
        const now = useApp.getState().ollama?.online;
        if (now && !wasOnline) useApp.getState().refreshModels();
      });
      if (document.visibilityState === "visible") s.refreshGpu();
    }, 5000);

    const ctxPoll = setInterval(() => {
      const s = useApp.getState();
      if (s.activeThreadId && document.visibilityState === "visible") s.refreshContext(s.activeThreadId);
    }, 15000);

    let lastWidth = window.innerWidth;
    applyResponsive(lastWidth, lastWidth > 1400 ? 0 : 99999);
    const onResize = () => {
      const w = window.innerWidth;
      applyResponsive(w, lastWidth);
      lastWidth = w;
    };
    window.addEventListener("resize", onResize);

    const mq = window.matchMedia("(prefers-color-scheme: light)");
    mq.addEventListener("change", applyTheme);
    const unsubTheme = useApp.subscribe((s, p) => {
      if (s.settings !== p.settings) applyTheme();
      if (s.settings?.workspace !== p.settings?.workspace) s.refreshBranch();
    });

    return () => {
      disposed = true;
      unlisten();
      clearInterval(poll);
      clearInterval(ctxPoll);
      window.removeEventListener("resize", onResize);
      mq.removeEventListener("change", applyTheme);
      unsubTheme();
    };
  }, []);
}
