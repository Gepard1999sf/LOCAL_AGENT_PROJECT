import { useCallback, useEffect, useState } from "react";
import { BookOpen, Cpu, FolderOpen, Play, Plug, PlugZap, RefreshCw, Square, Wand2, Zap } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { api } from "../../lib/tauri";
import { comfyApi, watchComfy, type ComfyProgress, type ComfyStatus } from "../../lib/comfy";
import { Button, Dot, SectionTitle } from "../ui/primitives";
import { Panel } from "../overlays/SidePanels";

const inputCls = "h-8 w-full rounded-[var(--radius-sm)] border border-line bg-base px-2 text-[13px] text-fg outline-none focus:border-blue";

function Row({ ok, label, detail }: { ok: boolean | null; label: React.ReactNode; detail?: React.ReactNode }) {
  return (
    <div className="flex items-start gap-2 py-1 text-[12.5px]">
      <span className="mt-1.5">
        <Dot color={ok === null ? "var(--fg-3)" : ok ? "var(--accent-green)" : "var(--accent-yellow)"} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-fg">{label}</span>
        {detail && <span className="block truncate font-mono text-[11px] text-fg3">{detail}</span>}
      </span>
    </div>
  );
}

/**
 * "Локальная фабрика ассетов": ComfyUI next to the coding agent on one 8 GB card. Shows who
 * holds the VRAM (Ollama model vs. a ComfyUI render), starts ComfyUI with the low-VRAM
 * flags, registers the `comfy` MCP server and installs the baked templates.
 */
