import { Command } from "cmdk";
import {
  BarChart3,
  Cpu,
  Download,
  FolderOpen,
  Keyboard,
  MessageSquare,
  Moon,
  PanelLeft,
  PanelRight,
  Plus,
  RefreshCw,
  ScrollText,
  Settings,
  Shrink,
  Sun,
  FileText,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { api } from "../../lib/tauri";
import { exportThread } from "../../lib/export";

const EMPTY: string[] = [];

function Item({ icon, label, shortcut, onSelect, value }: { icon: React.ReactNode; label: string; shortcut?: string; onSelect: () => void; value?: string }) {
  return (
    <Command.Item
      value={value ?? label}
      onSelect={onSelect}
      className="flex cursor-pointer items-center gap-2.5 rounded-[var(--radius-sm)] px-3 py-2 text-[13px] text-fg"
    >
      <span className="text-fg2">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {shortcut && <span className="text-[11px] text-fg3">{shortcut}</span>}
    </Command.Item>
  );
}

export function CommandPalette() {
  const setOverlay = useApp((s) => s.setOverlay);
  const newThread = useApp((s) => s.newThread);
  const openThread = useApp((s) => s.openThread);
  const summaries = useApp((s) => s.summaries);
  const models = useApp((s) => s.models);
  const setModel = useApp((s) => s.setModel);
  const activeId = useApp((s) => s.activeThreadId);
  const update = useApp((s) => s.updateSettings);
  const theme = useApp((s) => s.settings?.theme);
  const setWorkspace = useApp((s) => s.setWorkspace);
  const recent = useApp((s) => s.settings?.recentWorkspaces) ?? EMPTY;

  const run = (fn: () => void) => () => {
    setOverlay(null);
    fn();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/30 pt-[12vh]" onMouseDown={() => setOverlay(null)}>
      <Command
        label="Палитра команд"
        className="anim-msg-in w-[600px] max-w-[92vw] overflow-hidden rounded-[var(--radius-lg)] border border-line bg-overlay shadow-[var(--shadow-lg)]"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && setOverlay(null)}
      >
        <Command.Input
          autoFocus
          placeholder="Введите команду или название сессии…"
          className="h-12 w-full border-b border-line bg-transparent px-4 text-[14px] text-fg outline-none placeholder:text-fg3"
        />
        <Command.List className="max-h-[420px] overflow-y-auto p-1.5">
          <Command.Empty className="px-3 py-6 text-center text-[13px] text-fg3">Ничего не найдено</Command.Empty>
          <Command.Group heading="Действия">
            <Item icon={<Plus size={14} />} label="Новая сессия" value="new chat новая сессия" shortcut="Ctrl+N" onSelect={run(() => newThread())} />
            <Item
              icon={<FileText size={14} />}
              label="Новая сессия с файлом…"
              value="new chat with file новая сессия с файлом"
              onSelect={run(async () => {
                const p = await openDialog({ defaultPath: useApp.getState().settings?.workspace ?? undefined });
                if (typeof p !== "string") return;
                await newThread();
                const ws = (useApp.getState().settings?.workspace ?? "").replace(/[\\/]+$/, "");
                const rel = ws && p.toLowerCase().startsWith(ws.toLowerCase()) ? p.slice(ws.length + 1).replace(/\\/g, "/") : p;
                window.dispatchEvent(new CustomEvent("composer:set", { detail: `@${rel} ` }));
              })}
            />
            <Item
              icon={<FolderOpen size={14} />}
              label="Открыть папку проекта…"
              value="open folder открыть проект папку"
              onSelect={run(async () => {
                const d = await openDialog({ directory: true });
                if (typeof d === "string") setWorkspace(d);
              })}
            />
            {activeId && (
              <>
                <Item icon={<Download size={14} />} label="Экспорт сессии в Markdown" value="export экспорт markdown" shortcut="Ctrl+E" onSelect={run(() => exportThread(activeId, "md"))} />
                <Item
                  icon={<Shrink size={14} />}
                  label="Сжать историю сессии"
                  value="compact сжать контекст"
                  onSelect={run(() =>
                    api.request("POST", `/v1/threads/${activeId}/compact`, {}).then(
                      () => toast.success("Сжатие запущено"),
                      (e) => toast.error(String(e)),
                    ),
                  )}
                />
              </>
            )}
            <Item icon={<Settings size={14} />} label="Настройки" value="settings настройки ollama" shortcut="Ctrl+," onSelect={run(() => setOverlay("settings"))} />
            <Item icon={<Cpu size={14} />} label="Управление моделями Ollama" value="models модели ollama" onSelect={run(() => setOverlay("models"))} />
            <Item
              icon={theme === "dark" ? <Sun size={14} /> : <Moon size={14} />}
              label={theme === "dark" ? "Светлая тема" : "Тёмная тема"}
              value="theme тема светлая тёмная"
              onSelect={run(() => update({ theme: theme === "dark" ? "light" : "dark" }))}
            />
            <Item icon={<PanelLeft size={14} />} label="Показать/скрыть боковую панель" value="sidebar боковая панель" shortcut="Ctrl+B" onSelect={run(() => useApp.getState().toggleSidebar())} />
            <Item icon={<PanelRight size={14} />} label="Показать/скрыть инспектор" value="inspector инспектор" shortcut="Ctrl+J" onSelect={run(() => useApp.getState().toggleInspector())} />
            <Item icon={<ScrollText size={14} />} label="Журнал CodeWhale" value="logs журнал лог" onSelect={run(() => setOverlay("logs"))} />
            <Item icon={<BarChart3 size={14} />} label="Статистика использования" value="usage статистика токены" onSelect={run(() => setOverlay("usage"))} />
            <Item
              icon={<RefreshCw size={14} />}
              label="Перезапустить CodeWhale"
              value="restart перезапуск runtime"
              onSelect={run(() => api.runtimeRestart().then((r) => useApp.getState().setRuntime(r), (e) => toast.error(String(e))))}
            />
            <Item icon={<Keyboard size={14} />} label="Горячие клавиши" value="shortcuts горячие клавиши help" shortcut="Ctrl+/" onSelect={run(() => setOverlay("shortcuts"))} />
          </Command.Group>
          {models.filter((m) => !m.embedding).length > 0 && (
            <Command.Group heading="Модель">
              {models
                .filter((m) => !m.embedding)
                .map((m) => (
                  <Item key={m.name} icon={<Cpu size={14} />} label={`Модель: ${m.name}`} value={`model модель ${m.name}`} onSelect={run(() => setModel(m.name))} />
                ))}
            </Command.Group>
          )}
          {recent.length > 1 && (
            <Command.Group heading="Проекты">
              {recent.map((p) => (
                <Item key={p} icon={<FolderOpen size={14} />} label={p} value={`project проект ${p}`} onSelect={run(() => setWorkspace(p))} />
              ))}
            </Command.Group>
          )}
          {summaries.length > 0 && (
            <Command.Group heading="Сессии">
              {summaries.slice(0, 50).map((s) => (
                <Item key={s.id} icon={<MessageSquare size={14} />} label={s.title} value={`session ${s.title} ${s.id}`} onSelect={run(() => openThread(s.id))} />
              ))}
            </Command.Group>
          )}
        </Command.List>
      </Command>
    </div>
  );
}
