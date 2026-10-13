import { useHotkeys } from "react-hotkeys-hook";
import { toast } from "sonner";
import { useApp } from "../store/app";
import { useThreads } from "../store/threads";
import { exportThread } from "../lib/export";

const opts = { enableOnFormTags: true, preventDefault: true } as const;

export function useShortcuts() {
  useHotkeys("ctrl+n", () => useApp.getState().newThread(), opts);
  useHotkeys("ctrl+b", () => useApp.getState().toggleSidebar(), opts);
  useHotkeys("ctrl+j", () => useApp.getState().toggleInspector(), opts);
  useHotkeys("ctrl+comma", () => {
    const s = useApp.getState();
    s.setOverlay(s.overlay === "settings" ? null : "settings");
  }, opts);
  useHotkeys("ctrl+k", () => {
    const s = useApp.getState();
    s.setOverlay(s.overlay === "palette" ? null : "palette");
  }, opts);
  useHotkeys("ctrl+slash", () => {
    const s = useApp.getState();
    s.setOverlay(s.overlay === "shortcuts" ? null : "shortcuts");
  }, opts);
  useHotkeys("ctrl+f", () => window.dispatchEvent(new Event("sidebar:search")), opts);
  useHotkeys(
    "ctrl+1,ctrl+2,ctrl+3,ctrl+4,ctrl+5,ctrl+6,ctrl+7,ctrl+8,ctrl+9",
    (e) => {
      const n = Number(e.key);
      const s = useApp.getState();
      const target = s.summaries[n - 1];
      if (target) s.openThread(target.id);
    },
    opts,
  );
  useHotkeys(
    "escape",
    () => {
      const s = useApp.getState();
      if (s.overlay) {
        s.setOverlay(null);
        return;
      }
      const tid = s.activeThreadId;
      if (tid && useThreads.getState().threads[tid]?.activeTurnId) s.interrupt();
    },
    { enableOnFormTags: true },
  );
  useHotkeys(
    "ctrl+shift+c",
    () => {
      const blocks = document.querySelectorAll<HTMLElement>(".code-block[data-code]");
      const last = blocks[blocks.length - 1];
      if (last?.dataset.code) {
        navigator.clipboard.writeText(last.dataset.code);
        toast.success("Последний блок кода скопирован");
      } else toast("В сессии нет блоков кода");
    },
    opts,
  );
  useHotkeys("ctrl+e", () => {
    const tid = useApp.getState().activeThreadId;
    if (tid) exportThread(tid, "md");
  }, opts);
  useHotkeys("ctrl+d", () => {
    const tid = useApp.getState().activeThreadId;
    if (tid) useApp.getState().forkThread(tid);
  }, opts);
  useHotkeys("ctrl+w", () => {
    const s = useApp.getState();
    const tid = s.activeThreadId;
    if (!tid) return;
    if (useThreads.getState().threads[tid]?.activeTurnId) {
      toast("Агент ещё работает. Остановите его (Esc) перед закрытием сессии.");
      return;
    }
    s.openThread(null);
  }, opts);
}
