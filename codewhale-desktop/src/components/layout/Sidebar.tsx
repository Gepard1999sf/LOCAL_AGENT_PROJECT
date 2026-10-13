import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import {
  BarChart3,
  Copy,
  Download,
  FileJson,
  HelpCircle,
  MessageSquare,
  MoreHorizontal,
  Pencil,
  Plus,
  Search,
  Settings,
  SquareTerminal,
  Trash2,
  GitBranch,
  Loader2,
  ShieldQuestion,
} from "lucide-react";
import { summaryTurnActive, useApp } from "../../store/app";
import { useThreads } from "../../store/threads";
import { api } from "../../lib/tauri";
import { basename, dateGroup, fmtTime, type DateGroup } from "../../lib/format";
import { exportThread } from "../../lib/export";
import { IconButton } from "../ui/primitives";
import type { ThreadSummary } from "../../lib/types";

const GROUPS: DateGroup[] = ["Сегодня", "Вчера", "Последние 7 дней", "Ранее"];
const menuContent = "z-50 min-w-[190px] rounded-[var(--radius-md)] border border-line bg-overlay p-1 text-[13px] shadow-[var(--shadow-lg)]";
const menuItem = "flex cursor-pointer select-none items-center gap-2 rounded-[var(--radius-sm)] px-2.5 py-1.5 outline-none data-[highlighted]:bg-hover";

function Highlight({ text, q }: { text: string; q: string }) {
  if (!q) return <>{text}</>;
  const i = text.toLowerCase().indexOf(q.toLowerCase());
  if (i < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, i)}
      <mark className="rounded-sm bg-[color-mix(in_srgb,var(--accent-yellow)_35%,transparent)] text-fg">{text.slice(i, i + q.length)}</mark>
      {text.slice(i + q.length)}
    </>
  );
}

function MenuItems({ s, onRename, onDelete, Item }: { s: ThreadSummary; onRename: () => void; onDelete: () => void; Item: any }) {
  const fork = useApp((st) => st.forkThread);
  return (
    <>
      <Item className={menuItem} onSelect={onRename}>
        <Pencil size={13} /> Переименовать
      </Item>
      <Item className={menuItem} onSelect={() => fork(s.id)}>
        <Copy size={13} /> Дублировать
      </Item>
      <Item className={menuItem} onSelect={() => exportThread(s.id, "md")}>
        <Download size={13} /> Экспорт в Markdown
      </Item>
      <Item className={menuItem} onSelect={() => exportThread(s.id, "json")}>
        <FileJson size={13} /> Экспорт в JSON
      </Item>
      <div className="my-1 h-px bg-line" />
      <Item className={`${menuItem} text-red`} onSelect={onDelete}>
        <Trash2 size={13} /> Удалить
      </Item>
    </>
  );
}

const SessionItem = memo(function SessionItem({ s, active, q, hotkey }: { s: ThreadSummary; active: boolean; q: string; hotkey?: number }) {
  const openThread = useApp((st) => st.openThread);
  const rename = useApp((st) => st.renameThread);
  const archive = useApp((st) => st.archiveThread);
  const running = useThreads((st) => !!st.threads[s.id]?.activeTurnId) || summaryTurnActive(s);
  const [editing, setEditing] = useState(false);
  const [confirmDel, setConfirmDel] = useState(false);
  const [title, setTitle] = useState(s.title);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => setTitle(s.title), [s.title]);
  useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const commit = () => {
    setEditing(false);
    const t = title.trim();
    if (t && t !== s.title) rename(s.id, t);
  };

  const body: ReactNode = (
    <div
      role="button"
      tabIndex={0}
      onClick={() => !editing && openThread(s.id)}
      onKeyDown={(e) => e.key === "Enter" && openThread(s.id)}
      className={`group/item relative mx-2 cursor-pointer rounded-[var(--radius-sm)] px-2.5 py-1.5 transition-colors duration-100 ${
        active ? "bg-hover" : "hover:bg-[color-mix(in_srgb,var(--bg-hover)_60%,transparent)]"
      }`}
    >
      {active && <span className="absolute left-0 top-1.5 bottom-1.5 w-0.5 rounded-full bg-blue" />}
      <div className="flex items-center gap-1.5">
        {running ? (
          <Loader2 size={12} className="shrink-0 animate-spin text-bluefg" />
        ) : s.pending_attention_count > 0 ? (
          <ShieldQuestion size={12} className="shrink-0 text-yellow" />
        ) : null}
        {editing ? (
          <input
            ref={inputRef}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              if (e.key === "Escape") {
                setTitle(s.title);
                setEditing(false);
              }
              e.stopPropagation();
            }}
            onClick={(e) => e.stopPropagation()}
            className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-blue bg-base px-1 text-[13px] outline-none"
          />
        ) : (
          <span
            className="min-w-0 flex-1 truncate text-[13px] text-fg"
            onDoubleClick={(e) => {
              e.stopPropagation();
              setEditing(true);
            }}
          >
            <Highlight text={s.title || "Без названия"} q={q} />
          </span>
        )}
        {hotkey != null && <span className="hidden text-[10px] text-fg3 group-hover/item:inline">Ctrl+{hotkey}</span>}
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <button
              onClick={(e) => e.stopPropagation()}
              className="hidden h-5 w-5 items-center justify-center rounded text-fg3 hover:bg-overlay hover:text-fg group-hover/item:inline-flex data-[state=open]:inline-flex"
              aria-label="Меню сессии"
            >
              <MoreHorizontal size={14} />
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content className={menuContent} align="end" sideOffset={4}>
              <MenuItems s={s} onRename={() => setEditing(true)} onDelete={() => setConfirmDel(true)} Item={DropdownMenu.Item} />
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      </div>
      <div className="mt-0.5 flex items-center gap-1 truncate text-[11px] text-fg3">
        <span>{fmtTime(s.updated_at)}</span>
        <span>·</span>
        <span className="truncate font-mono">{s.model}</span>
        {s.branch && (
          <>
            <span>·</span>
            <GitBranch size={10} /> <span>{s.branch}</span>
          </>
        )}
      </div>
      {q && s.preview && s.preview !== s.title && (
        <div className="mt-0.5 line-clamp-2 text-[11px] text-fg2">
          <Highlight text={s.preview.slice(0, 160)} q={q} />
        </div>
      )}
      {confirmDel && (
        <div className="mt-1.5 flex items-center gap-2 text-[12px]" onClick={(e) => e.stopPropagation()}>
          <span className="text-red">Удалить сессию?</span>
          <button className="rounded bg-red px-2 py-0.5 text-white" onClick={() => archive(s.id)}>
            Да
          </button>
          <button className="rounded px-2 py-0.5 text-fg2 hover:bg-overlay" onClick={() => setConfirmDel(false)}>
            Нет
          </button>
        </div>
      )}
    </div>
  );

  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>{body}</ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content className={menuContent}>
          <MenuItems s={s} onRename={() => setEditing(true)} onDelete={() => setConfirmDel(true)} Item={ContextMenu.Item} />
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
});