export function AssetFactoryPanel() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const setRuntime = useApp((s) => s.setRuntime);
  const [status, setStatus] = useState<ComfyStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<ComfyProgress | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const [dir, setDir] = useState(settings?.comfy?.dir ?? "");
  const [python, setPython] = useState(settings?.comfy?.python ?? "");
  const [url, setUrl] = useState(settings?.comfy?.url ?? "http://127.0.0.1:8188");

  const refresh = useCallback(async () => {
    try {
      setStatus(await comfyApi.status());
    } catch (e) {
      toast.error("Не удалось опросить ComfyUI", { description: String(e) });
    }
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 5000);
    return () => clearInterval(t);
  }, [refresh]);
  useEffect(() => setDir(settings?.comfy?.dir ?? ""), [settings?.comfy?.dir]);
  useEffect(() => setPython(settings?.comfy?.python ?? ""), [settings?.comfy?.python]);
  useEffect(() => setUrl(settings?.comfy?.url ?? "http://127.0.0.1:8188"), [settings?.comfy?.url]);

  // Live progress and TAESD previews straight from ComfyUI's WebSocket.
  useEffect(() => {
    if (!status?.online) return;
    return watchComfy(status.url, { onProgress: setProgress, onPreview: setPreview });
  }, [status?.online, status?.url]);

  const patch = async (p: Partial<NonNullable<typeof settings>["comfy"]>) => {
    await update({ comfy: { ...(settings?.comfy ?? { url: "http://127.0.0.1:8188", dir: null, python: null, checkpoint: null, unloadLlm: true, extraArgs: null, workflowsDir: null }), ...p } });
    await refresh();
  };

  const run = async (key: string, fn: () => Promise<string | string[] | boolean | void>, okMsg?: string) => {
    setBusy(key);
    try {
      const r = await fn();
      if (okMsg) toast.success(okMsg, typeof r === "string" ? { description: r } : Array.isArray(r) ? { description: r.join("\n") } : undefined);
    } catch (e) {
      toast.error("Не получилось", { description: String(e) });
    } finally {
      setBusy(null);
      await refresh();
    }
  };

  const connect = () =>
    run(
      "mcp",
      async () => {
        const p = await comfyApi.registerMcp();
        setRuntime(await api.runtimeRestart());
        return p;
      },
      "MCP-сервер comfy подключён, CodeWhale перезапущен",
    );
  const disconnect = () =>
    run(
      "mcp",
      async () => {
        await comfyApi.unregisterMcp();
        setRuntime(await api.runtimeRestart());
      },
      "MCP-сервер comfy отключён",
    );

  const vramOwner = status?.queueRunning ? "comfy" : status?.ollamaLoaded.length ? "ollama" : "free";

  return (
    <Panel title="Фабрика ассетов" width={520}>
      <div className="px-4 py-3 text-[12px] text-fg2">
        ComfyUI рядом с агентом на одной видеокарте: перед рендером модель Ollama выгружается из VRAM, после — веса ComfyUI уходят в ОЗУ. Агент получает инструменты{" "}
        <span className="font-mono text-fg">generate_ui_icon</span>, <span className="font-mono text-fg">generate_pbr_material</span>, <span className="font-mono text-fg">generate_sprite_sheet</span>.
      </div>

      <div className="px-4">
        <SectionTitle>Видеопамять</SectionTitle>
        <div className="flex items-center gap-2 py-1 text-[13px]" data-testid="vram-owner">
          {vramOwner === "comfy" ? (
            <>
              <Wand2 size={14} className="text-yellow" /> VRAM: рендер в ComfyUI{progress ? ` (шаг ${progress.value}/${progress.max})` : "…"}
            </>
          ) : vramOwner === "ollama" ? (
            <>
              <Cpu size={14} className="text-blue" /> VRAM: занята CodeWhale ({status?.ollamaLoaded.join(", ")})
            </>
          ) : (
            <>
              <Zap size={14} className="text-green" /> VRAM: свободна
            </>
          )}
        </div>
        {status?.devices.map((d) => (
          <Row key={d.name} ok={true} label={d.name} detail={`свободно ${d.vramFreeMb} из ${d.vramTotalMb} МБ`} />
        ))}
        {preview && <img src={preview} alt="Предпросмотр рендера" className="my-2 max-h-48 rounded-[var(--radius-md)] border border-line" />}
        <div className="flex flex-wrap gap-1.5 py-2">
          <Button disabled={busy !== null} onClick={() => run("free", comfyApi.free, "VRAM освобождена")}>
            <Zap size={13} /> Освободить VRAM
          </Button>
        </div>

        <SectionTitle>ComfyUI</SectionTitle>
        {status && (
          <>
            <Row
              ok={status.online}
              label={status.online ? `ComfyUI ${status.version ?? ""} отвечает${status.startedByApp ? " (запущен приложением)" : ""}` : "ComfyUI не запущен"}
              detail={status.online ? `${status.url} · очередь: ${status.queueRunning} в работе, ${status.queuePending} ждут` : status.error ?? status.url}
            />
            <Row ok={status.dirValid} label={status.dirValid ? "Папка ComfyUI найдена (main.py)" : "Укажите папку ComfyUI с main.py"} detail={status.dir ?? undefined} />
          </>
        )}
        <div className="flex gap-1.5 py-1">
          <input className={inputCls} value={dir} placeholder="D:\\AI\\ComfyUI" aria-label="Папка ComfyUI" onChange={(e) => setDir(e.target.value)} onBlur={() => dir.trim() !== (settings?.comfy?.dir ?? "") && patch({ dir: dir.trim() || null })} />
          <Button
            onClick={async () => {
              const d = await openDialog({ directory: true, title: "Папка ComfyUI (где main.py)" });
              if (typeof d === "string") {
                setDir(d);
                await patch({ dir: d });
              }
            }}
          >
            <FolderOpen size={13} />
          </Button>
        </div>
        <div className="flex gap-1.5 py-1">
          <input className={inputCls} value={python} placeholder={status?.python ?? "python"} aria-label="Python для ComfyUI" onChange={(e) => setPython(e.target.value)} onBlur={() => python.trim() !== (settings?.comfy?.python ?? "") && patch({ python: python.trim() || null })} />
          <input className={inputCls} value={url} aria-label="Адрес ComfyUI" onChange={(e) => setUrl(e.target.value)} onBlur={() => url.trim() !== (settings?.comfy?.url ?? "") && patch({ url: url.trim() || "http://127.0.0.1:8188" })} />
        </div>
        <div className="flex flex-wrap gap-1.5 py-2">
          {status?.startedByApp ? (
            <Button disabled={busy !== null} onClick={() => run("comfy", comfyApi.stop, "ComfyUI остановлен")}>
              <Square size={13} /> Остановить ComfyUI
            </Button>
          ) : (
            <Button disabled={busy !== null || !status?.dirValid} onClick={() => run("comfy", comfyApi.start, "ComfyUI запущен")}>
              <Play size={13} className={busy === "comfy" ? "animate-spin" : ""} /> Запустить ComfyUI
            </Button>
          )}
          <Button onClick={() => refresh()} aria-label="Обновить">
            <RefreshCw size={13} />
          </Button>
        </div>
        <div className="pb-2 text-[11px] text-fg3">
          Запуск: <span className="font-mono">python main.py --listen 127.0.0.1 --port 8188 --lowvram --reserve-vram 1.0 --preview-method taesd --extra-model-paths-config …</span>
        </div>

        <SectionTitle>Модель и шаблоны</SectionTitle>
        <label className="block py-1.5">
          <span className="mb-1 block text-[12px] text-fg2">Чекпоинт для шаблонов ({"{{CHECKPOINT}}"})</span>
          <select className={inputCls} value={settings?.comfy?.checkpoint ?? ""} aria-label="Чекпоинт" onChange={(e) => patch({ checkpoint: e.target.value || null })}>
            <option value="">— не выбран —</option>
            {(status?.checkpoints ?? []).map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
            {settings?.comfy?.checkpoint && !(status?.checkpoints ?? []).includes(settings.comfy.checkpoint) && <option value={settings.comfy.checkpoint}>{settings.comfy.checkpoint}</option>}
          </select>
          <span className="mt-0.5 block text-[11px] text-fg3">Файлы из AI_ComfyModels\checkpoints на диске знаний и models\checkpoints ComfyUI</span>
        </label>
        {status && (
          <>
            <Row
              ok={status.templates.length > 0}
              label={status.templates.length ? `Шаблонов: ${status.templates.map((t) => t.name).join(", ")}` : "Шаблоны не установлены"}
              detail={status.workflowsDir}
            />
            <Row ok={status.mcpRegistered} label={status.mcpRegistered ? "MCP-сервер comfy зарегистрирован" : "MCP-сервер comfy не подключён"} detail={status.mcpPath} />
            <Row ok={status.skillInstalled} label={status.skillInstalled ? "Скилл comfyui-gamedev установлен" : "Скилл comfyui-gamedev не установлен"} />
          </>
        )}
        <div className="flex flex-wrap gap-1.5 py-2 pb-4">
          <Button disabled={busy !== null} onClick={() => run("templates", comfyApi.installTemplates, "Шаблоны, extra_model_paths.yaml и скилл установлены")}>
            <BookOpen size={13} /> Установить шаблоны
          </Button>
          {status?.mcpRegistered ? (
            <Button disabled={busy !== null} onClick={disconnect}>
              <PlugZap size={13} /> Отключить MCP
            </Button>
          ) : (
            <Button disabled={busy !== null} onClick={connect}>
              <Plug size={13} /> Подключить MCP
            </Button>
          )}
        </div>
        {status?.templates.map((t) => (
          <div key={t.name} className="mb-1.5 rounded-[var(--radius-md)] border border-line px-3 py-2 text-[12px]">
            <div className="font-mono text-fg">{t.name}</div>
            {t.description && <div className="text-fg2">{t.description}</div>}
            <div className="text-[11px] text-fg3">
              {t.nodeCount} нод · плейсхолдеры: {t.placeholders.join(", ") || "нет"}
            </div>
          </div>
        ))}
        <div className="pb-4 text-[11px] text-fg3">Свои шаблоны: сохраните граф в ComfyUI как API JSON, замените значения на {"{{ИМЯ}}"} и положите в папку шаблонов. Подробности — docs/COMFYUI.md.</div>
      </div>
    </Panel>
  );
}
