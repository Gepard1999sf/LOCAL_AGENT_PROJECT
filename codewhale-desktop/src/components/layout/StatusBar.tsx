import { Cpu, GitBranch, Wrench, Zap } from "lucide-react";
import { useApp } from "../../store/app";
import { threadActivity, toolLog, useThreads } from "../../store/threads";
import { fmtTokens } from "../../lib/format";
import { Dot, Tip } from "../ui/primitives";

const STATUS: Record<string, { label: string; color: string }> = {
  idle: { label: "Готов", color: "var(--accent-green)" },
  thinking: { label: "Думает", color: "var(--accent-yellow)" },
  tool: { label: "Инструмент", color: "var(--accent-blue)" },
  approval: { label: "Ждёт решения", color: "var(--accent-yellow)" },
  error: { label: "Ошибка", color: "var(--accent-red)" },
  offline: { label: "Ollama offline", color: "#8b949e" },
  starting: { label: "Запуск CodeWhale", color: "var(--accent-yellow)" },
  failed: { label: "CodeWhale не запущен", color: "var(--accent-red)" },
};

function Seg({ children, onClick, tip }: { children: React.ReactNode; onClick?: () => void; tip?: string }) {
  const el = (
    <button
      onClick={onClick}
      className="inline-flex h-full items-center gap-1.5 border-r border-line-muted px-2.5 text-fg2 transition-colors hover:bg-hover hover:text-fg disabled:cursor-default"
      disabled={!onClick}
    >
      {children}
    </button>
  );
  return tip ? <Tip label={tip} side="top">{el}</Tip> : el;
}

export function StatusBar() {
  const runtime = useApp((s) => s.runtime.state);
  const ollamaOnline = useApp((s) => s.ollama?.online);
  const activeId = useApp((s) => s.activeThreadId);
  const model = useApp((s) => s.settings?.defaultModel);
  const branch = useApp((s) => s.branch);
  const gpu = useApp((s) => s.gpu);
  const ctx = useApp((s) => (activeId ? s.contexts[activeId] : undefined));
  const setOverlay = useApp((s) => s.setOverlay);
  const setTab = useApp((s) => s.setInspectorTab);
  const thread = useThreads((s) => (activeId ? s.threads[activeId] : undefined));

  let key: string = threadActivity(thread);
  if (runtime === "starting") key = "starting";
  else if (runtime === "failed") key = "failed";
  else if (ollamaOnline === false) key = "offline";
  const st = STATUS[key];

  const lastTurn = thread?.turns.at(-1);
  const speed = [...(thread?.turns ?? [])].reverse().find((t) => t.tokPerSec)?.tokPerSec;
  const activeTools = toolLog(thread).filter((t) => t.status === "in_progress").length;
  const totalTools = toolLog(thread).length;
  const used = lastTurn?.usage?.input_tokens ?? ctx?.billed_input_tokens ?? ctx?.input_tokens ?? null;
  const windowTokens = ctx?.window_tokens ?? null;
  const pct = used && windowTokens ? Math.min(100, (used / windowTokens) * 100) : null;

  return (
    <footer className="flex h-7 shrink-0 items-stretch border-t border-line bg-elevated text-[12px]">
      <Seg onClick={() => setOverlay("logs")} tip="Журнал событий CodeWhale">
        <Dot color={st.color} pulse={key === "thinking" || key === "tool" || key === "starting"} />
        <span className="text-fg">{st.label}</span>
      </Seg>
      <Seg onClick={() => setOverlay("models")} tip="Сменить модель">
        <span className="font-mono">{model ?? "—"}</span>
      </Seg>
      <Seg onClick={() => setTab("context")} tip="Заполненность контекстного окна">
        <span>ctx:</span>
        <span className={pct != null && pct > 80 ? "text-red" : pct != null && pct > 60 ? "text-yellow" : ""}>
          {fmtTokens(used)} / {fmtTokens(windowTokens)}
        </span>
      </Seg>
      <Seg tip="Скорость генерации последнего ответа">
        <Zap size={12} /> {speed ? `${speed.toFixed(0)} ток/с` : "—"}
      </Seg>
      <Seg onClick={() => setTab("tools")} tip="Вызовы инструментов: активные / всего">
        <Wrench size={12} /> {activeTools > 0 ? <span className="text-bluefg">{activeTools}</span> : 0}
        <span className="text-fg3">/ {totalTools}</span>
      </Seg>
      {branch && (
        <Seg tip="Ветка git проекта">
          <GitBranch size={12} /> {branch}
        </Seg>
      )}
      <div className="flex-1" />
      {gpu && gpu.memTotalMb ? (
        <Seg tip={`${gpu.name}: ${gpu.util ?? 0}% загрузки, ${gpu.tempC ?? "?"}°C`}>
          <Cpu size={12} />
          <span className={gpu.memUsedMb! / gpu.memTotalMb > 0.92 ? "text-yellow" : ""}>
            VRAM {(gpu.memUsedMb! / 1024).toFixed(1)}/{(gpu.memTotalMb / 1024).toFixed(1)} ГБ
          </span>
        </Seg>
      ) : null}
    </footer>
  );
}
