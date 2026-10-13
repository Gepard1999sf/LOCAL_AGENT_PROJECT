import { DropdownMenu } from "radix-ui";
import {
  AlertTriangle,
  Brain,
  Check,
  ChevronDown,
  Eye,
  FolderOpen,
  GitBranch,
  Moon,
  PanelLeft,
  PanelRight,
  Settings,
  SlidersHorizontal,
  Sun,
  SunMoon,
  Wrench,
  Zap,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useApp } from "../../store/app";
import { basename, fmtBytes, fmtTokens } from "../../lib/format";
import { IconButton } from "../ui/primitives";
import type { OllamaModel } from "../../lib/types";

const EMPTY: string[] = [];
const menuContent =
  "z-50 min-w-[300px] overflow-hidden rounded-[var(--radius-md)] border border-line bg-overlay p-1 text-[13px] shadow-[var(--shadow-lg)]";
const menuItem =
  "flex cursor-pointer select-none items-center gap-2 rounded-[var(--radius-sm)] px-2.5 py-1.5 outline-none data-[highlighted]:bg-hover";

export function effectiveCtx(m?: OllamaModel): number | null {
  if (!m) return null;
  return m.numCtx ?? null;
}

export function ModelBadges({ m }: { m: OllamaModel }) {
  const caps = m.capabilities;
  const sizeB = parseFloat(m.parameterSize ?? "0");
  return (
    <span className="flex items-center gap-1 text-fg3">
      {sizeB > 0 && sizeB <= 5 && (
        <span title="Быстрая">
          <Zap size={12} />
        </span>
      )}
      {caps.includes("thinking") && (
        <span title="Рассуждает (thinking)">
          <Brain size={12} />
        </span>
      )}
      {caps.includes("tools") && (
        <span title="Вызывает инструменты">
          <Wrench size={12} />
        </span>
      )}
      {caps.includes("vision") && (
        <span title="Понимает изображения">
          <Eye size={12} />
        </span>
      )}
    </span>
  );
}

