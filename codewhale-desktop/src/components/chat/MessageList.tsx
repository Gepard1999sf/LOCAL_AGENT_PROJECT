import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown } from "lucide-react";
import { useThreads } from "../../store/threads";
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

export function MessageList({ threadId }: { threadId: string }) {
  const count = useThreads((s) => s.threads[threadId]?.turns.length ?? 0);
  const live = useThreads((s) => s.threads[threadId]?.live ?? false);
  const streamState = useThreads((s) => s.threads[threadId]?.streamState);
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

  if (!live && count === 0 && streamState !== "error") return <Skeleton />;

  const items = virtualizer.getVirtualItems();
  return (
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
  );
}
