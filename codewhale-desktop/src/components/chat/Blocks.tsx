import { memo, useState } from "react";
import {
  AlertTriangle,
  Brain,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Loader2,
  RotateCcw,
  ShieldCheck,
  ShieldQuestion,
  ShieldX,
  Shrink,
  X,
} from "lucide-react";
import { useThreads, type Approval, type ItemView } from "../../store/threads";
import { primaryArg, toolMeta } from "../../lib/tools";
import { fmtTimeSec, fmtDuration } from "../../lib/format";
import { Markdown } from "./Markdown";
import { DiffView, diffStats, looksLikeDiff } from "./DiffView";
import { Button } from "../ui/primitives";
import { useApp } from "../../store/app";

function StatusIcon({ status }: { status: string }) {
  if (status === "in_progress") return <Loader2 size={13} className="animate-spin text-bluefg" />;
  if (status === "failed") return <X size={13} className="text-red" />;
  return <Check size={13} className="anim-pulse text-green" />;
}

export const ThinkingBlock = memo(function ThinkingBlock({ item }: { item: ItemView }) {
  const streaming = item.status === "in_progress";
  const [open, setOpen] = useState(false);
  const text = item.text || item.detail || "";
  const preview = text.trim().split("\n").filter(Boolean).slice(-2).join(" ");
  return (
    <div className="my-2 overflow-hidden rounded-[var(--radius-md)] border border-[color-mix(in_srgb,var(--accent-purple)_35%,transparent)] bg-[color-mix(in_srgb,var(--accent-purple)_6%,transparent)]">
      <button className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] text-purple" onClick={() => setOpen((o) => !o)}>
        <Brain size={13} className={streaming ? "animate-pulse" : ""} />
        <span className="font-medium">{streaming ? "Думаю…" : "Размышления"}</span>
        {!open && preview && <span className="min-w-0 flex-1 truncate text-fg3">{preview}</span>}
        <span className="ml-auto text-fg3">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
      </button>
      {open && (
        <div className="max-h-80 overflow-auto border-t border-[color-mix(in_srgb,var(--accent-purple)_25%,transparent)] px-3 py-2 text-[12.5px] text-fg2">
          <Markdown text={text} />
        </div>
      )}
    </div>
  );
});

function prettyJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}

export const ToolBlock = memo(function ToolBlock({ item }: { item: ItemView }) {
  const [open, setOpen] = useState(false);
  const meta = toolMeta(item.toolName ?? (item.kind === "command_execution" ? "bash" : item.kind === "file_change" ? "edit" : null));
  const Icon = meta.icon;
  const arg = primaryArg(item.toolArgs) ?? item.summary;
  const output = item.status === "in_progress" ? item.text : item.text || "";
  const diffText = item.diff ?? (looksLikeDiff(output) ? output : undefined);
  const isDiff = !!diffText;
  const stats = diffText ? diffStats(diffText) : null;
  const dur =
    item.started_at && item.ended_at ? new Date(item.ended_at).getTime() - new Date(item.started_at).getTime() : null;

  return (
    <div className="my-2 overflow-hidden rounded-[var(--radius-md)] border border-line bg-elevated">
      <button className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px]" onClick={() => setOpen((o) => !o)}>
        <Icon size={13} className="shrink-0 text-fg2" />
        <span className="shrink-0 font-mono font-medium text-fg">{item.toolName ?? item.kind}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-fg2">{arg}</span>
        {stats && (
          <span className="shrink-0 font-mono text-[11px]">
            <span className="text-[var(--diff-add-fg)]">+{stats.add}</span> <span className="text-[var(--diff-del-fg)]">−{stats.del}</span>
          </span>
        )}
        <span className="shrink-0 text-[11px] text-fg3">{dur != null && dur > 0 ? fmtDuration(dur) : fmtTimeSec(item.started_at)}</span>
        <StatusIcon status={item.status} />
        <span className="text-fg3">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
      </button>
      {item.status === "in_progress" && <div className="progress-indeterminate h-0.5 bg-overlay" />}
      {open && (
        <div className="border-t border-line-muted text-[12px]">
          {item.toolArgs && (
            <div className="border-b border-line-muted px-3 py-2">
              <div className="mb-1 text-[11px] uppercase tracking-wide text-fg3">Аргументы</div>
              <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words font-mono text-[11.5px] text-fg2">{prettyJson(item.toolArgs)}</pre>
            </div>
          )}
          {isDiff && <DiffView text={diffText!} maxHeight={360} />}
          {output && !(isDiff && output === diffText) ? (
            isDiff ? (
              <div className="border-t border-line-muted px-3 py-1.5 font-mono text-[11.5px] text-fg3">{output}</div>
            ) : (
              <div className="px-3 py-2">
                <div className="mb-1 text-[11px] uppercase tracking-wide text-fg3">Результат</div>
                <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words font-mono text-[11.5px] text-fg2">{output}</pre>
              </div>
            )
          ) : !isDiff ? (
            <div className="px-3 py-2 text-fg3">{item.status === "in_progress" ? "Выполняется…" : "Нет вывода"}</div>
          ) : null}
          {item.metadata && Object.keys(item.metadata).length > 0 && (
            <details className="border-t border-line-muted px-3 py-1.5 text-fg3">
              <summary className="cursor-pointer text-[11px]">Метаданные</summary>
              <pre className="max-h-48 overflow-auto font-mono text-[11px]">{prettyJson(item.metadata)}</pre>
            </details>
          )}
        </div>
      )}
    </div>
  );
});

