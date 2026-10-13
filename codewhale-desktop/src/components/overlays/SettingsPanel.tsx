import { useEffect, useState } from "react";
import { FolderOpen, Keyboard, RefreshCw, X, Download, Archive } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { api } from "../../lib/tauri";
import { APPROVABLE_TOOLS, LITE_TOOLS } from "../../lib/tools";
import { Button, Dot, IconButton, SectionTitle, Toggle } from "../ui/primitives";
import { effectiveCtx } from "../../lib/models";

const RISK_HINT: Record<string, string> = {
  safe: "безопасно",
  write: "меняет файлы",
  exec: "запускает команды",
  network: "выходит в сеть",
};

function Field({ label, children, hint }: { label: string; children: React.ReactNode; hint?: string }) {
  return (
    <label className="block py-1.5">
      <span className="mb-1 block text-[12px] text-fg2">{label}</span>
      {children}
      {hint && <span className="mt-0.5 block text-[11px] text-fg3">{hint}</span>}
    </label>
  );
}

const inputCls =
  "h-8 w-full rounded-[var(--radius-sm)] border border-line bg-base px-2 text-[13px] text-fg outline-none focus:border-blue";

export function SettingsPanel() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const setOverlay = useApp((s) => s.setOverlay);
  const ollama = useApp((s) => s.ollama);
  const refreshOllama = useApp((s) => s.refreshOllama);
  const models = useApp((s) => s.models);
  const runtime = useApp((s) => s.runtime);
  const setRuntime = useApp((s) => s.setRuntime);
  const summaries = useApp((s) => s.summaries);
  const archiveThread = useApp((s) => s.archiveThread);
  const [url, setUrl] = useState(settings?.ollamaUrl ?? "");
  const [cwPath, setCwPath] = useState(settings?.codewhalePath ?? "");
  const [checking, setChecking] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);

  useEffect(() => setUrl(settings?.ollamaUrl ?? ""), [settings?.ollamaUrl]);
  useEffect(() => setCwPath(settings?.codewhalePath ?? ""), [settings?.codewhalePath]);
  if (!settings) return null;

  const restartRuntime = async () => {
    try {
      setRuntime(await api.runtimeRestart());
      toast.success("CodeWhale перезапущен");
    } catch (e) {
      toast.error("Не удалось перезапустить", { description: String(e) });
    }
  };

  // The path is saved once editing is done (blur / Enter), not on every keystroke.
  const commitCwPath = () => {
    const next = cwPath.trim() || null;
    if (next !== (settings.codewhalePath ?? null)) update({ codewhalePath: next });
  };

  const exportAll = async () => {
    const path = await save({ title: "Экспорт всех сессий", defaultPath: "codewhale-sessions.json", filters: [{ name: "JSON", extensions: ["json"] }] });
    if (!path) return;
    try {
      const all = [];
      for (const s of summaries) {
        const detail = await api.request("GET", `/v1/threads/${s.id}`).catch(() => null);
        all.push({ summary: s, detail });
      }
      await api.writeExport(path, JSON.stringify(all, null, 2));
      toast.success(`Экспортировано сессий: ${all.length}`, { description: path });
    } catch (e) {
      toast.error("Экспорт не удался", { description: String(e) });
    }
  };

  return (
    <aside className="anim-msg-in absolute bottom-7 right-0 top-12 z-30 flex w-[380px] flex-col border-l border-line bg-elevated shadow-[var(--shadow-lg)]">
      <div className="flex h-11 items-center border-b border-line px-4">
        <span className="text-[15px] font-semibold">⚙ Настройки</span>
        <IconButton label="Закрыть (Esc)" className="ml-auto" onClick={() => setOverlay(null)}>
          <X size={16} />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <SectionTitle>Ollama</SectionTitle>
        <Field label="Адрес">
          <input
            className={inputCls}
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onBlur={() => url !== settings.ollamaUrl && update({ ollamaUrl: url.trim() || "http://127.0.0.1:11434" })}
          />
        </Field>
        <Field label="Модель по умолчанию">
          <select className={inputCls} value={settings.defaultModel ?? ""} onChange={(e) => useApp.getState().setModel(e.target.value)}>
            <option value="" disabled>
              Выберите модель
            </option>
            {models
              .filter((m) => !m.embedding)
              .map((m) => {
                const ctx = effectiveCtx(m);
                return (
                  <option key={m.name} value={m.name}>
                    {m.name}
                    {ctx ? ` · ${Math.round(ctx / 1024)}k` : " · 4k"}
                    {m.capabilities.includes("tools") ? "" : " · без инструментов"}
                  </option>
                );
              })}
          </select>
        </Field>
        <div className="flex items-center gap-2 py-1.5">
          <Button
            onClick={async () => {
              setChecking(true);
              await refreshOllama();
              setChecking(false);
            }}
          >
            <RefreshCw size={13} className={checking ? "animate-spin" : ""} /> Проверить подключение
          </Button>
          <span className="flex items-center gap-1.5 text-[12px]">
            <Dot color={ollama?.online ? "var(--accent-green)" : "var(--accent-red)"} />
            {ollama?.online ? `OK · v${ollama.version}` : "нет связи"}
          </span>
        </div>

        <SectionTitle>CodeWhale</SectionTitle>
        <div className="mb-1 flex items-center gap-2 text-[12px] text-fg2">
          <Dot color={runtime.state === "running" ? "var(--accent-green)" : runtime.state === "failed" ? "var(--accent-red)" : "var(--accent-yellow)"} />
          {runtime.state === "running" ? `Работает · v${runtime.version ?? "?"} · порт ${runtime.port}` : runtime.state}
        </div>
        <Field label="Путь к codewhale.exe" hint="Пусто — встроенная копия или установленная через npm">
          <div className="flex gap-1">
            <input
              className={inputCls}
              value={cwPath}
              placeholder="авто"
              onChange={(e) => setCwPath(e.target.value)}
              onBlur={commitCwPath}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
                // Esc closes the panel without a blur event: save first.
                else if (e.key === "Escape") commitCwPath();
              }}
            />
            <IconButton
              label="Выбрать файл"
              onClick={async () => {
                const p = await openDialog({ filters: [{ name: "codewhale", extensions: ["exe"] }] });
                if (typeof p === "string") update({ codewhalePath: p });
              }}
            >
              <FolderOpen size={14} />
            </IconButton>
          </div>
        </Field>
        <div className="py-1.5">
          <span className="mb-1 block text-[12px] text-fg2">Набор инструментов агента</span>
          <div className="flex gap-1">
            {(["lite", "full"] as const).map((p) => (
              <button
                key={p}
                onClick={() => update({ toolProfile: p })}
                className={`flex-1 rounded-[var(--radius-sm)] border px-2 py-1.5 text-left text-[12px] ${
                  settings.toolProfile === p ? "border-blue bg-[color-mix(in_srgb,var(--accent-blue)_12%,transparent)]" : "border-line hover:bg-hover"
                }`}
              >
                <span className="block font-medium text-fg">{p === "lite" ? "Облегчённый" : "Полный"}</span>
                <span className="block text-[11px] text-fg3">
                  {p === "lite" ? `Только ${LITE_TOOLS.join(", ")}` : "Все инструменты CodeWhale, промпт длиннее"}
                </span>
              </button>
            ))}
          </div>
          <span className="mt-1 block text-[11px] text-fg3">Для локальных моделей облегчённый набор точнее и оставляет больше места под ваш код.</span>
        </div>
        <Toggle
          checked={settings.telemetry}
          onChange={(v) => update({ telemetry: v })}
          label="Телеметрия CodeWhale"
          hint="Выключено: приложение запускает CodeWhale с CODEWHALE_TELEMETRY=0 (local-first)"
        />
        <Button className="mt-1" onClick={restartRuntime}>
          <RefreshCw size={13} /> Перезапустить CodeWhale
        </Button>

        <SectionTitle>Внешний вид</SectionTitle>
        <div className="flex gap-1 py-1.5">
          {(
            [
              ["light", "Светлая"],
              ["dark", "Тёмная"],
              ["auto", "Как в системе"],
            ] as const
          ).map(([v, l]) => (
            <button
              key={v}
              onClick={() => update({ theme: v })}
              className={`flex-1 rounded-[var(--radius-sm)] border px-2 py-1 text-[12px] ${
                settings.theme === v ? "border-blue text-fg" : "border-line text-fg2 hover:bg-hover"
              }`}
            >
              {l}
            </button>
          ))}
        </div>
        <div className="grid grid-cols-[1fr_88px] gap-2">
          <Field label="Моноширинный шрифт">
            <select className={inputCls} value={settings.fontMono} onChange={(e) => update({ fontMono: e.target.value })}>
              {["JetBrains Mono", "Cascadia Code", "Consolas", "Fira Code"].map((f) => (
                <option key={f}>{f}</option>
              ))}
            </select>
          </Field>
          <Field label="Размер">
            <select className={inputCls} value={settings.fontSize} onChange={(e) => update({ fontSize: Number(e.target.value) })}>
              {[12, 13, 14, 15, 16, 18].map((n) => (
                <option key={n} value={n}>
                  {n}px
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Field label="Плотность">
          <select className={inputCls} value={settings.density} onChange={(e) => update({ density: e.target.value as any })}>
            <option value="comfortable">Свободная</option>
            <option value="compact">Компактная</option>
          </select>
        </Field>

        <SectionTitle>Агент</SectionTitle>
        <Toggle checked={settings.showToolCalls} onChange={(v) => update({ showToolCalls: v })} label="Показывать вызовы инструментов" />
        <Toggle checked={settings.showThinking} onChange={(v) => update({ showThinking: v })} label="Показывать размышления модели" />
        <div className="mt-2 text-[12px] text-fg2">Автоподтверждение</div>
        <div className="text-[11px] text-fg3">Отмеченные вызовы подтверждаются сами. Остальные ждут вашего решения прямо в чате.</div>
        {APPROVABLE_TOOLS.map((t) => (
          <Toggle
            key={t.name}
            checked={!!settings.autoApprove[t.name]}
            onChange={(v) => update({ autoApprove: { ...settings.autoApprove, [t.name]: v } })}
            label={<span className="font-mono text-[12.5px]">{t.label}</span>}
            hint={RISK_HINT[t.risk]}
          />
        ))}

        <SectionTitle>Клавиатура</SectionTitle>
        <Button variant="ghost" className="-ml-2" onClick={() => setOverlay("shortcuts")}>
          <Keyboard size={13} /> Показать все горячие клавиши →
        </Button>

        <SectionTitle>Данные</SectionTitle>
        <div className="flex flex-col items-start gap-1.5 pb-4">
          <Button onClick={exportAll}>
            <Download size={13} /> Экспортировать все сессии
          </Button>
          {confirmClear ? (
            <span className="flex items-center gap-2 text-[12px]">
              <span className="text-red">Убрать все сессии в архив?</span>
              <button
                className="rounded bg-red px-2 py-0.5 text-white"
                onClick={async () => {
                  for (const s of summaries) await archiveThread(s.id);
                  setConfirmClear(false);
                }}
              >
                Да
              </button>
              <button className="rounded px-2 py-0.5 text-fg2 hover:bg-hover" onClick={() => setConfirmClear(false)}>
                Нет
              </button>
            </span>
          ) : (
            <Button variant="danger" onClick={() => setConfirmClear(true)}>
              <Archive size={13} /> Очистить историю
            </Button>
          )}
          <Button
            onClick={async () => {
              try {
                await revealItemInDir(await api.codewhaleHome());
              } catch (e) {
                toast.error("Не удалось открыть папку", { description: String(e) });
              }
            }}
          >
            <FolderOpen size={13} /> Открыть ~/.codewhale/
          </Button>
          {settings._file && <span className="text-[11px] text-fg3">Настройки приложения: {settings._file}</span>}
        </div>
      </div>
    </aside>
  );
}
