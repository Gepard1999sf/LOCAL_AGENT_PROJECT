import { memo, useEffect, useState } from "react";
import { Check, Copy, WrapText } from "lucide-react";
import { highlight } from "../../lib/highlight";
import { IconButton } from "../ui/primitives";
import { DiffView } from "./DiffView";

export const CodeBlock = memo(function CodeBlock({
  code,
  lang,
  streaming,
}: {
  code: string;
  lang?: string | null;
  streaming?: boolean;
}) {
  const [html, setHtml] = useState<string | null>(null);
  const [wrap, setWrap] = useState(false);
  const [copied, setCopied] = useState(false);
  const isDiff = lang === "diff" || lang === "patch";
  const lines = code.split("\n").length;

  useEffect(() => {
    if (isDiff) return;
    let alive = true;
    // While streaming, re-highlight at most every 250 ms to keep typing smooth.
    const t = setTimeout(
      () => {
        highlight(code, lang).then((h) => alive && setHtml(h));
      },
      streaming ? 250 : 0,
    );
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [code, lang, streaming, isDiff]);

  const copy = async () => {
    await navigator.clipboard.writeText(code);
    setCopied(true);
    setTimeout(() => setCopied(false), 1200);
  };

  return (
    <div className="code-block group/code my-2 overflow-hidden rounded-[var(--radius-md)] border border-line bg-[var(--code-bg)]" data-code={code}>
      <div className="flex h-8 items-center justify-between border-b border-line-muted bg-elevated pl-3 pr-1 text-[11px] text-fg3">
        <span className="font-mono">{lang || "text"}</span>
        <div className="flex items-center gap-0.5">
          {!isDiff && (
            <IconButton size="sm" label={wrap ? "Без переноса" : "Перенос строк"} active={wrap} onClick={() => setWrap((w) => !w)}>
              <WrapText size={13} />
            </IconButton>
          )}
          <button
            onClick={copy}
            className="inline-flex h-7 items-center gap-1 rounded-[var(--radius-sm)] px-2 text-fg2 hover:bg-hover hover:text-fg"
          >
            {copied ? <Check size={13} className="text-green" /> : <Copy size={13} />}
            <span>{copied ? "Скопировано" : "Копировать"}</span>
          </button>
        </div>
      </div>
      {isDiff ? (
        <DiffView text={code} />
      ) : (
        <div className={`code-body max-h-[520px] overflow-auto ${wrap ? "wrap" : ""} ${lines > 3 ? "numbered" : ""}`}>
          {html ? (
            <div dangerouslySetInnerHTML={{ __html: html }} />
          ) : (
            <pre className="shiki m-0 py-2.5">
              <code>
                {code.split("\n").map((l, i) => (
                  <span key={i} className="line">
                    {l}
                    {"\n"}
                  </span>
                ))}
              </code>
            </pre>
          )}
        </div>
      )}
    </div>
  );
});
