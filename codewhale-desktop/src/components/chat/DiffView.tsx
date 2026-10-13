import { memo, useMemo, useState } from "react";

export interface DiffLine {
  type: "add" | "del" | "ctx" | "hunk" | "file";
  text: string;
  oldNo?: number;
  newNo?: number;
}

export function parseUnifiedDiff(text: string): DiffLine[] {
  const out: DiffLine[] = [];
  const src = text.replace(/\r\n/g, "\n").split("\n");
  if (src.at(-1) === "") src.pop();
  // Line numbers are known only after a parsable @@ header.
  let oldNo: number | undefined;
  let newNo: number | undefined;
  // Lines the current hunk still holds, from its @@ counts. "--- "/"+++ " are file
  // headers only outside a hunk: inside one, "--- x" is the deleted line "-- x".
  let inHunk = false;
  let oldLeft = 0;
  let newLeft = 0;
  // Between "diff --git" and the first @@ (mode, rename, "Binary files differ" lines).
  let gitHeader = false;
  for (let i = 0; i < src.length; i++) {
    const raw = src[i];
    if (raw.startsWith("@@")) {
      const m = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(raw);
      oldNo = m ? Number(m[1]) : undefined;
      newNo = m ? Number(m[3]) : undefined;
      // An omitted count means 1; without counts the hunk runs until the next file.
      oldLeft = m ? Number(m[2] ?? 1) : Infinity;
      newLeft = m ? Number(m[4] ?? 1) : Infinity;
      inHunk = true;
      gitHeader = false;
      out.push({ type: "hunk", text: raw });
      continue;
    }
    const headerPos = !inHunk || (oldLeft <= 0 && newLeft <= 0);
    // A "--- / +++ / @@" triple starts the next file even after a hunk with wrong counts.
    const nextFile = raw.startsWith("--- ") && src[i + 1]?.startsWith("+++ ") && src[i + 2]?.startsWith("@@");
    if (raw.startsWith("diff ")) gitHeader = true;
    if (
      gitHeader ||
      nextFile ||
      (headerPos && (raw.startsWith("--- ") || raw.startsWith("+++ ") || raw.startsWith("index ")))
    ) {
      inHunk = false;
      out.push({ type: "file", text: raw });
    } else if (raw.startsWith("\\")) {
      // "\ No newline at end of file"
      out.push({ type: "file", text: raw });
    } else if (raw.startsWith("+")) {
      out.push({ type: "add", text: raw.slice(1), newNo: newNo === undefined ? undefined : newNo++ });
      newLeft--;
    } else if (raw.startsWith("-")) {
      out.push({ type: "del", text: raw.slice(1), oldNo: oldNo === undefined ? undefined : oldNo++ });
      oldLeft--;
    } else {
      out.push({
        type: "ctx",
        text: raw.startsWith(" ") ? raw.slice(1) : raw,
        oldNo: oldNo === undefined ? undefined : oldNo++,
        newNo: newNo === undefined ? undefined : newNo++,
      });
      oldLeft--;
      newLeft--;
    }
  }
  return out;
}

function countStats(lines: DiffLine[]): { add: number; del: number } {
  let add = 0;
  let del = 0;
  for (const l of lines) {
    if (l.type === "add") add++;
    else if (l.type === "del") del++;
  }
  return { add, del };
}

export function diffStats(text: string): { add: number; del: number } {
  return countStats(parseUnifiedDiff(text));
}

export function looksLikeDiff(text: string): boolean {
  return /^@@ -\d+/m.test(text) || (/^--- /m.test(text) && /^\+\+\+ /m.test(text));
}

/**
 * Unified or side-by-side diff renderer (replaces react-diff-viewer, which is unmaintained).
 * Takes unified diff `text`, or ready `lines` when there is no real diff to parse.
 */
export const DiffView = memo(function DiffView({ text, lines: given, maxHeight = 480 }: { text?: string; lines?: DiffLine[]; maxHeight?: number }) {
  const lines = useMemo(() => given ?? parseUnifiedDiff(text ?? ""), [given, text]);
  const [split, setSplit] = useState(false);
  const stats = useMemo(() => countStats(lines), [lines]);

  const rows = useMemo(() => {
    if (!split) return null;
    // Pair deletions with additions inside each change block.
    const res: Array<[DiffLine | null, DiffLine | null]> = [];
    let dels: DiffLine[] = [];
    let adds: DiffLine[] = [];
    const flushBlock = () => {
      const n = Math.max(dels.length, adds.length);
      for (let i = 0; i < n; i++) res.push([dels[i] ?? null, adds[i] ?? null]);
      dels = [];
      adds = [];
    };
    for (const l of lines) {
      if (l.type === "del") dels.push(l);
      else if (l.type === "add") adds.push(l);
      else {
        flushBlock();
        res.push([l, l]);
      }
    }
    flushBlock();
    return res;
  }, [lines, split]);

  const cls = (t?: DiffLine["type"]) =>
    t === "add"
      ? "bg-[var(--diff-add-bg)] text-[var(--diff-add-fg)]"
      : t === "del"
        ? "bg-[var(--diff-del-bg)] text-[var(--diff-del-fg)]"
        : t === "hunk"
          ? "bg-[color-mix(in_srgb,var(--accent-blue)_10%,transparent)] text-fg3"
          : t === "file"
            ? "text-fg3"
            : "text-fg";

  return (
    <div className="font-mono text-[12px] leading-[1.55]">
      <div className="flex items-center justify-between border-b border-line-muted px-3 py-1 text-[11px]">
        <span>
          <span className="text-[var(--diff-add-fg)]">+{stats.add}</span> <span className="text-[var(--diff-del-fg)]">−{stats.del}</span>
        </span>
        <button className="text-fg3 hover:text-fg" onClick={() => setSplit((s) => !s)}>
          {split ? "Единый вид" : "Бок о бок"}
        </button>
      </div>
      <div className="overflow-auto" style={{ maxHeight }}>
        {!split ? (
          <table className="w-full border-collapse">
            <tbody>
              {lines.map((l, i) => (
                <tr key={i} className={cls(l.type)}>
                  <td className="w-10 select-none pr-1 text-right align-top text-fg3 opacity-70">{l.oldNo ?? ""}</td>
                  <td className="w-10 select-none pr-2 text-right align-top text-fg3 opacity-70">{l.newNo ?? ""}</td>
                  <td className="w-4 select-none align-top opacity-80">{l.type === "add" ? "+" : l.type === "del" ? "−" : ""}</td>
                  <td className="whitespace-pre pr-3">{l.text}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <table className="w-full table-fixed border-collapse">
            <tbody>
              {rows!.map(([a, b], i) => (
                <tr key={i}>
                  <td className={`w-10 select-none pr-1 text-right align-top text-fg3 ${cls(a?.type)}`}>{a?.oldNo ?? ""}</td>
                  <td className={`overflow-hidden whitespace-pre pr-2 ${cls(a?.type === "add" ? "ctx" : a?.type)}`}>{a && a.type !== "add" ? a.text : ""}</td>
                  <td className={`w-10 select-none border-l border-line-muted pr-1 text-right align-top text-fg3 ${cls(b?.type)}`}>{b?.newNo ?? ""}</td>
                  <td className={`overflow-hidden whitespace-pre pr-2 ${cls(b?.type === "del" ? "ctx" : b?.type)}`}>{b && b.type !== "del" ? b.text : ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
});
