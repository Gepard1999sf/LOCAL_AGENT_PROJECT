import { Fragment, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, Code2, Copy, Maximize2, TriangleAlert, X, ZoomIn, ZoomOut } from "lucide-react";
import { toast } from "sonner";
import { IconButton } from "../ui/primitives";

// Mermaid is heavy (~1 MB with its diagram chunks), so it is loaded with a dynamic import
// the first time a finished answer contains a ```mermaid block. Rendering is serialized
// because mermaid keeps one global config (theme) and measures labels in a temporary
// element appended to <body>.

type Theme = "dark" | "light";
type MermaidApi = (typeof import("mermaid"))["default"];

export type MermaidResult =
  | { ok: true; id: string; svg: string; width: number; height: number }
  | { ok: false; error: string; loadFailed?: boolean };

type Rendered = Extract<MermaidResult, { ok: true }>;

const FALLBACK_FONT = '"Inter", "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';

// Appended to mermaid's own stylesheet, which it scopes under the SVG id. Neutralizes the
// chat's `.md` typography (paragraph margins, loose line height, `overflow-wrap: anywhere`)
// so HTML labels keep the size mermaid measured outside the message.
const THEME_CSS = "line-height:1.5;overflow-wrap:normal;p{margin:0;}";

// mermaid's "dark" theme paints pie slices and mind-map branches in near-black tones and the
// mind-map root grey on grey. Mid-tone accents with white labels stay readable on the chat
// background (cScale* also feeds timeline/journey/kanban sections).
const DARK_SCALE = ["#1f6feb", "#8957e5", "#238636", "#bd561d", "#da3633", "#1b7c83", "#bf4b8a", "#9e6a03", "#4f5bd5", "#5a7d12", "#0e7490", "#a0457a"];
const DARK_VARS: Record<string, string> = {
  ...Object.fromEntries(
    DARK_SCALE.flatMap((c, i) => [
      [`cScale${i}`, c],
      [`cScaleLabel${i}`, "#ffffff"],
      [`pie${i + 1}`, c],
    ]),
  ),
  pieStrokeColor: "#0d1117",
  pieOuterStrokeColor: "#30363d",
  pieSectionTextColor: "#ffffff",
  git0: "#1f6feb",
  gitBranchLabel0: "#ffffff",
};

let api: Promise<MermaidApi> | null = null;
let configured: string | null = null;
let queue: Promise<unknown> = Promise.resolve();
let seq = 0;
const cache = new Map<string, MermaidResult>();
const pending = new Map<string, Promise<MermaidResult>>();

const keyOf = (code: string, theme: Theme) => `${theme}\u0000${code}`;

function loadMermaid(): Promise<MermaidApi> {
  if (!api) {
    const p = import("mermaid").then((m) => m.default);
    // A failed chunk load is retried on the next diagram instead of being remembered.
    p.catch(() => {
      if (api === p) api = null;
    });
    api = p;
  }
  return api;
}

function uiFont(): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue("--font-ui").trim();
  return v || FALLBACK_FONT;
}

function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object") {
    const o = e as { message?: unknown; str?: unknown };
    if (typeof o.message === "string") return o.message;
    if (typeof o.str === "string") return o.str;
  }
  return String(e);
}

/** Natural size from the root <svg> viewBox (mermaid scales the element to 100% width). */
function svgSize(svg: string): { width: number; height: number } {
  const root = /<svg\b[^>]*>/i.exec(svg)?.[0] ?? "";
  const vb = /viewBox="([^"]+)"/i.exec(root)?.[1].trim().split(/[\s,]+/).map(Number);
  if (vb && vb.length === 4 && vb[2] > 0 && vb[3] > 0) return { width: vb[2], height: vb[3] };
  return { width: 0, height: 0 };
}

