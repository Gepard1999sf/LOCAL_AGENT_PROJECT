import { useEffect, useRef, useState } from "react";
import { Copy, X } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { api } from "../../lib/tauri";
import { SHORTCUTS } from "../../lib/shortcuts";
import { fmtTokens } from "../../lib/format";
import { Button, IconButton } from "../ui/primitives";
import { ModelManager } from "../models/ModelManager";

function openLink(url: string) {
  openUrl(url).catch((e) => toast.error("Не удалось открыть ссылку", { description: String(e) }));
}

export function Panel({ title, children, width = 400 }: { title: React.ReactNode; children: React.ReactNode; width?: number }) {
  const setOverlay = useApp((s) => s.setOverlay);
  return (
    <aside
      className="anim-msg-in absolute bottom-7 right-0 top-12 z-30 flex flex-col border-l border-line bg-elevated shadow-[var(--shadow-lg)]"
      style={{ width }}
    >
      <div className="flex h-11 shrink-0 items-center border-b border-line px-4">
        <span className="text-[15px] font-semibold">{title}</span>
        <IconButton label="Закрыть (Esc)" className="ml-auto" onClick={() => setOverlay(null)}>
          <X size={16} />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
    </aside>
  );
}

export function ShortcutsPanel() {
  return (
    <Panel title="⌨ Горячие клавиши" width={420}>
      <div className="px-4 py-3">
        {SHORTCUTS.map((g) => (
          <div key={g.group} className="mb-4">
            <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-fg3">{g.group}</div>
            {g.items.map(([k, d]) => (
              <div key={k} className="flex items-center justify-between py-1 text-[13px]">
                <span className="text-fg2">{d}</span>
                <span className="flex gap-1">
                  {k.split(" / ").map((part) => (
                    <kbd key={part}>{part}</kbd>
                  ))}
                </span>
              </div>
            ))}
          </div>
        ))}
      </div>
    </Panel>
  );
}

export function AboutPanel() {
  const runtime = useApp((s) => s.runtime);
  const ollama = useApp((s) => s.ollama);
  const gpu = useApp((s) => s.gpu);
  return (
    <Panel title="🐋 О приложении">
      <div className="space-y-3 px-4 py-4 text-[13px]">
        <div>
          <div className="text-[18px] font-semibold">CodeWhale Desktop 0.1.0</div>
          <div className="text-fg2">Десктопный клиент агента CodeWhale. Работает полностью локально: модели в Ollama, данные в ~/.codewhale.</div>
        </div>
        <div className="space-y-1 rounded-[var(--radius-md)] border border-line p-3 font-mono text-[12px]">
          <div>CodeWhale: {runtime.version ?? "—"} ({runtime.state})</div>
          <div className="truncate" title={runtime.binary ?? ""}>Бинарник: {runtime.binary ?? "—"}</div>
          <div>Ollama: {ollama?.online ? `v${ollama.version}` : "offline"}</div>
          {gpu && <div>GPU: {gpu.name}, {((gpu.memTotalMb ?? 0) / 1024).toFixed(0)} ГБ</div>}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button onClick={() => openLink("https://github.com/codewhale-hq/Codewhale")}>CodeWhale на GitHub</Button>
          <Button onClick={() => openLink("https://docs.ollama.com")}>Документация Ollama</Button>
        </div>
      </div>
    </Panel>
  );
}

export function LogsPanel() {
  const logs = useApp((s) => s.runtimeLogs);
  const endRef = useRef<HTMLDivElement>(null);
  const [initial, setInitial] = useState<string[]>([]);
  useEffect(() => {
    api.runtimeLogs().then(setInitial).catch(() => {});
  }, []);
  const all = logs.length >= initial.length ? logs : initial;
  useEffect(() => endRef.current?.scrollIntoView(), [all.length]);
  return (
    <Panel title="Журнал CodeWhale" width={560}>
      <div className="flex justify-end gap-2 border-b border-line-muted px-3 py-1.5">
        <Button variant="ghost" onClick={() => navigator.clipboard.writeText(all.join("\n"))}>
          <Copy size={13} /> Копировать
        </Button>
      </div>
      <pre className="whitespace-pre-wrap break-words px-3 py-2 font-mono text-[11px] leading-[1.5] text-fg2">
        {all.length ? all.join("\n") : "Журнал пуст"}
        <div ref={endRef} />
      </pre>
    </Panel>
  );
}

export function UsagePanel() {
  const [data, setData] = useState<any>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    api.request("GET", "/v1/usage?group_by=day").then(setData, (e) => setErr(String(e)));
  }, []);
  const totals = data?.totals;
  return (
    <Panel title="📊 Использование">
      <div className="px-4 py-3 text-[13px]">
        {err && <div className="text-red">{err}</div>}
        {!data && !err && <div className="h-20 skeleton" />}
        {totals && (
          <>
            <div className="mb-3 grid grid-cols-2 gap-2">
              {[
                ["Ходов", totals.turns],
                ["Входных токенов", fmtTokens(totals.input_tokens)],
                ["Выходных токенов", fmtTokens(totals.output_tokens)],
                ["Из кэша промпта", fmtTokens(totals.cached_tokens)],
              ].map(([l, v]) => (
                <div key={l as string} className="rounded-[var(--radius-md)] border border-line p-2.5">
                  <div className="text-[11px] text-fg3">{l}</div>
                  <div className="text-[18px] font-semibold">{v}</div>
                </div>
              ))}
            </div>
            <div className="mb-1 text-[11px] uppercase tracking-wide text-fg3">По дням</div>
            <table className="w-full text-[12px]">
              <thead className="text-fg3">
                <tr>
                  <th className="py-1 text-left font-normal">День</th>
                  <th className="text-right font-normal">Ходов</th>
                  <th className="text-right font-normal">Вход</th>
                  <th className="text-right font-normal">Выход</th>
                </tr>
              </thead>
              <tbody>
                {(data.buckets ?? [])
                  .slice()
                  .reverse()
                  .map((b: any) => (
                    <tr key={b.key} className="border-t border-line-muted">
                      <td className="py-1">{b.key}</td>
                      <td className="text-right">{b.turns}</td>
                      <td className="text-right font-mono">{fmtTokens(b.input_tokens)}</td>
                      <td className="text-right font-mono">{fmtTokens(b.output_tokens)}</td>
                    </tr>
                  ))}
              </tbody>
            </table>
            <div className="mt-3 text-[11px] text-fg3">Локальные модели бесплатны: стоимость 0 ₽.</div>
          </>
        )}
      </div>
    </Panel>
  );
}

export function ModelsPanel() {
  return (
    <Panel title="Модели Ollama" width={520}>
      <ModelManager />
    </Panel>
  );
}
