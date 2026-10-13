import { useState } from "react";
import { DropdownMenu } from "radix-ui";
import { Copy, Download, FileJson, GitBranch, MoreHorizontal, Pencil, Shrink, Trash2, WifiOff } from "lucide-react";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { toolLog, useThreads } from "../../store/threads";
import { MessageList } from "./MessageList";
import { Composer } from "../composer/Composer";
import { EmptyState } from "../screens/EmptyState";
import { exportThread } from "../../lib/export";
import { api } from "../../lib/tauri";
import { fmtTime, fmtTokens } from "../../lib/format";
import { IconButton } from "../ui/primitives";

const menuContent = "z-50 min-w-[200px] rounded-[var(--radius-md)] border border-line bg-overlay p-1 text-[13px] shadow-[var(--shadow-lg)]";
const menuItem = "flex cursor-pointer select-none items-center gap-2 rounded-[var(--radius-sm)] px-2.5 py-1.5 outline-none data-[highlighted]:bg-hover";

function SessionHeader({ threadId }: { threadId: string }) {
  const summary = useApp((s) => s.summaries.find((x) => x.id === threadId));
  const rename = useApp((s) => s.renameThread);
  const archive = useApp((s) => s.archiveThread);
  const fork = useApp((s) => s.forkThread);
  const ctx = useApp((s) => s.contexts[threadId]);
  const thread = useThreads((s) => s.threads[threadId]);
  const streamState = thread?.streamState;
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState("");
  const [confirmDel, setConfirmDel] = useState(false);
  const tools = toolLog(thread).length;
  const tokens = thread?.turns.reduce((a, t) => a + (t.usage?.input_tokens ?? 0) + t.outputTokens, 0) ?? 0;
  const name = summary?.title ?? (thread?.turns[0]?.prompt?.slice(0, 70) || "Новая сессия");

  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-line px-5 py-2">
      <div className="min-w-0 flex-1">
        {editing ? (
          <input
            autoFocus
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={() => {
              setEditing(false);
              if (title.trim() && title !== name) rename(threadId, title.trim());
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              if (e.key === "Escape") setEditing(false);
            }}
            className="w-full rounded-[var(--radius-sm)] border border-blue bg-base px-1.5 py-0.5 text-[15px] font-semibold outline-none"
          />
        ) : (
          <div
            className="truncate text-[15px] font-semibold"
            onDoubleClick={() => {
              setTitle(name);
              setEditing(true);
            }}
            title="Двойной клик — переименовать"
          >
            {name}
          </div>
        )}
        <div className="flex items-center gap-1.5 text-[11.5px] text-fg3">
          {summary && <span>{fmtTime(summary.updated_at)}</span>}
          <span>· {fmtTokens(tokens)} токенов</span>
          <span>· {tools} инструментов</span>
          {ctx?.window_tokens ? <span>· окно {fmtTokens(ctx.window_tokens)}</span> : null}
          {streamState === "reconnecting" && (
            <span className="inline-flex items-center gap-1 text-yellow">
              · <WifiOff size={11} /> переподключение
            </span>
          )}
        </div>
      </div>
      {summary?.branch && (
        <span className="inline-flex items-center gap-1 rounded-full border border-line px-2 py-0.5 text-[11px] text-fg2">
          <GitBranch size={11} /> {summary.branch}
        </span>
      )}
      {confirmDel ? (
        <span className="flex items-center gap-2 text-[12px]">
          <span className="text-red">Удалить сессию?</span>
          <button className="rounded bg-red px-2 py-0.5 text-white" onClick={() => archive(threadId)}>
            Да
          </button>
          <button className="rounded px-2 py-0.5 text-fg2 hover:bg-hover" onClick={() => setConfirmDel(false)}>
            Нет
          </button>
        </span>
      ) : (
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <IconButton label="Действия с сессией">
              <MoreHorizontal size={16} />
            </IconButton>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content className={menuContent} align="end" sideOffset={4}>
              <DropdownMenu.Item className={menuItem} onSelect={() => { setTitle(name); setEditing(true); }}>
                <Pencil size={13} /> Переименовать
              </DropdownMenu.Item>
              <DropdownMenu.Item className={menuItem} onSelect={() => exportThread(threadId, "md")}>
                <Download size={13} /> Экспорт в Markdown <span className="ml-auto text-[11px] text-fg3">Ctrl+E</span>
              </DropdownMenu.Item>
              <DropdownMenu.Item className={menuItem} onSelect={() => exportThread(threadId, "json")}>
                <FileJson size={13} /> Экспорт в JSON
              </DropdownMenu.Item>
              <DropdownMenu.Item className={menuItem} onSelect={() => fork(threadId)}>
                <Copy size={13} /> Дублировать <span className="ml-auto text-[11px] text-fg3">Ctrl+D</span>
              </DropdownMenu.Item>
              <DropdownMenu.Item
                className={menuItem}
                onSelect={() =>
                  api.request("POST", `/v1/threads/${threadId}/compact`, {}).then(
                    () => toast.success("Сжатие контекста запущено"),
                    (e) => toast.error("Не удалось сжать", { description: String(e) }),
                  )
                }
              >
                <Shrink size={13} /> Сжать историю
              </DropdownMenu.Item>
              <div className="my-1 h-px bg-line" />
              <DropdownMenu.Item className={`${menuItem} text-red`} onSelect={() => setConfirmDel(true)}>
                <Trash2 size={13} /> Удалить сессию
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      )}
    </div>
  );
}

export function ChatArea() {
  const threadId = useApp((s) => s.activeThreadId);
  return (
    <main className="flex min-w-[360px] flex-1 flex-col bg-base">
      {threadId ? (
        <>
          <SessionHeader threadId={threadId} />
          <MessageList threadId={threadId} />
        </>
      ) : (
        <EmptyState />
      )}
      <Composer threadId={threadId} />
    </main>
  );
}