async function renderNow(code: string, theme: Theme): Promise<MermaidResult> {
  let mermaid: MermaidApi;
  try {
    mermaid = await loadMermaid();
  } catch (e) {
    return { ok: false, error: errorMessage(e), loadFailed: true };
  }
  const font = uiFont();
  const conf = `${theme}|${font}`;
  if (configured !== conf) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      // Throw instead of drawing mermaid's own "syntax error" SVG into the page.
      suppressErrorRendering: true,
      theme: theme === "dark" ? "dark" : "default",
      fontFamily: font,
      themeVariables: { fontFamily: font, ...(theme === "dark" ? DARK_VARS : {}) },
      themeCSS: THEME_CSS,
    });
    configured = conf;
  }
  const id = `cw-mermaid-${++seq}`;
  try {
    const { svg } = await mermaid.render(id, code);
    return { ok: true, id, svg, ...svgSize(svg) };
  } catch (e) {
    return { ok: false, error: errorMessage(e) };
  } finally {
    // mermaid removes its temporary container itself; this only guards against leftovers.
    document.getElementById(`d${id}`)?.remove();
    document.getElementById(`i${id}`)?.remove();
  }
}

/** Cached result for this source and theme, if it was rendered before (survives remounts). */
export function peekMermaid(code: string, theme: Theme): MermaidResult | null {
  return cache.get(keyOf(code, theme)) ?? null;
}

/** Renders a diagram to an SVG string. Never rejects: errors come back as `{ ok: false }`. */
export function renderMermaid(code: string, theme: Theme): Promise<MermaidResult> {
  const key = keyOf(code, theme);
  const hit = cache.get(key);
  if (hit) return Promise.resolve(hit);
  const running = pending.get(key);
  if (running) return running;
  const job = queue.then(() => renderNow(code, theme)).catch((e): MermaidResult => ({ ok: false, error: errorMessage(e) }));
  queue = job;
  pending.set(key, job);
  void job.then((r) => {
    pending.delete(key);
    if (!r.ok && r.loadFailed) return;
    if (cache.size >= 200) cache.clear();
    cache.set(key, r);
  });
  return job;
}

function readTheme(): Theme {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

function subscribeTheme(onChange: () => void): () => void {
  const obs = new MutationObserver(onChange);
  obs.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => obs.disconnect();
}

/** Follows `data-theme` on <html>, which useBootstrap sets from the settings (or the OS for "auto"). */
function useAppTheme(): Theme {
  return useSyncExternalStore(subscribeTheme, readTheme);
}

function ToolButton({ children, onClick, pressed }: { children: ReactNode; onClick: () => void; pressed?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={pressed}
      className={`inline-flex h-7 items-center gap-1 rounded-[var(--radius-sm)] px-2 text-fg2 hover:bg-hover hover:text-fg ${
        pressed ? "bg-hover text-fg" : ""
      }`}
    >
      {children}
    </button>
  );
}

/** The diagram source in Shiki's markup: line spans with the newlines between them. */
function SourceView({ code }: { code: string }) {
  const lines = code.split("\n");
  return (
    <div className={`code-body max-h-[520px] overflow-auto ${lines.length > 3 ? "numbered" : ""}`}>
      <pre className="shiki m-0 py-2.5">
        <code>
          {lines.map((l, i) => (
            <Fragment key={i}>
              <span className="line">{l}</span>
              {i < lines.length - 1 ? "\n" : null}
            </Fragment>
          ))}
        </code>
      </pre>
    </div>
  );
}

function useCopy(): [string | null, (key: string, text: string) => Promise<void>] {
  const [copied, setCopied] = useState<string | null>(null);
  const copy = useCallback(async (key: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      setTimeout(() => setCopied((k) => (k === key ? null : k)), 1200);
    } catch (e) {
      toast.error("Не удалось скопировать", { description: String(e) });
    }
  }, []);
  return [copied, copy];
}

