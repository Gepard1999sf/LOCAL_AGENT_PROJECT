import { memo, useState } from "react";
import { Check, Copy, Pencil, RotateCcw, Square } from "lucide-react";
import { useThreads, type TurnView } from "../../store/threads";
import { useApp } from "../../store/app";
import { Markdown } from "./Markdown";
import { ApprovalCard, CompactionBlock, ErrorBlock, StatusLine, ThinkingBlock, ToolBlock } from "./Blocks";
import { fmtDuration, fmtTimeSec, fmtTokens } from "../../lib/format";
import { IconButton } from "../ui/primitives";

const TOOL_KINDS = new Set(["tool_call", "command_execution", "file_change"]);

const Block = memo(function Block({ threadId, blockId, isLastInTurn, turnActive }: { threadId: string; blockId: string; isLastInTurn: boolean; turnActive: boolean }) {
  const showThinking = useApp((s) => s.settings?.showThinking ?? true);
  const showTools = useApp((s) => s.settings?.showToolCalls ?? true);
  const isApproval = blockId.startsWith("approval:");
  const approval = useThreads((s) => (isApproval ? s.threads[threadId]?.approvals[blockId.slice(9)] : undefined));
  const item = useThreads((s) => (isApproval ? undefined : s.threads[threadId]?.items[blockId]));

  if (isApproval) return approval ? <ApprovalCard threadId={threadId} approval={approval} /> : null;
  if (!item) return null;
  switch (item.kind) {
    case "agent_message": {
      const text = item.text || item.detail;
      const streaming = item.status === "in_progress" && turnActive && isLastInTurn;
      if (!text && !streaming) return null;
      return (
        <div className="my-1.5">
          <Markdown text={text} streaming={streaming} />
        </div>
      );
    }
    case "agent_reasoning":
      return showThinking ? <ThinkingBlock item={item} /> : null;
    case "context_compaction":
      return <CompactionBlock item={item} />;
    case "error":
      return <ErrorBlock message={item.text || item.detail || item.summary} />;
    case "status":
      return <StatusLine item={item} />;
    case "user_message":
      return null;
    default:
      if (TOOL_KINDS.has(item.kind) || item.toolName) return showTools ? <ToolBlock item={item} /> : null;
      return item.text || item.detail ? <StatusLine item={item} /> : null;
  }
});

function UserMessage({ turn, onRetry }: { turn: TurnView; onRetry: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="group/user flex justify-end">
      <div className="flex max-w-[70%] flex-col items-end">
        <div className="mb-1 flex items-center gap-1 opacity-0 transition-opacity group-hover/user:opacity-100">
          <span className="mr-1 text-[11px] text-fg3">{fmtTimeSec(turn.createdAt)}</span>
          <IconButton size="sm" label="Копировать" onClick={() => { navigator.clipboard.writeText(turn.prompt); setCopied(true); setTimeout(() => setCopied(false), 1200); }}>
            {copied ? <Check size={13} /> : <Copy size={13} />}
          </IconButton>
          <IconButton
            size="sm"
            label="Редактировать"
            onClick={() => window.dispatchEvent(new CustomEvent("composer:set", { detail: turn.prompt }))}
          >
            <Pencil size={13} />
          </IconButton>
          <IconButton size="sm" label="Отправить ещё раз" onClick={onRetry}>
            <RotateCcw size={13} />
          </IconButton>
        </div>
        <div className="anim-msg-in whitespace-pre-wrap break-words rounded-[var(--radius-md)] bg-[var(--user-bubble)] px-3.5 py-2.5 text-[var(--chat-font-size)] leading-[var(--leading-normal)] text-[var(--user-bubble-fg)] shadow-[var(--shadow-sm)]">
          {turn.prompt}
        </div>
      </div>
    </div>
  );
}

