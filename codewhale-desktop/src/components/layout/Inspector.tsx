import { useEffect, useMemo, useState } from "react";
import { Tabs } from "radix-ui";
import { AlertTriangle, ChevronsRight, FileEdit, FileText, AtSign, Loader2, Check, X, Shrink, Wrench } from "lucide-react";
import { useApp } from "../../store/app";
import { toolLog, useThreads } from "../../store/threads";
import { toolMeta } from "../../lib/tools";
import { fmtBytes, fmtDuration, fmtTimeSec, fmtTokens } from "../../lib/format";
import { api } from "../../lib/tauri";
import { IconButton, Button } from "../ui/primitives";
import { toast } from "sonner";

function resolvePath(p: string) {
  const ws = useApp.getState().settings?.workspace ?? "";
  if (/^[A-Za-z]:[\\/]/.test(p) || !ws) return p;
  return `${ws.replace(/[\\/]+$/, "")}\\${p.replace(/\//g, "\\")}`;
}

function ToolsTab({ threadId }: { threadId: string | null }) {
  const thread = useThreads((s) => (threadId ? s.threads[threadId] : undefined));
  const log = useMemo(() => toolLog(thread), [thread]);
  const [open, setOpen] = useState<string | null>(null);
  const totalMs = log.reduce((acc, l) => (l.startedAt && l.endedAt ? acc + (new Date(l.endedAt).getTime() - new Date(l.startedAt).getTime()) : acc), 0);

  if (!log.length) return <div className="p-4 text-center text-[12px] text-fg3">Агент ещё не вызывал инструменты в этой сессии.</div>;
  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {log.map((l) => {
          const meta = toolMeta(l.name === "command_execution" ? "bash" : l.name);
          const Icon = meta.icon;
          const item = thread?.items[l.itemId];
          const isOpen = open === l.itemId;
          return (
            <div key={l.itemId} className="mb-1 rounded-[var(--radius-sm)] border border-transparent hover:border-line-muted">
              <button className="flex w-full items-start gap-2 px-2 py-1.5 text-left" onClick={() => setOpen(isOpen ? null : l.itemId)}>
                <Icon size={13} className="mt-0.5 shrink-0 text-fg2" />
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5 text-[12.5px]">
                    <span className="font-mono text-fg">{l.name}</span>
                    <span className="ml-auto flex items-center gap-1 text-[11px] text-fg3">
                      {l.status === "in_progress" ? (
                        <Loader2 size={11} className="animate-spin text-bluefg" />
                      ) : l.status === "failed" ? (
                        <X size={11} className="text-red" />
                      ) : (
                        <Check size={11} className="text-green" />
                      )}
                      {fmtTimeSec(l.startedAt)}
                    </span>
                  </span>
                  {l.arg && <span className="block truncate font-mono text-[11px] text-fg3">{l.arg}</span>}
                </span>
              </button>
              {l.status === "in_progress" && <div className="progress-indeterminate mx-2 mb-1 h-0.5 rounded bg-overlay" />}
              {isOpen && item && (
                <pre className="mx-2 mb-2 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-[var(--radius-sm)] bg-base p-2 font-mono text-[11px] text-fg2">
                  {item.toolArgs ? JSON.stringify(item.toolArgs, null, 2) + "\n\n" : ""}
                  {(item.text || item.detail || "").slice(0, 6000)}
                </pre>
              )}
            </div>
          );
        })}
      </div>
      <div className="border-t border-line px-3 py-1.5 text-[11px] text-fg3">
        Всего: {log.length} вызовов · {fmtDuration(totalMs) || "0 с"}
      </div>
    </div>
  );
}

function FileRow({ path, icon, extra }: { path: string; icon: React.ReactNode; extra?: string }) {
  return (
    <button
      className="flex w-full items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-left text-[12px] hover:bg-hover"
      title="Открыть в редакторе"
      onClick={() => api.openInEditor(resolvePath(path)).catch((e) => toast.error(String(e)))}
    >
      {icon}
      <span className="min-w-0 flex-1 truncate font-mono">{path}</span>
      {extra && <span className="text-[11px] text-fg3">{extra}</span>}
    </button>
  );
}