/** The source as a code block: while mermaid loads and when the diagram cannot be drawn. */
function SourceBlock({ code }: { code: string }) {
  const [copied, copy] = useCopy();
  return (
    <div className="code-block my-2 overflow-hidden rounded-[var(--radius-md)] border border-line bg-[var(--code-bg)]" data-code={code}>
      <div className="flex h-8 items-center justify-between border-b border-line-muted bg-elevated pl-3 pr-1 text-[11px] text-fg3">
        <span className="font-mono">mermaid</span>
        <ToolButton onClick={() => copy("code", code)}>
          {copied ? <Check size={13} className="text-green" /> : <Copy size={13} />}
          <span>{copied ? "Скопировано" : "Копировать"}</span>
        </ToolButton>
      </div>
      <SourceView code={code} />
    </div>
  );
}

function ErrorView({ code, error, loadFailed }: { code: string; error: string; loadFailed?: boolean }) {
  const first = (error.split("\n").find((l) => l.trim()) ?? error).trim().replace(/:$/, "");
  const summary = first.length > 300 ? `${first.slice(0, 300)}…` : first;
  const more = error.trim() !== first;
  return (
    <div className="mermaid-error my-2">
      <div className="flex items-start gap-1.5 text-[12px] leading-snug text-red" role="status">
        <TriangleAlert size={13} className="mt-px shrink-0" />
        <span className="min-w-0">
          {loadFailed ? "Не удалось загрузить модуль схем" : "Схема не распознана"}: {summary}
        </span>
      </div>
      {more && (
        <details className="mt-0.5 text-[11px] text-fg3">
          <summary className="cursor-pointer select-none">Подробнее</summary>
          <pre className="m-0 mt-1 overflow-x-auto whitespace-pre font-mono text-[11px] leading-snug">{error.trim()}</pre>
        </details>
      )}
      <SourceBlock code={code} />
    </div>
  );
}

const ZOOM_MIN = 0.25;
const ZOOM_MAX = 4;

function DiagramOverlay({ result, onClose }: { result: Rendered; onClose: () => void }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  // A second copy of the SVG on the page: give its ids (styles, arrow markers) their own namespace.
  const svg = useMemo(() => result.svg.split(result.id).join(`${result.id}-full`), [result]);
  const [zoom, setZoom] = useState(() => {
    if (!result.width || !result.height) return 1;
    // Small diagrams are enlarged to fill the screen (up to 2x); big ones open at 100% with scrolling.
    const fit = Math.min((window.innerWidth - 96) / result.width, (window.innerHeight - 140) / result.height);
    return Math.min(2, Math.max(1, fit));
  });
  const step = (f: number) => setZoom((z) => Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z * f)));

  useEffect(() => {
    // Focus the dialog itself (a focused button would pop its tooltip) and give focus back on close.
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialogRef.current?.focus();
    // Capture phase on window: Esc must close the overlay and never reach the global
    // hotkeys (which interrupt a running turn).
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      before?.focus();
    };
  }, [onClose]);

  useLayoutEffect(() => {
    const el = hostRef.current?.querySelector("svg");
    if (!el) return;
    el.style.maxWidth = "none";
    if (result.width && result.height) {
      el.style.width = `${Math.round(result.width * zoom)}px`;
      el.style.height = `${Math.round(result.height * zoom)}px`;
    }
  }, [svg, zoom, result.width, result.height]);

  return createPortal(
    <div
      ref={dialogRef}
      tabIndex={-1}
      className="mermaid-overlay fixed inset-0 z-[100] flex flex-col bg-[color-mix(in_srgb,var(--bg-base)_94%,transparent)] outline-none"
      role="dialog"
      aria-modal="true"
      aria-label="Схема во весь размер"
    >
      <div className="flex h-11 shrink-0 items-center gap-1 border-b border-line bg-elevated px-3 text-[12px] text-fg2">
        <span className="mr-auto font-medium text-fg">Схема</span>
        <IconButton size="sm" label="Уменьшить" onClick={() => step(1 / 1.25)} disabled={zoom <= ZOOM_MIN}>
          <ZoomOut size={14} />
        </IconButton>
        <button
          type="button"
          onClick={() => setZoom(1)}
          title="Исходный размер"
          className="h-7 min-w-12 rounded-[var(--radius-sm)] px-1.5 font-mono text-[11px] text-fg2 hover:bg-hover hover:text-fg"
        >
          {Math.round(zoom * 100)}%
        </button>
        <IconButton size="sm" label="Увеличить" onClick={() => step(1.25)} disabled={zoom >= ZOOM_MAX}>
          <ZoomIn size={14} />
        </IconButton>
        <IconButton size="sm" label="Закрыть" onClick={onClose} className="ml-1">
          <X size={14} />
        </IconButton>
      </div>
      <div
        className="min-h-0 flex-1 overflow-auto p-6"
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
      >
        <div
          ref={hostRef}
          className="mx-auto w-max rounded-[var(--radius-md)] border border-line bg-[var(--code-bg)] p-4"
          style={{ lineHeight: 1.5, overflowWrap: "normal" }}
          dangerouslySetInnerHTML={{ __html: svg }}
        />
      </div>
    </div>,
    document.body,
  );
}