export const TurnRow = memo(function TurnRow({ threadId, index }: { threadId: string; index: number }) {
  const turn = useThreads((s) => s.threads[threadId]?.turns[index]);
  const active = useThreads((s) => s.threads[threadId]?.activeTurnId === turn?.id);
  const sendPrompt = useApp((s) => s.sendPrompt);
  const interrupt = useApp((s) => s.interrupt);
  const [copied, setCopied] = useState(false);
  if (!turn) return null;

  const inProgress = active || turn.status === "in_progress" || turn.status === "queued";
  const failed = turn.status === "failed";
  const interrupted = turn.status === "interrupted" || turn.status === "canceled";
  const hasVisible = turn.blocks.length > 0;

  const copyAnswer = () => {
    const st = useThreads.getState().threads[threadId];
    const text = turn.blocks
      .map((id) => st?.items[id])
      .filter((it) => it?.kind === "agent_message")
      .map((it) => it!.text || it!.detail)
      .join("\n\n");
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1200);
  };

  return (
    <div className="mx-auto w-full max-w-[920px] px-5 py-3">
      {turn.prompt && <UserMessage turn={turn} onRetry={() => sendPrompt(turn.prompt)} />}
      <div className="group/asst anim-msg-in mt-3 rounded-[var(--radius-md)] border border-line bg-elevated px-4 py-3">
        <div className="mb-1 flex items-center gap-2 text-[12px]">
          <span className="font-semibold text-fg">🐋 CodeWhale</span>
          {turn.model && <span className="text-fg3">· {turn.model}</span>}
          <span className="ml-auto flex items-center gap-1 opacity-0 transition-opacity group-hover/asst:opacity-100">
            <span className="text-[11px] text-fg3">{fmtTimeSec(turn.startedAt ?? turn.createdAt)}</span>
            <IconButton size="sm" label="Копировать ответ" onClick={copyAnswer}>
              {copied ? <Check size={13} /> : <Copy size={13} />}
            </IconButton>
          </span>
        </div>
        {turn.blocks.map((id, i) => (
          <Block key={id} threadId={threadId} blockId={id} isLastInTurn={i === turn.blocks.length - 1} turnActive={inProgress} />
        ))}
        {inProgress && !hasVisible && (
          <div className="flex items-center gap-2 py-1.5 text-[13px] text-fg2">
            <span className="flex gap-1">
              <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-purple [animation-delay:-0.2s]" />
              <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-purple [animation-delay:-0.1s]" />
              <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-purple" />
            </span>
            Модель загружается и читает контекст…
          </div>
        )}
        {failed && <ErrorBlock message={turn.error || "Ход завершился с ошибкой"} onRetry={() => sendPrompt(turn.prompt)} />}
        {interrupted && <div className="mt-1 flex items-center gap-1.5 text-[12px] text-fg3"><Square size={11} /> Остановлено</div>}
        {(turn.durationMs != null || turn.outputTokens > 0) && !inProgress && (
          <div className="mt-2 flex flex-wrap gap-x-3 border-t border-line-muted pt-1.5 text-[11px] text-fg3">
            {turn.durationMs != null && <span>{fmtDuration(turn.durationMs)}</span>}
            {turn.usage?.input_tokens != null && <span>вход {fmtTokens(turn.usage.input_tokens)}</span>}
            {turn.outputTokens > 0 && <span>выход {fmtTokens(turn.outputTokens)}</span>}
            {turn.tokPerSec && <span>⚡ {turn.tokPerSec.toFixed(0)} ток/с</span>}
            {turn.requests > 1 && <span>{turn.requests} запросов к модели</span>}
          </div>
        )}
        {inProgress && (
          <div className="mt-2 flex justify-end">
            <button onClick={interrupt} className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] px-2 py-0.5 text-[11px] text-fg3 hover:bg-hover hover:text-fg">
              <Square size={10} /> Стоп (Esc)
            </button>
          </div>
        )}
      </div>
    </div>
  );
});