function FilesTab({ threadId }: { threadId: string | null }) {
  const thread = useThreads((s) => (threadId ? s.threads[threadId] : undefined));
  const mentioned = useMemo(() => {
    const set = new Set<string>();
    for (const t of thread?.turns ?? []) for (const m of t.prompt.matchAll(/@([^\s@]+)/g)) set.add(m[1]);
    return [...set];
  }, [thread?.turns]);
  const changed = Object.entries(thread?.filesChanged ?? {});
  const read = Object.keys(thread?.filesRead ?? {}).filter((p) => !thread?.filesChanged[p]);

  if (!changed.length && !read.length && !mentioned.length)
    return <div className="p-4 text-center text-[12px] text-fg3">Файлы появятся, когда агент начнёт их читать или менять.</div>;
  return (
    <div className="h-full overflow-y-auto p-2">
      {changed.length > 0 && (
        <>
          <div className="px-2 pb-1 pt-1 text-[11px] font-semibold uppercase tracking-wide text-fg3">Изменённые</div>
          {changed.map(([p, n]) => (
            <FileRow key={p} path={p} icon={<FileEdit size={13} className="text-yellow" />} extra={n > 1 ? `×${n}` : undefined} />
          ))}
        </>
      )}
      {read.length > 0 && (
        <>
          <div className="px-2 pb-1 pt-3 text-[11px] font-semibold uppercase tracking-wide text-fg3">Прочитанные</div>
          {read.map((p) => (
            <FileRow key={p} path={p} icon={<FileText size={13} className="text-fg3" />} />
          ))}
        </>
      )}
      {mentioned.length > 0 && (
        <>
          <div className="px-2 pb-1 pt-3 text-[11px] font-semibold uppercase tracking-wide text-fg3">Упомянутые</div>
          {mentioned.map((p) => (
            <FileRow key={p} path={p} icon={<AtSign size={13} className="text-fg3" />} />
          ))}
        </>
      )}
    </div>
  );
}

function Row({ label, value, warn }: { label: string; value: React.ReactNode; warn?: boolean }) {
  return (
    <div className="flex items-center justify-between py-1 text-[12px]">
      <span className="text-fg2">{label}</span>
      <span className={`font-mono ${warn ? "text-yellow" : "text-fg"}`}>{value}</span>
    </div>
  );
}

function ContextTab({ threadId }: { threadId: string | null }) {
  const ctx = useApp((s) => (threadId ? s.contexts[threadId] : undefined));
  const refreshContext = useApp((s) => s.refreshContext);
  const ollama = useApp((s) => s.ollama);
  const models = useApp((s) => s.models);
  const thread = useThreads((s) => (threadId ? s.threads[threadId] : undefined));
  const turns = thread?.turns ?? [];
  const lastUsage = turns.at(-1)?.usage?.input_tokens;
  const firstUsage = turns.find((t) => t.usage?.input_tokens)?.usage?.input_tokens;

  useEffect(() => {
    if (threadId) refreshContext(threadId);
  }, [threadId, turns.length, refreshContext]);

  if (!threadId) return <div className="p-4 text-center text-[12px] text-fg3">Откройте сессию, чтобы увидеть контекст.</div>;

  const used = lastUsage ?? ctx?.billed_input_tokens ?? ctx?.input_tokens ?? 0;
  const win = ctx?.window_tokens ?? 0;
  const pct = win ? Math.min(100, (used / win) * 100) : 0;
  const modelName = ctx?.model ?? thread?.thread?.model;
  const loaded = ollama?.loaded?.find((m) => m.name === modelName || m.model === modelName);
  const modelInfo = models.find((m) => m.name === modelName);
  const ollamaCtx = loaded?.context_length ?? modelInfo?.numCtx ?? null;
  const mismatch = win > 0 && ollamaCtx != null && ollamaCtx < win;
  const genMs = turns.reduce((a, t) => a + (t.durationMs ?? 0), 0);
  const outTok = turns.reduce((a, t) => a + t.outputTokens, 0);

  return (
    <div className="h-full overflow-y-auto p-3">
      <div className="mb-1 flex items-baseline justify-between text-[13px]">
        <span className="font-medium">Контекст</span>
        <span className="font-mono text-[12px]">
          {fmtTokens(used)} / {fmtTokens(win)} ({pct.toFixed(0)}%)
        </span>
      </div>
      <div className="mb-3 h-2 overflow-hidden rounded-full bg-overlay">
        <div
          className="h-full rounded-full transition-[width] duration-300"
          style={{ width: `${pct}%`, background: pct > 80 ? "var(--accent-red)" : pct > 60 ? "var(--accent-yellow)" : "var(--accent-green)" }}
        />
      </div>

      {mismatch && (
        <div className="mb-3 flex gap-2 rounded-[var(--radius-sm)] border border-[color-mix(in_srgb,var(--accent-yellow)_45%,transparent)] bg-[color-mix(in_srgb,var(--accent-yellow)_8%,transparent)] p-2 text-[11.5px] text-fg2">
          <AlertTriangle size={14} className="mt-0.5 shrink-0 text-yellow" />
          <span>
            Ollama загрузила модель с контекстом {fmtTokens(ollamaCtx)}, а CodeWhale рассчитывает на {fmtTokens(win)}. Начало запроса
            будет обрезано без предупреждения. Выберите модель с суффиксом <span className="font-mono">-32k</span> или задайте num_ctx в Modelfile.
          </span>
        </div>
      )}

      <Row label="Базовая часть (промпт + инструменты)" value={firstUsage ? `≈ ${fmtTokens(firstUsage)}` : "—"} />
      <Row label="История диалога" value={firstUsage && lastUsage ? `≈ ${fmtTokens(Math.max(0, lastUsage - firstUsage))}` : "—"} />
      <Row label="Резерв на ответ" value={fmtTokens(ctx?.output_cap_tokens)} />
      <Row label="Сжатие истории начнётся с" value={fmtTokens(ctx?.compaction_trigger_tokens)} />
      <Row label="Давление на контекст" value={ctx?.pressure ?? "—"} warn={ctx?.pressure === "high"} />

      <div className="my-3 h-px bg-line" />
      <div className="mb-1 text-[12px] font-medium">Модель в Ollama</div>
      <Row label="Модель" value={modelName ?? "—"} />
      <Row label="Контекст Ollama (num_ctx)" value={ollamaCtx ? fmtTokens(ollamaCtx) : "4k (по умолчанию)"} warn={!ollamaCtx || ollamaCtx < 16384} />
      <Row label="В видеопамяти" value={loaded ? `${fmtBytes(loaded.size_vram)} из ${fmtBytes(loaded.size)}` : "не загружена"} warn={!!loaded && loaded.size_vram < loaded.size} />
      {loaded && loaded.size_vram < loaded.size && (
        <div className="text-[11px] text-yellow">Часть модели работает на процессоре, генерация медленнее.</div>
      )}

      <div className="my-3 h-px bg-line" />
      <Row label="💰 Стоимость (локально)" value="0.00 ₽" />
      <Row label="Время генерации" value={fmtDuration(genMs) || "—"} />
      <Row label="Сгенерировано токенов" value={fmtTokens(outTok)} />

      <Button
        className="mt-3 w-full"
        onClick={() =>
          api.request("POST", `/v1/threads/${threadId}/compact`, {}).then(
            () => toast.success("Сжатие контекста запущено"),
            (e) => toast.error("Не удалось сжать", { description: String(e) }),
          )
        }
      >
        <Shrink size={13} /> Сжать историю сейчас
      </Button>
    </div>
  );
}