export function ErrorBlock({ message, onRetry }: { message: string; onRetry?: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="my-2 rounded-[var(--radius-md)] border border-[color-mix(in_srgb,var(--accent-red)_45%,transparent)] bg-[color-mix(in_srgb,var(--accent-red)_8%,transparent)] px-3 py-2.5">
      <div className="flex items-center gap-2 text-[13px] font-medium text-red">
        <AlertTriangle size={14} /> Ошибка
      </div>
      <div className="mt-1 whitespace-pre-wrap break-words font-mono text-[12px] text-fg2">{message}</div>
      <div className="mt-2 flex gap-2">
        {onRetry && (
          <Button onClick={onRetry}>
            <RotateCcw size={13} /> Повторить
          </Button>
        )}
        <Button
          onClick={() => {
            navigator.clipboard.writeText(message);
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }}
        >
          <Copy size={13} /> {copied ? "Скопировано" : "Копировать"}
        </Button>
      </div>
    </div>
  );
}

export function CompactionBlock({ item }: { item: ItemView }) {
  return (
    <div className="my-3 flex items-center gap-2 text-[11px] text-fg3">
      <div className="h-px flex-1 bg-line" />
      <Shrink size={12} /> {item.summary || "Контекст сжат, старая часть диалога заменена выжимкой"}
      <div className="h-px flex-1 bg-line" />
    </div>
  );
}

export function StatusLine({ item }: { item: ItemView }) {
  const text = item.text || item.summary || item.detail;
  if (!text) return null;
  return <div className="my-1 text-[11.5px] italic text-fg3">{text}</div>;
}

function ArgsPreview({ name, args }: { name?: string; args?: Record<string, unknown> }) {
  if (!args) return null;
  const path = (args.path ?? args.file_path) as string | undefined;
  const content = args.content as string | undefined;
  const command = args.command as string | undefined;
  const oldS = (args.old_string ?? args.old_str ?? args.search) as string | undefined;
  const newS = (args.new_string ?? args.new_str ?? args.replace) as string | undefined;
  if (command) return <pre className="whitespace-pre-wrap break-words px-2.5 py-2 font-mono text-[12px] text-fg">{command}</pre>;
  if (path && oldS != null && newS != null) {
    const diff = [
      "--- a/" + path,
      "+++ b/" + path,
      "@@ -1 +1 @@",
      ...oldS.split("\n").map((l) => "-" + l),
      ...newS.split("\n").map((l) => "+" + l),
    ].join("\n");
    return (
      <>
        <div className="px-2.5 pt-1.5 font-mono text-[12px] text-fg2">{path}</div>
        <DiffView text={diff} maxHeight={260} />
      </>
    );
  }
  if (path && content != null) {
    const lines = content.split("\n");
    return (
      <>
        <div className="px-2.5 pt-1.5 font-mono text-[12px] text-fg2">
          {path} <span className="text-fg3">· {lines.length} строк</span>
        </div>
        <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-words px-2.5 py-2 font-mono text-[11.5px] text-[var(--diff-add-fg)]">
          {lines.slice(0, 60).join("\n")}
          {lines.length > 60 ? "\n…" : ""}
        </pre>
      </>
    );
  }
  return <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-words px-2.5 py-2 font-mono text-[11.5px] text-fg">{name === undefined ? "" : JSON.stringify(args, null, 2)}</pre>;
}

