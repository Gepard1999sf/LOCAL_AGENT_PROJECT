import { useCallback, useEffect, useState } from "react";
import { BookOpen, Download, FolderOpen, HardDrive, Play, Plug, PlugZap, RefreshCw, Square, Wrench } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { api } from "../../lib/tauri";
import { driveApi, fmtBytes, type DriveStatus, type GoldenModel, type IndexProgress } from "../../lib/drive";
import { canonicalName, pullStatusLabel, sameModel, usePulls } from "../../lib/modelManager";
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
 * The removable "knowledge drive" (section 30 of the user's plan): shows what the drive
 * holds and drives the one-time setup — MCP registration, OLLAMA_MODELS, git_vault index,
 * kiwix-serve, skills and the golden model set.
 */
export function KnowledgeDrivePanel() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const models = useApp((s) => s.models);
  const refreshModels = useApp((s) => s.refreshModels);
  const setRuntime = useApp((s) => s.setRuntime);
  const pulls = usePulls((s) => s.pulls);
  const startPull = usePulls((s) => s.start);
  const [status, setStatus] = useState<DriveStatus | null>(null);
  const [golden, setGolden] = useState<GoldenModel[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<IndexProgress | null>(null);
  const [root, setRoot] = useState(settings?.knowledgeDrive?.root ?? "");

  const refresh = useCallback(async () => {
    try {
      setStatus(await driveApi.status());
    } catch (e) {
      toast.error("Не удалось прочитать состояние диска", { description: String(e) });
    }
  }, []);

  useEffect(() => {
    refresh();
    driveApi.goldenModels().then(setGolden).catch(() => {});
    refreshModels();
  }, [refresh, refreshModels]);
  useEffect(() => setRoot(settings?.knowledgeDrive?.root ?? ""), [settings?.knowledgeDrive?.root]);

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

  const saveRoot = async (value: string) => {
    const next = value.trim() || null;
    if (next === (settings?.knowledgeDrive?.root ?? null)) return;
    await update({ knowledgeDrive: { ...(settings?.knowledgeDrive ?? { kiwixPort: 8090 }), root: next } });
    await refresh();
  };

  const connect = () =>
    run(
      "mcp",
      async () => {
        const path = await driveApi.registerMcp();
        setRuntime(await api.runtimeRestart());
        return path;
      },
      "MCP-сервер ai-drive подключён, CodeWhale перезапущен",
    );
  const disconnect = () =>
    run(
      "mcp",
      async () => {
        await driveApi.unregisterMcp();
        setRuntime(await api.runtimeRestart());
      },
      "MCP-сервер ai-drive отключён",
    );
  const buildIndex = () =>
    run(
      "index",
      async () => {
        setProgress({ files: 0, bytes: 0, skipped: 0 });
        const s = await driveApi.buildIndex(setProgress);
        setProgress(null);
        return `${s.files} файлов, ${fmtBytes(s.bytes)} за ${Math.round(s.elapsedMs / 1000)} с`;
      },
      "Индекс git_vault построен",
    );

  const installed = (name: string) => models.some((m) => sameModel(m.name, name));
  const pullOf = (name: string) => pulls[canonicalName(name)] ?? pulls[name];

  return (
    <Panel title="Диск знаний" width={520}>
      <div className="px-4 py-3 text-[12px] text-fg2">
        Съёмный диск с оффлайн-документацией, исходниками, дампами StackExchange и моделями. Агент получает к нему доступ через MCP-сервер{" "}
        <span className="font-mono text-fg">ai-drive</span>, встроенный в это приложение.
      </div>

      <div className="px-4">
        <SectionTitle>Диск</SectionTitle>
        <div className="flex gap-1.5 py-1">
          <input className={inputCls} value={root} placeholder="D:\AI_Drive" onChange={(e) => setRoot(e.target.value)} onBlur={() => saveRoot(root)} aria-label="Папка диска" />
          <Button
            onClick={async () => {
              const dir = await openDialog({ directory: true, title: "Папка диска знаний" });
              if (typeof dir === "string") {
                setRoot(dir);
                await saveRoot(dir);
              }
            }}
          >
            <FolderOpen size={13} />
          </Button>
          <Button onClick={() => refresh()} aria-label="Обновить">
            <RefreshCw size={13} />
          </Button>
        </div>
        {status && (
          <>
            <Row ok={status.present} label={status.present ? "Диск подключён" : "Диск не найден — подключите его или укажите папку"} detail={status.root} />
            {status.sections.map((s) => (
              <Row key={s.key} ok={s.exists} label={`${s.key} — ${s.title}`} detail={s.exists ? `${s.entries} элементов` : "нет папки"} />
            ))}
            <div className="flex flex-wrap gap-1.5 py-2">
              <Button disabled={busy !== null || !status.present} onClick={() => run("helpers", driveApi.installHelpers, "Папки созданы, скрипты скопированы в bin")}>
                <Wrench size={13} /> Подготовить диск
              </Button>
              <Button
                disabled={!status.present}
                onClick={async () => {
                  try {
                    await revealItemInDir(status.root);
                  } catch (e) {
                    toast.error("Не удалось открыть папку", { description: String(e) });
                  }
                }}
              >
                <FolderOpen size={13} /> Открыть
              </Button>
            </div>
            <div className="pb-2 text-[11px] text-fg3">
              Первичное наполнение: скрипт <span className="font-mono">bin\init-ai-drive.ps1</span> (модели, репозитории, дампы Kiwix) — запускается в PowerShell, подробности в docs/KNOWLEDGE_DRIVE.md.
            </div>

            <SectionTitle>Подключение к агенту</SectionTitle>
            <Row ok={status.mcpRegistered} label={status.mcpRegistered ? "MCP-сервер ai-drive зарегистрирован" : "MCP-сервер не подключён"} detail={status.mcpPath} />
            <Row
              ok={status.skillsInstalled.length === 2}
              label={status.skillsInstalled.length ? `Скиллы установлены: ${status.skillsInstalled.join(", ")}` : "Скиллы knowledge-drive и math-physics-godot не установлены"}
            />
            <Row ok={status.godotFound} label={status.godotFound ? "Godot найден (godot_check работает)" : "Godot не настроен — укажите godot.exe в настройках"} detail={status.godotBin ?? undefined} />
            <Row ok={!!status.sympyHelper} label={status.sympyHelper ? "SymPy-помощник найден" : "SymPy-помощник не найден (нажмите «Подготовить диск»)"} detail={status.sympyHelper ?? undefined} />
            <div className="flex flex-wrap gap-1.5 py-2">
              {status.mcpRegistered ? (
                <Button disabled={busy !== null} onClick={disconnect}>
                  <PlugZap size={13} /> Отключить MCP
                </Button>
              ) : (
                <Button disabled={busy !== null} onClick={connect}>
                  <Plug size={13} /> Подключить MCP
                </Button>
              )}
              <Button disabled={busy !== null} onClick={() => run("skills", () => driveApi.installSkills(false), "Скиллы установлены в ~/.codewhale/skills")}>
                <BookOpen size={13} /> Установить скиллы
              </Button>
              <Button disabled={busy !== null || !settings?.workspace} onClick={() => run("skills", () => driveApi.installSkills(true), "Скиллы установлены в проект (.agents/skills)")}>
                <BookOpen size={13} /> …в проект
              </Button>
            </div>

            <SectionTitle>Модели Ollama на диске</SectionTitle>
            <Row
              ok={status.ollamaModelsOnDrive}
              label={status.ollamaModelsOnDrive ? "OLLAMA_MODELS указывает на диск" : status.ollamaModelsEnv ? "OLLAMA_MODELS указывает в другое место" : "OLLAMA_MODELS не задана (модели на системном диске)"}
              detail={status.ollamaModelsEnv ?? undefined}
            />
            <div className="flex flex-wrap gap-1.5 py-2">
              <Button disabled={busy !== null || !status.present} onClick={() => run("env", driveApi.setOllamaModels, "OLLAMA_MODELS сохранена для пользователя")}>
                <HardDrive size={13} /> OLLAMA_MODELS → диск
              </Button>
              <Button disabled={busy !== null} onClick={() => run("ollama", driveApi.restartOllama, "Ollama перезапущена")}>
                <RefreshCw size={13} /> Перезапустить Ollama
              </Button>
            </div>
            <div className="pb-1 text-[11px] text-fg3">Уже скачанные модели не переносятся автоматически: переместите папку models вручную или скачайте заново.</div>
            {golden.map((m) => {
              const p = pullOf(m.name);
              const has = installed(m.name);
              return (
                <div key={m.name} className="mb-1.5 rounded-[var(--radius-md)] border border-line px-3 py-2 text-[12px]">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-fg">{m.name}</span>
                    <span className="text-fg3">~{m.approxGb} ГБ</span>
                    <span className="ml-auto">
                      {has ? (
                        <span className="text-green">установлена</span>
                      ) : p && p.phase === "running" ? (
                        <span className="text-fg2">
                          {pullStatusLabel(p.status)} {p.overallTotal > 0 ? `${Math.round((p.overallCompleted / p.overallTotal) * 100)}%` : ""}
                        </span>
                      ) : (
                        <Button onClick={() => startPull(m.name, () => refreshModels())}>
                          <Download size={13} /> Скачать
                        </Button>
                      )}
                    </span>
                  </div>
                  <div className="text-fg2">{m.role}</div>
                  <div className="text-[11px] text-fg3">{m.note}</div>
                </div>
              );
            })}

            <SectionTitle>Поиск по исходникам (git_vault)</SectionTitle>
            <Row
              ok={!!status.index}
              label={status.index ? `Индекс построен: ${status.index.files ?? "?"} файлов, ${fmtBytes(status.index.bytes)}` : "Индекс не построен — code_search недоступен"}
              detail={status.index?.builtAt ?? undefined}
            />
            <div className="flex flex-wrap items-center gap-1.5 py-2">
              <Button disabled={busy !== null || status.indexRunning || !status.present} onClick={buildIndex}>
                <RefreshCw size={13} className={busy === "index" ? "animate-spin" : ""} /> {status.index ? "Перестроить индекс" : "Построить индекс"}
              </Button>
              {progress && (
                <span className="text-[12px] text-fg2">
                  {progress.files} файлов · {fmtBytes(progress.bytes)}
                </span>
              )}
            </div>

            <SectionTitle>Kiwix (StackOverflow, StackExchange)</SectionTitle>
            <Row
              ok={status.kiwixOnline ?? false}
              label={status.kiwixOnline ? `kiwix-serve отвечает на ${status.kiwixUrl}` : `kiwix-serve не запущен (${status.kiwixUrl})`}
              detail={status.zims.length ? `${status.zims.length} ZIM: ${status.zims.join(", ")}` : "в kiwix_zim нет файлов .zim"}
            />
            {status.kiwixBooks.length > 0 && <div className="pb-1 font-mono text-[11px] text-fg3">{status.kiwixBooks.map((b) => b.name).join(" · ")}</div>}
            <div className="flex flex-wrap gap-1.5 py-2 pb-4">
              {status.kiwixRunning ? (
                <Button disabled={busy !== null} onClick={() => run("kiwix", driveApi.kiwixStop, "kiwix-serve остановлен")}>
                  <Square size={13} /> Остановить Kiwix
                </Button>
              ) : (
                <Button disabled={busy !== null || status.zims.length === 0} onClick={() => run("kiwix", async () => (await driveApi.kiwixStart()).url, "kiwix-serve запущен")}>
                  <Play size={13} /> Запустить Kiwix
                </Button>
              )}
            </div>

            <div className="pb-4 text-[11px] text-fg3">
              MCP-сервер: <span className="font-mono">{status.exePath} mcp-drive</span>
              <br />
              Докcеты: {status.docsets.length ? status.docsets.join(", ") : "нет (положите папки *.docset в docsets)"}
            </div>
          </>
        )}
      </div>
    </Panel>
  );
}