export function Inspector() {
  const collapsed = useApp((s) => s.inspectorCollapsed);
  const tab = useApp((s) => s.inspectorTab);
  const setTab = useApp((s) => s.setInspectorTab);
  const toggle = useApp((s) => s.toggleInspector);
  const threadId = useApp((s) => s.activeThreadId);
  const thread = useThreads((s) => (threadId ? s.threads[threadId] : undefined));
  const activeTools = useMemo(() => toolLog(thread).filter((t) => t.status === "in_progress").length, [thread]);
  const changed = Object.keys(thread?.filesChanged ?? {}).length;

  if (collapsed) {
    return (
      <aside className="flex w-8 shrink-0 flex-col items-center gap-2 border-l border-line bg-elevated py-2 text-[10px] text-fg3">
        <button title="Инструменты" onClick={() => setTab("tools")} className={`flex flex-col items-center ${activeTools ? "text-bluefg" : ""}`}>
          <Wrench size={13} />
          {activeTools > 0 && activeTools}
        </button>
        <button title="Изменённые файлы" onClick={() => setTab("files")} className={`flex flex-col items-center ${changed ? "text-yellow" : ""}`}>
          <FileEdit size={13} />
          {changed > 0 && changed}
        </button>
        {thread?.activeTurnId && <div className="progress-indeterminate h-10 w-1 rounded bg-overlay" />}
      </aside>
    );
  }

  return (
    <aside className="panel-transition flex w-[320px] shrink-0 flex-col border-l border-line bg-elevated">
      <Tabs.Root value={tab} onValueChange={(v) => setTab(v as any)} className="flex min-h-0 flex-1 flex-col">
        <div className="flex h-10 items-center border-b border-line pl-2 pr-1">
          <Tabs.List className="flex gap-0.5">
            {(
              [
                ["tools", "Инструменты"],
                ["files", "Файлы"],
                ["context", "Контекст"],
              ] as const
            ).map(([v, l]) => (
              <Tabs.Trigger
                key={v}
                value={v}
                className="rounded-[var(--radius-sm)] px-2.5 py-1 text-[12.5px] text-fg2 hover:text-fg data-[state=active]:bg-hover data-[state=active]:text-fg"
              >
                {l}
                {v === "tools" && activeTools > 0 && <span className="ml-1 text-bluefg">{activeTools}</span>}
              </Tabs.Trigger>
            ))}
          </Tabs.List>
          <IconButton size="sm" label="Свернуть (Ctrl+J)" className="ml-auto" onClick={toggle}>
            <ChevronsRight size={14} />
          </IconButton>
        </div>
        <Tabs.Content value="tools" className="min-h-0 flex-1">
          <ToolsTab threadId={threadId} />
        </Tabs.Content>
        <Tabs.Content value="files" className="min-h-0 flex-1">
          <FilesTab threadId={threadId} />
        </Tabs.Content>
        <Tabs.Content value="context" className="min-h-0 flex-1">
          <ContextTab threadId={threadId} />
        </Tabs.Content>
      </Tabs.Root>
    </aside>
  );
}