export const ApprovalCard = memo(function ApprovalCard({ threadId, approval }: { threadId: string; approval: Approval }) {
  const decide = useApp((s) => s.decideApproval);
  const meta = toolMeta(approval.toolName);
  const Icon = meta.icon;
  const toolItem = useThreads((s) => {
    const t = s.threads[threadId];
    const id = approval.toolCallId ? t?.toolCalls[approval.toolCallId] : undefined;
    return id ? t?.items[id] : undefined;
  });
  const args = approval.args ?? toolItem?.toolArgs;
  const arg = primaryArg(args);
  const pending = approval.status === "pending";
  const title: string | undefined = approval.payload?.summary;
  const reason = approval.payload?.intent_summary ?? approval.payload?.reason ?? approval.payload?.message;
  const riskColor =
    meta.risk === "exec" ? "var(--accent-red)" : meta.risk === "write" ? "var(--accent-yellow)" : meta.risk === "network" ? "var(--accent-purple)" : "var(--accent-green)";

  if (!pending) {
    const label =
      approval.status === "allowed" ? "Разрешено" : approval.status === "denied" ? "Отклонено" : approval.status === "timeout" ? "Время ожидания истекло" : "Отменено";
    const I = approval.status === "allowed" ? ShieldCheck : ShieldX;
    return (
      <div className="my-1.5 flex items-center gap-2 text-[11.5px] text-fg3">
        <I size={13} /> {label}: <span className="font-mono">{approval.toolName ?? "инструмент"}</span>
        {arg && <span className="truncate font-mono">{arg}</span>}
      </div>
    );
  }

  return (
    <div
      className="my-2 overflow-hidden rounded-[var(--radius-md)] border bg-elevated"
      style={{ borderColor: `color-mix(in srgb, ${riskColor} 55%, transparent)` }}
    >
      <div className="flex items-center gap-2 px-3 pt-2.5 text-[13px] font-medium">
        <ShieldQuestion size={15} style={{ color: riskColor }} />
        Агент просит разрешение{title ? <span className="font-normal text-fg2">: {title}</span> : null}
      </div>
      <div className="mx-3 my-2 overflow-hidden rounded-[var(--radius-sm)] border border-line-muted bg-base">
        <div className="flex items-center gap-2 border-b border-line-muted px-2.5 py-1 text-[12px]">
          <Icon size={13} className="text-fg2" />
          <span className="font-mono">{approval.toolName ?? "tool"}</span>
          <span className="text-fg3">· {meta.label}</span>
        </div>
        {args ? <ArgsPreview name={approval.toolName} args={args} /> : <pre className="px-2.5 py-2 font-mono text-[12px] text-fg3">{approval.payload?.description ?? "Аргументы недоступны"}</pre>}
      </div>
      {reason && typeof reason === "string" && <div className="line-clamp-3 px-3 pb-1 text-[12px] text-fg2">{reason.trim()}</div>}
      <div className="flex flex-wrap gap-2 px-3 pb-3 pt-1">
        <Button variant="success" onClick={() => decide(threadId, approval.id, true)}>
          <Check size={13} /> Разрешить
        </Button>
        <Button variant="danger" onClick={() => decide(threadId, approval.id, false)}>
          <X size={13} /> Отклонить
        </Button>
        <Button variant="ghost" onClick={() => decide(threadId, approval.id, true, true)} title="Разрешать такие вызовы до конца сессии">
          <ShieldCheck size={13} /> Всегда в этой сессии
        </Button>
      </div>
    </div>
  );
});