/**
 * A finished ```mermaid block: the rendered SVG with a toolbar (source toggle, copy SVG,
 * full-size view). Shows the source as a code block until the SVG is ready and, with an
 * error line, when mermaid cannot parse it.
 */
export const MermaidDiagram = memo(function MermaidDiagram({ code }: { code: string }) {
  const theme = useAppTheme();
  const [result, setResult] = useState<MermaidResult | null>(() => peekMermaid(code, theme));
  const [showCode, setShowCode] = useState(false);
  const [full, setFull] = useState(false);
  const [copied, copy] = useCopy();
  const closeFull = useCallback(() => setFull(false), []);

  useEffect(() => {
    const hit = peekMermaid(code, theme);
    if (hit) {
      setResult(hit);
      return;
    }
    // The previous result (e.g. the other theme) stays on screen until the new one is ready.
    let alive = true;
    void renderMermaid(code, theme).then((r) => {
      if (alive) setResult(r);
    });
    return () => {
      alive = false;
    };
  }, [code, theme]);

  if (!result) return <SourceBlock code={code} />;
  if (!result.ok) return <ErrorView code={code} error={result.error} loadFailed={result.loadFailed} />;

  return (
    <div
      className="code-block mermaid-block my-2 overflow-hidden rounded-[var(--radius-md)] border border-line bg-[var(--code-bg)]"
      data-code={code}
    >
      <div className="flex h-8 items-center justify-between border-b border-line-muted bg-elevated pl-3 pr-1 text-[11px] text-fg3">
        <span className="font-mono">mermaid</span>
        <div className="flex items-center gap-0.5">
          <ToolButton pressed={showCode} onClick={() => setShowCode((v) => !v)}>
            <Code2 size={13} />
            <span>Код</span>
          </ToolButton>
          {showCode && (
            <ToolButton onClick={() => copy("code", code)}>
              {copied === "code" ? <Check size={13} className="text-green" /> : <Copy size={13} />}
              <span>{copied === "code" ? "Скопировано" : "Копировать код"}</span>
            </ToolButton>
          )}
          <ToolButton onClick={() => copy("svg", result.svg)}>
            {copied === "svg" ? <Check size={13} className="text-green" /> : <Copy size={13} />}
            <span>{copied === "svg" ? "Скопировано" : "Копировать SVG"}</span>
          </ToolButton>
          <IconButton size="sm" label="Во весь размер" onClick={() => setFull(true)}>
            <Maximize2 size={13} />
          </IconButton>
        </div>
      </div>
      {showCode ? (
        <SourceView code={code} />
      ) : (
        <div
          className="mermaid-view flex justify-center overflow-x-auto p-3 [&>svg]:h-auto [&>svg]:max-w-full"
          style={{ lineHeight: 1.5, overflowWrap: "normal" }}
          onDoubleClick={() => setFull(true)}
          dangerouslySetInnerHTML={{ __html: result.svg }}
        />
      )}
      {full && <DiagramOverlay result={result} onClose={closeFull} />}
    </div>
  );
});