function ModelSelector() {
  const models = useApp((s) => s.models);
  const current = useApp((s) => s.settings?.defaultModel);
  const setModel = useApp((s) => s.setModel);
  const setOverlay = useApp((s) => s.setOverlay);
  const refreshModels = useApp((s) => s.refreshModels);
  const chat = models.filter((m) => !m.embedding);
  const cur = chat.find((m) => m.name === current);
  const ctx = effectiveCtx(cur);

  return (
    <DropdownMenu.Root onOpenChange={(o) => o && refreshModels()}>
      <DropdownMenu.Trigger asChild>
        <button className="inline-flex h-8 max-w-[300px] items-center gap-2 rounded-[var(--radius-sm)] border border-line bg-elevated px-2.5 text-[13px] hover:bg-hover">
          <span className="truncate font-mono">{current ?? "Модель не выбрана"}</span>
          {cur && (
            <span className={`text-[11px] ${ctx && ctx >= 16384 ? "text-fg3" : "text-yellow"}`}>
              {ctx ? fmtTokens(ctx) : "4k"}
            </span>
          )}
          {cur && !cur.capabilities.includes("tools") && <AlertTriangle size={13} className="text-yellow" />}
          <ChevronDown size={13} className="text-fg3" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className={menuContent} sideOffset={6} align="start">
          {chat.length === 0 && <div className="px-2.5 py-2 text-fg3">Нет моделей. Скачайте модель в Ollama.</div>}
          {chat.map((m) => {
            const c = effectiveCtx(m);
            const agentOk = m.capabilities.includes("tools");
            return (
              <DropdownMenu.Item key={m.name} className={menuItem} onSelect={() => setModel(m.name)}>
                <span className="w-3 text-blue">{m.name === current ? "●" : ""}</span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-mono">{m.name}</span>
                  <span className="block text-[11px] text-fg3">
                    {m.parameterSize} · {m.quantization} · {fmtBytes(m.size)}
                    {!agentOk && <span className="text-yellow"> · без инструментов</span>}
                  </span>
                </span>
                <span
                  className={`text-[11px] ${c && c >= 16384 ? "text-fg2" : "text-yellow"}`}
                  title={c ? "num_ctx из Modelfile" : "Контекст по умолчанию Ollama: 4096 токенов, агенту мало"}
                >
                  {c ? fmtTokens(c) : "4k!"}
                </span>
                <ModelBadges m={m} />
              </DropdownMenu.Item>
            );
          })}
          <DropdownMenu.Separator className="my-1 h-px bg-line" />
          <DropdownMenu.Item className={menuItem} onSelect={() => setOverlay("models")}>
            <SlidersHorizontal size={13} /> Управление моделями…
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function ProjectSelector() {
  const ws = useApp((s) => s.settings?.workspace);
  const recent = useApp((s) => s.settings?.recentWorkspaces) ?? EMPTY;
  const branch = useApp((s) => s.branch);
  const setWorkspace = useApp((s) => s.setWorkspace);

  const pick = async () => {
    const dir = await openDialog({ directory: true, title: "Открыть папку проекта", defaultPath: ws ?? undefined });
    if (typeof dir === "string") setWorkspace(dir);
  };

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button className="inline-flex h-8 max-w-[280px] items-center gap-2 rounded-[var(--radius-sm)] border border-line bg-elevated px-2.5 text-[13px] hover:bg-hover">
          <FolderOpen size={14} className="text-fg2" />
          <span className="truncate">{ws ? basename(ws) : "Проект не выбран"}</span>
          {branch && (
            <span className="inline-flex items-center gap-0.5 text-[11px] text-fg3">
              <GitBranch size={11} /> {branch}
            </span>
          )}
          <ChevronDown size={13} className="text-fg3" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className={menuContent} sideOffset={6} align="start">
          {recent.length > 0 && <DropdownMenu.Label className="px-2.5 py-1 text-[11px] uppercase tracking-wide text-fg3">Недавние</DropdownMenu.Label>}
          {recent.map((p) => (
            <DropdownMenu.Item key={p} className={menuItem} onSelect={() => p !== ws && setWorkspace(p)}>
              <span className="w-3 text-blue">{p === ws ? <Check size={12} /> : null}</span>
              <span className="min-w-0 flex-1">
                <span className="block truncate">{basename(p)}</span>
                <span className="block truncate text-[11px] text-fg3">{p}</span>
              </span>
            </DropdownMenu.Item>
          ))}
          {recent.length > 0 && <DropdownMenu.Separator className="my-1 h-px bg-line" />}
          <DropdownMenu.Item className={menuItem} onSelect={pick}>
            <FolderOpen size={13} /> Открыть папку…
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function ThemeToggle() {
  const theme = useApp((s) => s.settings?.theme ?? "dark");
  const update = useApp((s) => s.updateSettings);
  const next = theme === "dark" ? "light" : theme === "light" ? "auto" : "dark";
  const Icon = theme === "dark" ? Moon : theme === "light" ? Sun : SunMoon;
  const label = theme === "dark" ? "Тема: тёмная" : theme === "light" ? "Тема: светлая" : "Тема: как в системе";
  return (
    <IconButton label={`${label} (нажмите для смены)`} onClick={() => update({ theme: next })}>
      <Icon size={16} />
    </IconButton>
  );
}

export function Toolbar() {
  const setOverlay = useApp((s) => s.setOverlay);
  const overlay = useApp((s) => s.overlay);
  const toggleSidebar = useApp((s) => s.toggleSidebar);
  const toggleInspector = useApp((s) => s.toggleInspector);
  const inspectorCollapsed = useApp((s) => s.inspectorCollapsed);
  const sidebarCollapsed = useApp((s) => s.sidebarCollapsed);
  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line bg-elevated px-2">
      <IconButton label="Боковая панель (Ctrl+B)" active={!sidebarCollapsed} onClick={toggleSidebar}>
        <PanelLeft size={16} />
      </IconButton>
      <button
        onClick={() => setOverlay("about")}
        className="mr-2 inline-flex items-center gap-1.5 rounded-[var(--radius-sm)] px-1.5 py-1 text-[15px] font-semibold hover:bg-hover"
      >
        <span>🐋</span> <span className="hidden sm:inline">CodeWhale</span>
      </button>
      <ModelSelector />
      <ProjectSelector />
      <div className="ml-auto flex items-center gap-0.5">
        <IconButton label="Настройки (Ctrl+,)" active={overlay === "settings"} onClick={() => setOverlay(overlay === "settings" ? null : "settings")}>
          <Settings size={16} />
        </IconButton>
        <ThemeToggle />
        <IconButton label="Инспектор (Ctrl+J)" active={!inspectorCollapsed} onClick={toggleInspector}>
          <PanelRight size={16} />
        </IconButton>
      </div>
    </header>
  );
}
