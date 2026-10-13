import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowUp, AtSign, FileText, Folder, ImagePlus, Paperclip, Slash, Square, X, Zap } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { useThreads } from "../../store/threads";
import { api } from "../../lib/tauri";
import { fuzzyScore } from "../../lib/format";
import { Tip } from "../ui/primitives";
import { exportThreadMarkdown } from "../../lib/export";

interface Attachment {
  mime: string;
  dataBase64: string;
  name: string;
}

interface Suggestion {
  key: string;
  label: string;
  hint?: string;
  icon: "file" | "dir" | "cmd";
  apply: () => void;
}

const HISTORY_KEY = "cw.inputHistory";

function loadHistory(): string[] {
  try {
    return JSON.parse(localStorage.getItem(HISTORY_KEY) ?? "[]");
  } catch {
    return [];
  }
}

function saveHistory(h: string[]) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(h.slice(-100)));
  } catch {
    /* storage unavailable */
  }
}

const IGNORED = /(^|\/)(node_modules|\.git|target|dist|\.codewhale|\.godot|__pycache__)\//;

export function Composer({ threadId }: { threadId: string | null }) {
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const [selIdx, setSelIdx] = useState(0);
  const [sending, setSending] = useState(false);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const histIdx = useRef<number | null>(null);
  const focusTick = useApp((s) => s.composerFocusTick);
  const sendPrompt = useApp((s) => s.sendPrompt);
  const interrupt = useApp((s) => s.interrupt);
  const newThread = useApp((s) => s.newThread);
  const setOverlay = useApp((s) => s.setOverlay);
  const runtimeState = useApp((s) => s.runtime.state);
  const models = useApp((s) => s.models);
  const defaultModel = useApp((s) => s.settings?.defaultModel);
  const active = useThreads((s) => (threadId ? !!s.threads[threadId]?.activeTurnId : false));
  const pendingApproval = useThreads((s) =>
    threadId ? Object.values(s.threads[threadId]?.approvals ?? {}).some((a) => a.status === "pending") : false,
  );

  const supportsVision = useMemo(() => {
    const m = models.find((x) => x.name === defaultModel);
    return !!m?.capabilities.includes("vision");
  }, [models, defaultModel]);

  useEffect(() => {
    taRef.current?.focus();
  }, [focusTick, threadId]);

  // Other components can put text into the composer (edit message, palette).
  useEffect(() => {
    const onSet = (e: Event) => {
      const v = (e as CustomEvent<string>).detail ?? "";
      setText(v);
      requestAnimationFrame(() => {
        const ta = taRef.current;
        if (ta) {
          ta.focus();
          ta.setSelectionRange(v.length, v.length);
        }
      });
    };
    window.addEventListener("composer:set", onSet);
    return () => window.removeEventListener("composer:set", onSet);
  }, []);

  // Auto-grow up to ~12 lines.
  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = Math.min(ta.scrollHeight, 280) + "px";
  }, [text]);

  const commands = useMemo(
    () => [
      { cmd: "/new", hint: "Новая сессия", run: () => newThread() },
      { cmd: "/clear", hint: "Начать с чистого листа (новая сессия)", run: () => newThread() },
      {
        cmd: "/compact",
        hint: "Сжать историю, чтобы освободить контекст",
        run: async () => {
          if (!threadId) return;
          await api.request("POST", `/v1/threads/${threadId}/compact`, {}).then(
            () => toast.success("Сжатие контекста запущено"),
            (e) => toast.error("Не удалось сжать контекст", { description: String(e) }),
          );
        },
      },
      { cmd: "/export", hint: "Экспорт в Markdown", run: () => threadId && exportThreadMarkdown(threadId) },
      { cmd: "/model", hint: "Сменить модель", run: () => setOverlay("models") },
      { cmd: "/tools", hint: "Инструменты и автоподтверждение", run: () => setOverlay("settings") },
      { cmd: "/settings", hint: "Настройки", run: () => setOverlay("settings") },
      { cmd: "/help", hint: "Горячие клавиши", run: () => setOverlay("shortcuts") },
    ],
    [threadId, newThread, setOverlay],
  );

  const replaceToken = useCallback((start: number, end: number, value: string) => {
    setText((t) => t.slice(0, start) + value + t.slice(end));
    setSuggestions([]);
    requestAnimationFrame(() => {
      const ta = taRef.current;
      if (ta) {
        const pos = start + value.length;
        ta.focus();
        ta.setSelectionRange(pos, pos);
      }
    });
  }, []);

  // Compute suggestions for "@path" and "/command" at the caret.
  const searchSeq = useRef(0);
  const updateSuggestions = useCallback(
    async (value: string, caret: number) => {
      const before = value.slice(0, caret);
      const at = /(^|\s)@([^\s@]*)$/.exec(before);
      const slash = /^\/(\S*)$/.exec(before);
      if (slash) {
        const q = slash[1];
        const list = commands
          .map((c) => ({ c, s: fuzzyScore(q, c.cmd.slice(1)) }))
          .filter((x) => x.s >= 0)
          .sort((a, b) => b.s - a.s)
          .map(({ c }) => ({
            key: c.cmd,
            label: c.cmd,
            hint: c.hint,
            icon: "cmd" as const,
            apply: () => {
              setText("");
              setSuggestions([]);
              c.run();
            },
          }));
        setSuggestions(list);
        setSelIdx(0);
        return;
      }
      if (at && runtimeState === "running") {
        const q = at[2];
        const start = caret - q.length - 1;
        const seq = ++searchSeq.current;
        try {
          const res = await api.request<{ paths: string[] }>(
            "GET",
            `/v1/workspace/files/search?query=${encodeURIComponent(q || ".")}&limit=60`,
          );
          if (seq !== searchSeq.current) return;
          const paths = (res.paths ?? []).filter((p) => !IGNORED.test(p + "/")).slice(0, 10);
          setSuggestions(
            paths.map((p) => ({
              key: p,
              label: p,
              icon: p.endsWith("/") ? "dir" : "file",
              apply: () => replaceToken(start, caret, `@${p} `),
            })),
          );
          setSelIdx(0);
        } catch {
          setSuggestions([]);
        }
        return;
      }
      setSuggestions([]);
    },
    [commands, replaceToken, runtimeState],
  );

  const submit = async () => {
    const value = text.trim();
    if ((!value && attachments.length === 0) || sending || active) return;
    if (value.startsWith("/")) {
      const cmd = commands.find((c) => c.cmd === value.split(/\s/)[0]);
      if (cmd) {
        setText("");
        cmd.run();
        return;
      }
    }
    setSending(true);
    const hist = loadHistory();
    if (value && hist.at(-1) !== value) saveHistory([...hist, value]);
    histIdx.current = null;
    try {
      await sendPrompt(value || "Опиши изображение.", attachments.map(({ mime, dataBase64 }) => ({ mime, dataBase64 })));
      setText("");
      setAttachments([]);
    } catch {
      /* toast already shown; keep the text so nothing is lost */
    } finally {
      setSending(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (suggestions.length) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelIdx((i) => (i + 1) % suggestions.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelIdx((i) => (i - 1 + suggestions.length) % suggestions.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
        e.preventDefault();
        suggestions[selIdx]?.apply();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setSuggestions([]);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
      return;
    }
    // ↑ on an empty input edits the last message; Ctrl+↑/↓ walks the input history.
    if (e.key === "ArrowUp" && (e.ctrlKey || text === "")) {
      const hist = loadHistory();
      if (!hist.length) return;
      e.preventDefault();
      const idx = histIdx.current == null ? hist.length - 1 : Math.max(0, histIdx.current - 1);
      histIdx.current = idx;
      setText(hist[idx]);
      return;
    }
    if (e.key === "ArrowDown" && e.ctrlKey) {
      const hist = loadHistory();
      if (histIdx.current == null) return;
      e.preventDefault();
      const idx = histIdx.current + 1;
      if (idx >= hist.length) {
        histIdx.current = null;
        setText("");
      } else {
        histIdx.current = idx;
        setText(hist[idx]);
      }
    }
  };

  const addImageFile = (file: File) => {
    if (file.size > 4 * 1024 * 1024) {
      toast.error("Изображение больше 4 МБ");
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result);
      const [, mime, data] = /^data:(.*?);base64,(.*)$/.exec(url) ?? [];
      if (data) setAttachments((a) => [...a, { mime, dataBase64: data, name: file.name || "изображение" }].slice(0, 10));
    };
    reader.readAsDataURL(file);
  };

  const attach = async () => {
    const picked = await openDialog({
      multiple: true,
      title: "Прикрепить файл или изображение",
      defaultPath: useApp.getState().settings?.workspace ?? undefined,
    });
    const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
    const ws = (useApp.getState().settings?.workspace ?? "").replace(/[\\/]+$/, "").toLowerCase();
    const mentions: string[] = [];
    for (const p of paths) {
      if (/\.(png|jpe?g|gif|webp)$/i.test(p)) {
        try {
          const img = await api.readImage(p);
          setAttachments((a) => [...a, { mime: img.mime, dataBase64: img.dataBase64, name: img.name ?? "изображение" }]);
        } catch (e) {
          toast.error("Не удалось прикрепить изображение", { description: String(e) });
        }
      } else {
        const rel = ws && p.toLowerCase().startsWith(ws) ? p.slice(ws.length + 1).replace(/\\/g, "/") : p;
        mentions.push(`@${rel}`);
      }
    }
    if (mentions.length) setText((t) => (t ? t.trimEnd() + " " : "") + mentions.join(" ") + " ");
    taRef.current?.focus();
  };

  const busy = active || sending;
  const placeholder =
    runtimeState !== "running"
      ? "CodeWhale запускается…"
      : pendingApproval
        ? "Агент ждёт вашего решения выше…"
        : "Напишите задачу… (Enter — отправить, Shift+Enter — новая строка, @ — файл, / — команды)";

  return (
    <div className="border-t border-line bg-base px-5 pb-3 pt-2">
      <div className="relative mx-auto w-full max-w-[920px]">
        {suggestions.length > 0 && (
          <div className="absolute bottom-full left-0 right-0 z-20 mb-2 overflow-hidden rounded-[var(--radius-md)] border border-line bg-overlay shadow-[var(--shadow-lg)]">
            {suggestions.map((s, i) => (
              <button
                key={s.key}
                onMouseDown={(e) => {
                  e.preventDefault();
                  s.apply();
                }}
                onMouseEnter={() => setSelIdx(i)}
                className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px] ${i === selIdx ? "bg-hover" : ""}`}
              >
                {s.icon === "cmd" ? (
                  <Slash size={13} className="text-fg3" />
                ) : s.icon === "dir" ? (
                  <Folder size={13} className="text-fg3" />
                ) : (
                  <FileText size={13} className="text-fg3" />
                )}
                <span className="font-mono">{s.label}</span>
                {s.hint && <span className="ml-auto text-[12px] text-fg3">{s.hint}</span>}
              </button>
            ))}
          </div>
        )}
        <div className="rounded-[var(--radius-md)] border border-line bg-elevated transition-colors focus-within:border-blue">
          {attachments.length > 0 && (
            <div className="flex flex-wrap gap-2 px-3 pt-2">
              {attachments.map((a, i) => (
                <div key={i} className="group/att relative h-14 w-14 overflow-hidden rounded-[var(--radius-sm)] border border-line">
                  <img src={`data:${a.mime};base64,${a.dataBase64}`} alt={a.name} className="h-full w-full object-cover" />
                  <button
                    className="absolute right-0.5 top-0.5 hidden rounded-full bg-black/70 p-0.5 text-white group-hover/att:block"
                    onClick={() => setAttachments((x) => x.filter((_, j) => j !== i))}
                    aria-label="Убрать"
                  >
                    <X size={11} />
                  </button>
                </div>
              ))}
              {!supportsVision && (
                <div className="self-center text-[11px] text-yellow">Текущая модель может не понимать изображения</div>
              )}
            </div>
          )}
          <textarea
            ref={taRef}
            value={text}
            rows={2}
            placeholder={placeholder}
            onChange={(e) => {
              setText(e.target.value);
              histIdx.current = null;
              updateSuggestions(e.target.value, e.target.selectionStart ?? e.target.value.length);
            }}
            onKeyDown={onKeyDown}
            onBlur={() => setTimeout(() => setSuggestions([]), 120)}
            onPaste={(e) => {
              const files = Array.from(e.clipboardData.files).filter((f) => f.type.startsWith("image/"));
              if (files.length) {
                e.preventDefault();
                files.forEach(addImageFile);
              }
            }}
            className="block max-h-[280px] min-h-[52px] w-full resize-none bg-transparent px-3.5 py-2.5 text-[14px] leading-[1.5] text-fg outline-none placeholder:text-fg3"
          />
          <div className="flex items-center gap-1 px-2 pb-2">
            <Tip label="Прикрепить файл или изображение">
              <button onClick={attach} className="inline-flex h-7 items-center gap-1 rounded-[var(--radius-sm)] px-2 text-[12px] text-fg2 hover:bg-hover hover:text-fg">
                <Paperclip size={13} /> Файл
              </button>
            </Tip>
            <Tip label="Упомянуть файл проекта">
              <button
                onClick={() => {
                  const ta = taRef.current;
                  const pos = ta?.selectionStart ?? text.length;
                  const next = text.slice(0, pos) + (pos > 0 && !/\s$/.test(text.slice(0, pos)) ? " @" : "@") + text.slice(pos);
                  setText(next);
                  requestAnimationFrame(() => {
                    ta?.focus();
                    const p = next.indexOf("@", pos) + 1;
                    ta?.setSelectionRange(p, p);
                    updateSuggestions(next, p);
                  });
                }}
                className="inline-flex h-7 items-center gap-1 rounded-[var(--radius-sm)] px-2 text-[12px] text-fg2 hover:bg-hover hover:text-fg"
              >
                <AtSign size={13} /> Файл проекта
              </button>
            </Tip>
            <Tip label="Быстрые команды">
              <button
                onClick={() => {
                  setText("/");
                  updateSuggestions("/", 1);
                  taRef.current?.focus();
                }}
                className="inline-flex h-7 items-center gap-1 rounded-[var(--radius-sm)] px-2 text-[12px] text-fg2 hover:bg-hover hover:text-fg"
              >
                <Zap size={13} /> Команды
              </button>
            </Tip>
            {supportsVision && (
              <Tip label="Вставьте изображение через Ctrl+V">
                <span className="inline-flex h-7 items-center px-1 text-fg3">
                  <ImagePlus size={13} />
                </span>
              </Tip>
            )}
            <span className="ml-auto mr-2 hidden text-[11px] text-fg3 sm:inline">Shift+Enter — новая строка</span>
            {busy && active ? (
              <button
                onClick={interrupt}
                className="inline-flex h-8 items-center gap-1.5 rounded-[var(--radius-sm)] bg-red px-3 text-[13px] font-medium text-white hover:brightness-110"
              >
                <Square size={12} /> Стоп
              </button>
            ) : (
              <button
                onClick={submit}
                disabled={(!text.trim() && attachments.length === 0) || busy || runtimeState !== "running"}
                className="inline-flex h-8 items-center gap-1.5 rounded-[var(--radius-sm)] bg-[#238636] px-3 text-[13px] font-medium text-white hover:brightness-110 disabled:opacity-40"
              >
                <ArrowUp size={14} /> Отправить
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
