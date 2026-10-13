import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { AlertTriangle, ArrowDown, RotateCcw, X } from "lucide-react";
import { useThreads } from "../../store/threads";
import { useApp } from "../../store/app";
import { Button } from "../ui/primitives";
import { TurnRow } from "./Turn";

function Skeleton() {
  return (
    <div className="mx-auto w-full max-w-[920px] space-y-6 px-5 py-6">
      {[0, 1, 2].map((i) => (
        <div key={i} className="space-y-2">
          <div className="ml-auto h-9 w-1/3 skeleton" />
          <div className="h-4 w-1/2 skeleton" />
          <div className="h-4 w-4/5 skeleton" />
          <div className="h-4 w-2/3 skeleton" />
        </div>
      ))}
    </div>
  );
}

/** The event stream ended for good (e.g. the thread is gone): say so instead of an empty list. */
function StreamError({ threadId, message, inline }: { threadId: string; message?: string; inline?: boolean }) {
  const reload = useApp((s) => s.reloadThread);
  const close = useApp((s) => s.openThread);
  const [busy, setBusy] = useState(false);
  const retry = async () => {
    setBusy(true);
    await reload(threadId);
    setBusy(false);
  };
  const text = message || "Поток событий сессии прервался";
  if (inline) {
    return (
      <div className="mx-auto flex w-full max-w-[920px] items-center gap-2 px-5 pt-3 text-[12.5px]">
        <div className="flex flex-1 items-center gap-2 rounded-[var(--radius-md)] border border-[color-mix(in_srgb,var(--accent-red)_45%,transparent)] bg-[color-mix(in_srgb,var(--accent-red)_8%,transparent)] px-3 py-1.5">
          <AlertTriangle size={14} className="shrink-0 text-red" />
          <span className="min-w-0 flex-1 truncate text-fg2">{text}. Новые события не приходят.</span>
          <Button disabled={busy} onClick={retry}>
            <RotateCcw size={13} className={busy ? "animate-spin" : ""} /> Повторить
          </Button>
        </div>
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-6">
      <div className="anim-msg-in flex max-w-md flex-col items-center text-center">
        <AlertTriangle size={32} className="mb-3 text-red" />
        <h2 className="text-[17px] font-semibold">Не удалось загрузить сессию</h2>
        <p className="mt-2 whitespace-pre-wrap text-[13px] text-fg2">{text}</p>
        <div className="mt-4 flex gap-2">
          <Button variant="primary" disabled={busy} onClick={retry}>
            <RotateCcw size={14} className={busy ? "animate-spin" : ""} /> Повторить
          </Button>
          <Button onClick={() => close(null)}>
            <X size={14} /> Закрыть
          </Button>
        </div>
      </div>
    </div>
  );
}

export function MessageList({ threadId }: { threadId: string }) {
  const count = useThreads((s) => s.threads[threadId]?.turns.length ?? 0);
  const live = useThreads((s) => s.threads[threadId]?.live ?? false);
  const streamState = useThreads((s) => s.threads[threadId]?.streamState);
  const lastError = useThreads((s) => s.threads[threadId]?.lastError);
  // A cheap signal that changes whenever the last turn grows.
  const tailSig = useThreads((s) => {
    const t = s.threads[threadId];
    const last = t?.turns.at(-1);
    if (!last) return "";
    const lastBlock = last.blocks.at(-1);
    const item = lastBlock ? t!.items[lastBlock] : undefined;
    return `${last.id}:${last.blocks.length}:${item?.text.length ?? 0}:${last.status}`;
  });

  const parentRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const [showJump, setShowJump] = useState(false);

  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 260,
    overscan: 4,
    getItemKey: (i) => `${threadId}:${i}`,
  });

  const scrollToEnd = useCallback((smooth = false) => {
    const el = parentRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? "smooth" : "auto" });
  }, []);

  const onScroll = () => {
    const el = parentRef.current;
    if (!el) return;
    const near = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    atBottom.current = near;
    setShowJump(!near);
  };

  // Jump to the end when a thread opens or history finishes replaying.
  useLayoutEffect(() => {
    atBottom.current = true;
    requestAnimationFrame(() => scrollToEnd());
  }, [threadId, live, scrollToEnd]);

  // Follow streaming output only while the user stays at the bottom.
  useEffect(() => {
    if (!atBottom.current) return;
    const id = requestAnimationFrame(() => scrollToEnd());
    return () => cancelAnimationFrame(id);
  }, [tailSig, count, scrollToEnd]);

  const failed = streamState === "error";
  if (failed && count === 0) return <StreamError threadId={threadId} message={lastError} />;
  if (!live && count === 0) return <Skeleton />;

  const items = virtualizer.getVirtualItems();
  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      {failed && <StreamError threadId={threadId} message={lastError} inline />}
      <div className="relative min-h-0 flex-1">
        <div ref={parentRef} onScroll={onScroll} className="h-full overflow-y-auto" style={{ contain: "strict" }}>
          <div style={{ height: virtualizer.getTotalSize(), position: "relative", width: "100%" }}>
            <div style={{ position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${items[0]?.start ?? 0}px)` }}>
              {items.map((v) => (
                <div key={v.key} data-index={v.index} ref={virtualizer.measureElement}>
                  <TurnRow threadId={threadId} index={v.index} />
                </div>
              ))}
            </div>
          </div>
          <div className="h-4" />
        </div>
        {showJump && (
          <button
            onClick={() => {
              atBottom.current = true;
              scrollToEnd(true);
            }}
            className="absolute bottom-4 left-1/2 inline-flex -translate-x-1/2 items-center gap-1 rounded-full border border-line bg-overlay px-3 py-1 text-[12px] text-fg2 shadow-[var(--shadow-md)] hover:text-fg"
          >
            <ArrowDown size={13} /> Вниз
          </button>
        )}
      </div>
    </div>
  );
}