export function Sidebar() {
  const collapsed = useApp((s) => s.sidebarCollapsed);
  const summaries = useApp((s) => s.summaries);
  const sessions = useApp((s) => s.sessions);
  const loaded = useApp((s) => s.summariesLoaded);
  const activeId = useApp((s) => s.activeThreadId);
  const search = useApp((s) => s.search);
  const setSearch = useApp((s) => s.setSearch);
  const newThread = useApp((s) => s.newThread);
  const openThread = useApp((s) => s.openThread);
  const resumeSession = useApp((s) => s.resumeSession);
  const setOverlay = useApp((s) => s.setOverlay);
  const toggleSidebar = useApp((s) => s.toggleSidebar);
  const setSidebarOrder = useApp((s) => s.setSidebarOrder);
  const [remoteHits, setRemoteHits] = useState<string[] | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  // Server-side search over thread content, debounced.
  useEffect(() => {
    if (!search.trim()) {
      setRemoteHits(null);
      return;
    }
    const t = setTimeout(async () => {
      try {
        const res = await api.request<ThreadSummary[]>("GET", `/v1/threads/summary?limit=100&search=${encodeURIComponent(search.trim())}`);
        setRemoteHits(res.map((r) => r.id));
      } catch {
        setRemoteHits(null);
      }
    }, 250);
    return () => clearTimeout(t);
  }, [search]);

  useEffect(() => {
    const onFocus = () => {
      if (useApp.getState().sidebarCollapsed) toggleSidebar();
      setTimeout(() => searchRef.current?.focus(), 50);
    };
    window.addEventListener("sidebar:search", onFocus);
    return () => window.removeEventListener("sidebar:search", onFocus);
  }, [toggleSidebar]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return summaries;
    return summaries.filter(
      (s) =>
        s.title.toLowerCase().includes(q) ||
        s.preview.toLowerCase().includes(q) ||
        basename(s.workspace).toLowerCase().includes(q) ||
        remoteHits?.includes(s.id),
    );
  }, [summaries, search, remoteHits]);

  const grouped = useMemo(() => {
    const g = new Map<DateGroup, ThreadSummary[]>();
    for (const s of filtered) {
      const k = dateGroup(s.updated_at);
      g.set(k, [...(g.get(k) ?? []), s]);
    }
    return g;
  }, [filtered]);

  // Sessions in the order listed (the collapsed rail is unfiltered). Ctrl+1..9
  // (useShortcuts) open the first nine of the order published to the store.
  const ordered = useMemo(
    () => (collapsed ? summaries : GROUPS.flatMap((g) => grouped.get(g) ?? [])),
    [collapsed, summaries, grouped],
  );
  const hotkeys = useMemo(() => new Map(ordered.slice(0, 9).map((s, i) => [s.id, i + 1])), [ordered]);
  useEffect(() => setSidebarOrder(ordered.map((s) => s.id)), [ordered, setSidebarOrder]);
  useEffect(() => () => setSidebarOrder(null), [setSidebarOrder]);

  if (collapsed) {
    return (
      <aside className="panel-transition flex w-12 shrink-0 flex-col items-center gap-1 border-r border-line bg-elevated py-2">
        <IconButton label="Новая сессия (Ctrl+N)" tipSide="right" onClick={() => newThread()}>
          <Plus size={16} />
        </IconButton>
        <IconButton label="Поиск по сессиям" tipSide="right" onClick={() => window.dispatchEvent(new Event("sidebar:search"))}>
          <Search size={16} />
        </IconButton>
        <div className="my-1 h-px w-6 bg-line" />
        <div className="flex min-h-0 flex-1 flex-col items-center gap-1 overflow-y-auto">
          {summaries.slice(0, 12).map((s) => (
            <IconButton key={s.id} label={s.title} tipSide="right" active={s.id === activeId} onClick={() => openThread(s.id)}>
              <MessageSquare size={15} />
            </IconButton>
          ))}
        </div>
        <IconButton label="Настройки" tipSide="right" onClick={() => setOverlay("settings")}>
          <Settings size={16} />
        </IconButton>
      </aside>
    );
  }

  return (
    <aside className="panel-transition flex w-[260px] shrink-0 flex-col border-r border-line bg-elevated">
      <div className="p-2">
        <button
          onClick={() => newThread()}
          className="flex h-9 w-full items-center gap-2 rounded-[var(--radius-sm)] bg-blue px-3 text-[13px] font-medium text-white hover:brightness-110"
        >
          <Plus size={15} /> Новая сессия <span className="ml-auto text-[11px] opacity-80">Ctrl+N</span>
        </button>
      </div>
      <div className="px-2 pb-2">
        <div className="flex h-8 items-center gap-2 rounded-[var(--radius-sm)] border border-line bg-base px-2 focus-within:border-blue">
          <Search size={13} className="text-fg3" />
          <input
            ref={searchRef}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setSearch("")}
            placeholder="Поиск по сессиям…"
            className="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-fg3"
          />
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto pb-2">
        {!loaded &&
          [0, 1, 2, 3].map((i) => (
            <div key={i} className="mx-3 my-2 space-y-1.5">
              <div className="h-3.5 w-4/5 skeleton" />
              <div className="h-2.5 w-1/2 skeleton" />
            </div>
          ))}
        {loaded && filtered.length === 0 && (
          <div className="px-4 py-6 text-center text-[12px] text-fg3">{search ? "Ничего не найдено" : "Сессий пока нет"}</div>
        )}
        {GROUPS.map((g) => {
          const list = grouped.get(g);
          if (!list?.length) return null;
          return (
            <div key={g} className="mb-1">
              <div className="flex items-center gap-2 px-4 pb-1 pt-2 text-[11px] font-medium text-fg3">
                <span>{g}</span>
                <span className="h-px flex-1 bg-line-muted" />
              </div>
              {list.map((s) => (
                <SessionItem key={s.id} s={s} active={s.id === activeId} q={search.trim()} hotkey={hotkeys.get(s.id)} />
              ))}
            </div>
          );
        })}
        {sessions.length > 0 && !search && (
          <div className="mt-2">
            <div className="flex items-center gap-2 px-4 pb-1 pt-2 text-[11px] font-medium text-fg3">
              <span>Сессии терминала (TUI)</span>
              <span className="h-px flex-1 bg-line-muted" />
            </div>
            {sessions.slice(0, 15).map((s) => (
              <button
                key={s.id}
                onClick={() => resumeSession(s.id)}
                className="mx-2 flex w-[calc(100%-16px)] items-start gap-1.5 rounded-[var(--radius-sm)] px-2.5 py-1.5 text-left hover:bg-[color-mix(in_srgb,var(--bg-hover)_60%,transparent)]"
                title="Открыть сессию терминала в приложении"
              >
                <SquareTerminal size={12} className="mt-0.5 shrink-0 text-fg3" />
                <span className="min-w-0">
                  <span className="block truncate text-[12.5px] text-fg2">{s.title}</span>
                  <span className="block truncate text-[11px] text-fg3">
                    {fmtTime(s.updated_at)} · {s.message_count} сообщ. · {s.model}
                  </span>
                </span>
              </button>
            ))}
          </div>
        )}
      </div>
      <div className="flex items-center gap-1 border-t border-line px-2 py-1.5">
        <button onClick={() => setOverlay("settings")} className="inline-flex h-7 items-center gap-1.5 rounded-[var(--radius-sm)] px-2 text-[12px] text-fg2 hover:bg-hover hover:text-fg">
          <Settings size={13} /> Настройки
        </button>
        <button onClick={() => setOverlay("usage")} className="inline-flex h-7 items-center gap-1.5 rounded-[var(--radius-sm)] px-2 text-[12px] text-fg2 hover:bg-hover hover:text-fg">
          <BarChart3 size={13} /> Usage
        </button>
        <IconButton size="sm" label="Горячие клавиши (Ctrl+/)" className="ml-auto" onClick={() => setOverlay("shortcuts")}>
          <HelpCircle size={14} />
        </IconButton>
      </div>
    </aside>
  );
}
