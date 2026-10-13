import { useEffect, useState } from "react";
import { RefreshCw, TerminalSquare } from "lucide-react";
import { useApp } from "../../store/app";
import { api } from "../../lib/tauri";
import { Button } from "../ui/primitives";

export function OllamaOffline() {
  const refresh = useApp((s) => s.refreshOllama);
  const url = useApp((s) => s.ollama?.url);
  const [checking, setChecking] = useState(false);
  return (
    <div className="flex h-full items-center justify-center bg-base px-6">
      <div className="anim-msg-in flex max-w-md flex-col items-center text-center">
        <div className="mb-3 h-10 w-10 rounded-full bg-[#8b949e]" />
        <h1 className="text-[20px] font-semibold">Ollama не запущена</h1>
        <p className="mt-2 text-[13px] text-fg2">
          CodeWhale Desktop работает только с локальной Ollama ({url || "http://127.0.0.1:11434"}). Запустите её из меню «Пуск» и
          нажмите «Проверить снова». Проверка идёт автоматически каждые 5 секунд.
        </p>
        <div className="mt-3 rounded-[var(--radius-sm)] border border-line bg-elevated px-3 py-1.5 font-mono text-[12px]">ollama serve</div>
        <Button
          variant="primary"
          className="mt-5"
          onClick={async () => {
            setChecking(true);
            await refresh();
            setChecking(false);
          }}
        >
          <RefreshCw size={14} className={checking ? "animate-spin" : ""} /> Проверить снова
        </Button>
      </div>
    </div>
  );
}

export function RuntimeFailed() {
  const runtime = useApp((s) => s.runtime);
  const setRuntime = useApp((s) => s.setRuntime);
  const setOverlay = useApp((s) => s.setOverlay);
  const [logs, setLogs] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.runtimeLogs().then(setLogs).catch(() => {});
  }, [runtime.error]);
  return (
    <div className="flex h-full items-center justify-center bg-base px-6">
      <div className="anim-msg-in flex w-full max-w-xl flex-col items-center text-center">
        <TerminalSquare size={36} className="mb-3 text-red" />
        <h1 className="text-[20px] font-semibold">CodeWhale не запустился</h1>
        <p className="mt-2 whitespace-pre-wrap text-[13px] text-fg2">{runtime.error}</p>
        {logs.length > 0 && (
          <pre className="mt-3 max-h-48 w-full overflow-auto rounded-[var(--radius-sm)] border border-line bg-elevated p-2 text-left font-mono text-[11px] text-fg3">
            {logs.slice(-15).join("\n")}
          </pre>
        )}
        <div className="mt-4 flex gap-2">
          <Button
            variant="primary"
            onClick={async () => {
              setBusy(true);
              try {
                setRuntime(await api.runtimeRestart());
              } catch {
                /* status event carries the error */
              }
              setBusy(false);
            }}
          >
            <RefreshCw size={14} className={busy ? "animate-spin" : ""} /> Перезапустить
          </Button>
          <Button onClick={() => setOverlay("settings")}>Указать путь к CodeWhale</Button>
        </div>
      </div>
    </div>
  );
}
