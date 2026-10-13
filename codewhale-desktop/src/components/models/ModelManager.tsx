import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Tabs } from "radix-ui";
import {
  Brain,
  Check,
  Copy,
  Cpu,
  Download,
  ExternalLink,
  Eye,
  HardDriveDownload,
  Info,
  LoaderCircle,
  Power,
  RefreshCw,
  Search,
  Trash2,
  TriangleAlert,
  Wrench,
  X,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { useApp } from "../../store/app";
import { api } from "../../lib/tauri";
import { fmtBytes, fmtTokens } from "../../lib/format";
import type { OllamaModel } from "../../lib/types";
import { Button, IconButton } from "../ui/primitives";
import {
  canonicalName,
  isValidModelName,
  modelsApi,
  pullStatusLabel,
  sameModel,
  usePulls,
  type ModelDetails,
  type PullEntry,
  type RunningModel,
  type VramEstimate,
} from "../../lib/modelManager";

// ---------------------------------------------------------------------------------------------
// Curated models. The Ollama API has no library search, so this list ships with the app.
// Sizes are the download size of the default (Q4_K_M / MXFP4) tag and may drift over time.
// ---------------------------------------------------------------------------------------------

export interface RecommendedModel {
  name: string;
  title: string;
  /** Approximate download size, GB. */
  sizeGb: number;
  /** Native context window, tokens. */
  ctx: number;
  caps: Array<"tools" | "thinking" | "vision">;
  note: string;
}

/** Рекомендации, проверьте актуальность: теги и размеры в библиотеке Ollama меняются. */
export const RECOMMENDED_MODELS: RecommendedModel[] = [
  { name: "qwen3:1.7b", title: "Qwen3 1.7B", sizeGb: 1.4, ctx: 40_960, caps: ["tools", "thinking"], note: "Для слабых видеокарт и CPU. Простые правки." },
  { name: "phi4-mini:3.8b", title: "Phi-4 mini", sizeGb: 2.5, ctx: 131_072, caps: ["tools"], note: "Компактная модель Microsoft с вызовом инструментов." },
  {
    name: "qwen3:4b-instruct-2507-q4_K_M",
    title: "Qwen3 4B Instruct 2507",
    sizeGb: 2.5,
    ctx: 262_144,
    caps: ["tools"],
    note: "Лучший баланс для 6–8 ГБ VRAM: быстрая, уверенно вызывает инструменты.",
  },
  { name: "granite4:tiny-h", title: "Granite 4 Tiny (гибрид)", sizeGb: 4.2, ctx: 131_072, caps: ["tools"], note: "Гибрид Mamba/Transformer: KV-кэш почти не растёт с контекстом." },
  { name: "qwen2.5-coder:7b", title: "Qwen2.5 Coder 7B", sizeGb: 4.7, ctx: 32_768, caps: ["tools"], note: "Специализирована на коде, хорошо дополняет и правит." },
  { name: "llama3.1:8b", title: "Llama 3.1 8B", sizeGb: 4.9, ctx: 131_072, caps: ["tools"], note: "Универсальная модель с длинным контекстом." },
  { name: "qwen3:8b", title: "Qwen3 8B", sizeGb: 5.2, ctx: 40_960, caps: ["tools", "thinking"], note: "Умнее 4B, умеет рассуждать. Нужна карта от 8 ГБ." },
  { name: "deepseek-r1:8b", title: "DeepSeek-R1 8B", sizeGb: 5.2, ctx: 131_072, caps: ["tools", "thinking"], note: "Рассуждающая модель: медленнее, но точнее в сложных задачах." },
  { name: "qwen2.5-coder:14b", title: "Qwen2.5 Coder 14B", sizeGb: 9.0, ctx: 32_768, caps: ["tools"], note: "Заметно сильнее 7B. Для карт от 12 ГБ." },
  { name: "qwen3:14b", title: "Qwen3 14B", sizeGb: 9.3, ctx: 40_960, caps: ["tools", "thinking"], note: "Сильная универсальная модель для 12–16 ГБ VRAM." },
  { name: "gpt-oss:20b", title: "gpt-oss 20B", sizeGb: 14, ctx: 131_072, caps: ["tools", "thinking"], note: "MoE от OpenAI: быстрая для своего размера. Для 16 ГБ VRAM." },
  { name: "devstral:24b", title: "Devstral 24B", sizeGb: 14, ctx: 131_072, caps: ["tools"], note: "Mistral для агентной работы с кодом. Для 16–24 ГБ VRAM." },
  { name: "mistral-small3.2:24b", title: "Mistral Small 3.2", sizeGb: 15, ctx: 131_072, caps: ["tools", "vision"], note: "Понимает изображения и вызывает инструменты." },
  { name: "qwen3-coder:30b", title: "Qwen3 Coder 30B-A3B", sizeGb: 19, ctx: 262_144, caps: ["tools"], note: "MoE для кода: терпимо работает даже частично на CPU. Лучше от 24 ГБ." },
];

const CTX_CHOICES = [4096, 8192, 16384, 24576, 32768, 49152, 65536, 98304, 131072, 262144];
const GIB = 1024 ** 3;

function openLink(url: string) {
  openUrl(url).catch((e) => toast.error("Не удалось открыть ссылку", { description: String(e) }));
}

function errText(e: unknown): string {
  return typeof e === "string" ? e : (e as any)?.message ?? String(e);
}

/** Ollama's default context when the Modelfile sets none (VRAM tiers of Ollama 0.12+). */
function ollamaDefaultCtx(gpuTotal: number | null): number {
  if (gpuTotal && gpuTotal >= 47 * GIB) return 262144;
  if (gpuTotal && gpuTotal >= 23 * GIB) return 32768;
  return 4096;
}

function runCtx(m: OllamaModel, gpuTotal: number | null): { ctx: number; isDefault: boolean } {
  const isDefault = !m.numCtx;
  const ctx = m.numCtx ?? ollamaDefaultCtx(gpuTotal);
  return { ctx: m.nativeCtx ? Math.min(ctx, m.nativeCtx) : ctx, isDefault };
}

function fmtEta(sec: number): string {
  if (!Number.isFinite(sec) || sec <= 0) return "";
  if (sec < 60) return `~${Math.ceil(sec)} с`;
  const min = Math.round(sec / 60);
  if (min < 60) return `~${min} мин`;
  return `~${Math.floor(min / 60)} ч ${min % 60} мин`;
}

function fmtParams(n?: number | null): string | null {
  if (!n) return null;
  return n >= 1e9 ? `${(n / 1e9).toFixed(2)} млрд` : `${Math.round(n / 1e6)} млн`;
}

// ---------------------------------------------------------------------------------------------
// VRAM fit
// ---------------------------------------------------------------------------------------------

type Fit = "ok" | "tight" | "over" | "unknown";

/** Leaves ~15% of VRAM for Windows, the browser and other apps. */
function fitOf(total: number, gpuTotal: number | null): Fit {
  if (!gpuTotal) return "unknown";
  if (total <= gpuTotal * 0.85) return "ok";
  if (total <= gpuTotal) return "tight";
  return "over";
}

const FIT_STYLE: Record<Fit, string> = {
  ok: "text-green border-[color-mix(in_srgb,var(--accent-green)_40%,transparent)]",
  tight: "text-yellow border-[color-mix(in_srgb,var(--accent-yellow)_40%,transparent)]",
  over: "text-red border-[color-mix(in_srgb,var(--accent-red)_40%,transparent)]",
  unknown: "text-fg3 border-line",
};

const FIT_HINT: Record<Fit, string> = {
  ok: "Помещается в видеопамять целиком",
  tight: "Впритык: может не хватить, если видеопамять занята другими программами",
  over: "Не помещается: часть слоёв уйдёт на CPU, генерация станет заметно медленнее",
  unknown: "Объём видеопамяти неизвестен (нужна карта NVIDIA с nvidia-smi)",
};

function FitChip({ total, gpuTotal, prefix = "VRAM ≈", title }: { total: number; gpuTotal: number | null; prefix?: string; title?: string }) {
  const fit = fitOf(total, gpuTotal);
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-[var(--radius-sm)] border px-1.5 text-[11px] leading-[18px] ${FIT_STYLE[fit]}`}
      title={title ?? FIT_HINT[fit]}
    >
      {fit === "over" && <TriangleAlert size={11} />}
      {prefix ? `${prefix} ` : ""}
      {fmtBytes(total)}
      {gpuTotal ? <span className="text-fg3">/ {fmtBytes(gpuTotal)}</span> : null}
    </span>
  );
}

// Estimates are cached per model version and context; at most 3 are computed at a time.
const estimateCache = new Map<string, Promise<VramEstimate>>();
let estimatesRunning = 0;
const estimateQueue: Array<() => void> = [];

function queued<T>(task: () => Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const run = () => {
      estimatesRunning++;
      task()
        .then(resolve, reject)
        .finally(() => {
          estimatesRunning--;
          estimateQueue.shift()?.();
        });
    };
    if (estimatesRunning < 3) run();
    else estimateQueue.push(run);
  });
}

function getEstimate(name: string, ctx: number, version: string): Promise<VramEstimate> {
  const key = `${version}|${name}|${ctx}`;
  let p = estimateCache.get(key);
  if (!p) {
    p = queued(() => modelsApi.estimateVram(name, ctx));
    p.catch(() => estimateCache.delete(key));
    estimateCache.set(key, p);
  }
  return p;
}

function useEstimate(name: string | null, ctx: number, version: string) {
  const key = name ? `${version}|${name}|${ctx}` : "";
  const [state, setState] = useState<{ key: string; data?: VramEstimate; error?: string }>({ key: "" });
  useEffect(() => {
    if (!name) return;
    let alive = true;
    getEstimate(name, ctx, version).then(
      (data) => alive && setState({ key, data }),
      (e) => alive && setState({ key, error: errText(e) }),
    );
    return () => {
      alive = false;
    };
  }, [key, name, ctx, version]);
  return state.key === key ? state : { key, data: undefined, error: undefined };
}

// ---------------------------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------------------------

function CapBadges({ caps }: { caps: string[] }) {
  const items: Array<[string, ReactNode, string]> = [
    ["tools", <Wrench size={11} />, "инструменты"],
    ["thinking", <Brain size={11} />, "рассуждения"],
    ["vision", <Eye size={11} />, "изображения"],
    ["embedding", null, "эмбеддинги"],
  ];
  return (
    <span className="flex flex-wrap items-center gap-1">
      {items
        .filter(([c]) => caps.includes(c))
        .map(([c, icon, label]) => (
          <span
            key={c}
            className={`inline-flex items-center gap-1 rounded-[var(--radius-sm)] bg-overlay px-1.5 text-[10.5px] leading-[17px] ${
              c === "tools" ? "text-bluefg" : c === "thinking" ? "text-purple" : "text-fg2"
            }`}
          >
            {icon}
            {label}
          </span>
        ))}
    </span>
  );
}

function ProgressBar({ value, tone = "blue" }: { value: number | null; tone?: "blue" | "green" | "red" | "yellow" }) {
  const color = { blue: "bg-blue", green: "bg-green", red: "bg-red", yellow: "bg-yellow" }[tone];
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-overlay">
      {value == null ? (
        <div className={`h-full w-1/3 animate-pulse rounded-full ${color}`} />
      ) : (
        <div className={`h-full rounded-full transition-[width] duration-200 ${color}`} style={{ width: `${Math.min(100, Math.max(0, value * 100))}%` }} />
      )}
    </div>
  );
}

const inputCls =
  "h-7 min-w-0 rounded-[var(--radius-sm)] border border-line bg-base px-2 text-[12px] text-fg outline-none placeholder:text-fg3 focus:border-blue";
const smallBtn = "px-2 py-0.5 text-[12px]";

function CtxSelect({ value, onChange, max, min = 2048 }: { value: number; onChange: (v: number) => void; max?: number | null; min?: number }) {
  const options = CTX_CHOICES.filter((c) => c >= min && (!max || c <= max));
  if (!options.includes(value)) options.push(value);
  options.sort((a, b) => a - b);
  return (
    <select className={`${inputCls} px-1.5`} value={value} onChange={(e) => onChange(Number(e.target.value))}>
      {options.map((c) => (
        <option key={c} value={c}>
          {fmtTokens(c)}
        </option>
      ))}
    </select>
  );
}

function useGpuTotal(): number | null {
  const gpu = useApp((s) => s.gpu);
  return gpu?.memTotalMb ? gpu.memTotalMb * 1024 * 1024 : null;
}

// ---------------------------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------------------------

export type ModelManagerTab = "installed" | "pull" | "running";

export function ModelManager({ defaultTab = "installed" }: { defaultTab?: ModelManagerTab }) {
  const [tab, setTab] = useState<ModelManagerTab>(defaultTab);
  const models = useApp((s) => s.models);
  const ollama = useApp((s) => s.ollama);
  const gpu = useApp((s) => s.gpu);
  const refreshModels = useApp((s) => s.refreshModels);
  const refreshOllama = useApp((s) => s.refreshOllama);
  const refreshGpu = useApp((s) => s.refreshGpu);
  const pulls = usePulls((s) => s.pulls);
  const syncActive = usePulls((s) => s.syncActive);
  const activePulls = Object.values(pulls).filter((p) => p.phase === "running").length;

  useEffect(() => {
    refreshModels();
    refreshOllama();
    refreshGpu();
  }, [refreshModels, refreshOllama, refreshGpu]);

  // Pick up downloads that kept running in Rust across a webview reload.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let alive = true;
    const tick = async () => {
      const more = await syncActive(() => refreshModels());
      if (alive && more) timer = setTimeout(tick, 2000);
    };
    tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [syncActive, refreshModels]);

  const tabs: Array<[ModelManagerTab, string, ReactNode]> = [
    ["installed", "Установленные", models.length || null],
    ["pull", "Загрузить", activePulls ? <LoaderCircle size={11} className="animate-spin" /> : null],
    ["running", "Запущенные", ollama?.loaded?.length || null],
  ];

  return (
    <Tabs.Root value={tab} onValueChange={(v) => setTab(v as ModelManagerTab)} className="flex flex-col">
      <div className="sticky top-0 z-10 border-b border-line bg-elevated px-3 pb-2 pt-2">
        <Tabs.List className="flex gap-0.5">
          {tabs.map(([v, label, badge]) => (
            <Tabs.Trigger
              key={v}
              value={v}
              className="inline-flex items-center gap-1.5 rounded-[var(--radius-sm)] px-2.5 py-1 text-[12.5px] text-fg2 hover:text-fg data-[state=active]:bg-hover data-[state=active]:text-fg"
            >
              {label}
              {badge != null && <span className="text-[11px] text-bluefg">{badge}</span>}
            </Tabs.Trigger>
          ))}
          <IconButton
            size="sm"
            label="Обновить"
            className="ml-auto"
            onClick={() => {
              estimateCache.clear();
              refreshModels();
              refreshOllama();
              refreshGpu();
            }}
          >
            <RefreshCw size={13} />
          </IconButton>
        </Tabs.List>
        <div className="mt-1.5 truncate px-1 text-[11px] text-fg3">
          {ollama && !ollama.online ? (
            <span className="text-red">Ollama не отвечает ({ollama.url}). Запустите Ollama и нажмите «Обновить».</span>
          ) : gpu?.memTotalMb ? (
            <>
              GPU: {gpu.name}, {fmtBytes(gpu.memTotalMb * 1024 * 1024)} VRAM
              {gpu.memUsedMb != null && <> · занято {fmtBytes(gpu.memUsedMb * 1024 * 1024)}</>}
            </>
          ) : (
            "Видеокарта NVIDIA не найдена: оценки VRAM без сравнения с объёмом карты"
          )}
        </div>
      </div>
      <Tabs.Content value="installed" className="outline-none">
        <InstalledTab />
      </Tabs.Content>
      <Tabs.Content value="pull" className="outline-none">
        <PullTab />
      </Tabs.Content>
      <Tabs.Content value="running" className="outline-none">
        <RunningTab />
      </Tabs.Content>
    </Tabs.Root>
  );
}

// ---------------------------------------------------------------------------------------------
// Installed
// ---------------------------------------------------------------------------------------------

function InstalledTab() {
  const models = useApp((s) => s.models);
  const [query, setQuery] = useState("");
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? models.filter((m) => m.name.toLowerCase().includes(q) || (m.family ?? "").toLowerCase().includes(q)) : models;
  }, [models, query]);
  const totalSize = models.reduce((s, m) => s + m.size, 0);

  return (
    <div className="px-3 py-2">
      {models.length > 5 && (
        <label className="mb-2 flex items-center gap-1.5">
          <Search size={13} className="text-fg3" />
          <input className={`${inputCls} flex-1`} placeholder="Поиск по имени" value={query} onChange={(e) => setQuery(e.target.value)} />
        </label>
      )}
      {models.length === 0 ? (
        <div className="px-1 py-6 text-center text-[12.5px] text-fg2">
          Модели не установлены. Откройте вкладку «Загрузить» и выберите модель из списка.
        </div>
      ) : (
        <>
          {shown.map((m) => (
            <ModelRow key={m.name} m={m} />
          ))}
          <div className="px-1 pt-1 text-[11px] text-fg3">
            {models.length} моделей, {fmtBytes(totalSize)} на диске (варианты одной модели делят файлы весов, реальный объём меньше).
          </div>
        </>
      )}
    </div>
  );
}

type RowPanel = null | "details" | "variant" | "copy" | "delete";

function ModelRow({ m }: { m: OllamaModel }) {
  const current = useApp((s) => s.settings?.defaultModel);
  const setModel = useApp((s) => s.setModel);
  const loaded = useApp((s) => s.ollama?.loaded?.find((l) => sameModel(l.name, m.name) || sameModel(l.model, m.name)));
  const gpuTotal = useGpuTotal();
  const [panel, setPanel] = useState<RowPanel>(null);
  const isCurrent = !!current && sameModel(current, m.name);
  const version = `${m.size}|${m.modifiedAt ?? ""}`;
  const { ctx, isDefault } = runCtx(m, gpuTotal);
  const est = useEstimate(m.embedding ? null : m.name, ctx, version);
  const toggle = (p: RowPanel) => setPanel(panel === p ? null : p);

  return (
    <div className={`mb-1.5 rounded-[var(--radius-md)] border px-3 py-2 ${isCurrent ? "border-blue" : "border-line"}`}>
      <div className="flex items-center gap-2">
        <Cpu size={14} className="shrink-0 text-fg3" />
        <span className="min-w-0 flex-1 truncate font-mono text-[13px]" title={m.name}>
          {m.name}
        </span>
        {isCurrent && <span className="shrink-0 text-[11px] text-bluefg">по умолчанию</span>}
      </div>
      <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-fg3">
        {m.parameterSize && <span>{m.parameterSize}</span>}
        {m.quantization && <span>{m.quantization}</span>}
        <span>{fmtBytes(m.size)}</span>
        {!m.embedding && (
          <span title={isDefault ? "num_ctx не задан в модели: Ollama берёт значение по умолчанию (OLLAMA_CONTEXT_LENGTH или 4k/32k/256k по объёму VRAM)" : "num_ctx из Modelfile"}>
            контекст {fmtTokens(ctx)}
            {isDefault ? " (по умолч.)" : ""}
            {m.nativeCtx ? ` из ${fmtTokens(m.nativeCtx)}` : ""}
          </span>
        )}
        {loaded && (
          <span className={loaded.size_vram < loaded.size ? "text-yellow" : "text-green"}>
            загружена{loaded.size_vram < loaded.size ? " (часть на CPU)" : ""}
          </span>
        )}
      </div>
      <div className="mt-1 flex flex-wrap items-center gap-1.5">
        <CapBadges caps={m.capabilities} />
        {est.data && (
          <FitChip
            total={est.data.totalF16Bytes}
            gpuTotal={gpuTotal}
            title={`${FIT_HINT[fitOf(est.data.totalF16Bytes, gpuTotal)]}. Оценка при контексте ${fmtTokens(est.data.numCtx)}: веса ${fmtBytes(
              est.data.weightsBytes,
            )} + KV-кэш ${est.data.kvKnown ? fmtBytes(est.data.kvF16Bytes) : "?"} + служебное ${fmtBytes(est.data.overheadBytes)}`}
          />
        )}
        {est.error && <span className="text-[11px] text-fg3" title={est.error}>VRAM: нет оценки</span>}
      </div>
      <div className="mt-1.5 flex flex-wrap gap-1.5">
        {!m.embedding && !isCurrent && (
          <Button className={smallBtn} onClick={() => setModel(m.name)}>
            <Check size={12} /> Использовать
          </Button>
        )}
        <Button variant="ghost" className={smallBtn} onClick={() => toggle("details")}>
          <Info size={12} /> Подробнее
        </Button>
        {!m.embedding && (
          <Button variant="ghost" className={smallBtn} onClick={() => toggle("variant")}>
            <HardDriveDownload size={12} /> Вариант с num_ctx
          </Button>
        )}
        <Button variant="ghost" className={smallBtn} onClick={() => toggle("copy")}>
          <Copy size={12} /> Копировать
        </Button>
        <Button variant="ghost" className={`${smallBtn} hover:text-red`} onClick={() => toggle("delete")}>
          <Trash2 size={12} /> Удалить
        </Button>
      </div>
      {panel === "details" && <DetailsView m={m} version={version} />}
      {panel === "variant" && <VariantForm m={m} version={version} onDone={() => setPanel(null)} />}
      {panel === "copy" && <CopyForm m={m} isCurrent={isCurrent} onDone={() => setPanel(null)} />}
      {panel === "delete" && <DeleteConfirm m={m} isCurrent={isCurrent} loaded={!!loaded} onDone={() => setPanel(null)} />}
    </div>
  );
}

function SubPanel({ children }: { children: ReactNode }) {
  return <div className="mt-2 rounded-[var(--radius-sm)] border border-line-muted bg-base px-2.5 py-2 text-[12px]">{children}</div>;
}

function DetailsView({ m, version }: { m: OllamaModel; version: string }) {
  const gpuTotal = useGpuTotal();
  const [details, setDetails] = useState<ModelDetails | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [ctx, setCtx] = useState(() => runCtx(m, gpuTotal).ctx);
  const est = useEstimate(m.embedding ? null : m.name, ctx, version);

  useEffect(() => {
    let alive = true;
    modelsApi.show(m.name).then(
      (d) => alive && setDetails(d),
      (e) => alive && setErr(errText(e)),
    );
    return () => {
      alive = false;
    };
  }, [m.name, version]);

  if (err) return <SubPanel><span className="text-red">{err}</span></SubPanel>;
  if (!details) return <SubPanel><div className="skeleton h-16" /></SubPanel>;
  const a = details.arch;
  const headDim = a.embeddingLength && a.headCount ? Math.round(a.embeddingLength / a.headCount) : null;
  const kvHeads = a.headCountKv.length > 1 ? `${Math.min(...a.headCountKv)}–${Math.max(...a.headCountKv)} по слоям` : a.headCountKv[0] ?? a.headCount;
  const rows: Array<[string, ReactNode]> = [
    ["Архитектура", a.architecture],
    ["Семейство", details.families.length > 1 ? details.families.join(", ") : details.family],
    ["Параметров", [details.parameterSize, fmtParams(details.parameterCount)].filter(Boolean).join(" · ")],
    ["Квантизация", details.quantizationLevel],
    ["Формат", details.format],
    ["Основана на", details.parentModel],
    ["Контекст обучения", a.contextLength ? fmtTokens(a.contextLength) : null],
    ["num_ctx в Modelfile", details.numCtx ? fmtTokens(details.numCtx) : "не задан"],
    ["Слоёв", a.blockCount],
    ["Головы внимания / KV", a.headCount ? `${a.headCount} / ${kvHeads ?? "?"}` : null],
    ["Размер головы K / V", a.keyLength || headDim ? `${a.keyLength ?? headDim} / ${a.valueLength ?? headDim}` : null],
    ["Скользящее окно", a.slidingWindow ? fmtTokens(a.slidingWindow) : null],
    ["Проектор изображений", details.hasProjector ? "есть" : null],
  ];

  return (
    <SubPanel>
      <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
        {rows
          .filter(([, v]) => v != null && v !== "")
          .map(([k, v]) => (
            <div key={k} className="contents">
              <span className="text-fg3">{k}</span>
              <span className="min-w-0 truncate font-mono text-[11.5px]">{v}</span>
            </div>
          ))}
      </div>
      {details.capabilities.length > 0 && (
        <div className="mt-1.5">
          <CapBadges caps={details.capabilities} />
        </div>
      )}

      {!m.embedding && (
        <div className="mt-2.5">
          <div className="mb-1 flex items-center gap-2">
            <span className="font-medium">Видеопамять при контексте</span>
            <CtxSelect value={ctx} onChange={setCtx} max={a.contextLength} min={4096} />
          </div>
          {est.error && <div className="text-red">{est.error}</div>}
          {!est.data && !est.error && <div className="skeleton h-10" />}
          {est.data && <EstimateTable e={est.data} gpuTotal={gpuTotal} />}
        </div>
      )}

      {details.licenseSnippet && (
        <details className="mt-2.5">
          <summary className="cursor-pointer text-fg2">Лицензия</summary>
          <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] text-fg2">{details.licenseSnippet}</pre>
          {details.licenseTruncated && (
            <div className="text-[11px] text-fg3">
              Полный текст: <span className="font-mono">ollama show --license {m.name}</span>
            </div>
          )}
        </details>
      )}
    </SubPanel>
  );
}

function EstimateTable({ e, gpuTotal }: { e: VramEstimate; gpuTotal: number | null }) {
  const rows: Array<[string, number, number, string]> = [
    ["f16 (по умолчанию)", e.kvF16Bytes, e.totalF16Bytes, ""],
    ["q8_0", e.kvQ80Bytes, e.totalQ80Bytes, "OLLAMA_KV_CACHE_TYPE=q8_0 (работает с Flash Attention, Ollama включает его сама, где он поддерживается)"],
    ["q4_0", e.kvQ40Bytes, e.totalQ40Bytes, "OLLAMA_KV_CACHE_TYPE=q4_0: заметно хуже качество"],
  ];
  return (
    <div className="text-[11.5px]">
      <table className="w-full">
        <thead className="text-fg3">
          <tr>
            <th className="py-0.5 text-left font-normal">KV-кэш</th>
            <th className="text-right font-normal">кэш</th>
            <th className="text-right font-normal">всего</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(([label, kv, total, hint]) => (
            <tr key={label} className="border-t border-line-muted" title={hint}>
              <td className="py-0.5">{label}</td>
              <td className="text-right font-mono">{e.kvKnown ? fmtBytes(kv) : "?"}</td>
              <td className="py-0.5 text-right">
                <FitChip total={total} gpuTotal={gpuTotal} prefix="" />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="mt-1 text-[11px] leading-[1.45] text-fg3">
        Веса {fmtBytes(e.weightsBytes)} + KV-кэш ({e.kvKnown ? `${fmtBytes(e.kvBytesPerTokenF16)} на токен в f16` : "нет данных об архитектуре"}) + ~
        {fmtBytes(e.overheadBytes)} служебных буферов.
        {e.ctxClamped && <> Контекст ограничен обучающим окном модели ({fmtTokens(e.numCtx)}).</>} Оценка приблизительная; для моделей со
        скользящим окном (Gemma 3, gpt-oss) реальный кэш меньше.
      </div>
    </div>
  );
}

function VariantForm({ m, version, onDone }: { m: OllamaModel; version: string; onDone: () => void }) {
  const models = useApp((s) => s.models);
  const refreshModels = useApp((s) => s.refreshModels);
  const gpuTotal = useGpuTotal();
  const max = m.nativeCtx ? Math.min(m.nativeCtx, 262144) : 262144;
  const [ctx, setCtx] = useState(() => Math.min(32768, max));
  // Same naming as the old models panel: qwen3:4b-instruct-2507-q4_K_M -> qwen3-4b-32k.
  const base = m.name.replace(/:latest$/, "").replace(/[:/]/g, "-").replace(/-instruct-2507-q4_K_M$/, "");
  const [name, setName] = useState(`${base}-${Math.round(ctx / 1024)}k`);
  const [nameTouched, setNameTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const est = useEstimate(m.name, ctx, version);
  const exists = models.some((x) => sameModel(x.name, name));
  const valid = isValidModelName(name) && !exists;

  const onCtx = (v: number) => {
    setCtx(v);
    if (!nameTouched) setName(`${base}-${Math.round(v / 1024)}k`);
  };

  const create = async () => {
    setBusy(true);
    try {
      await api.ollamaCreateVariant(m.name, name.trim(), ctx);
      toast.success(`Создана модель ${name.trim()}`, { description: "Файлы весов общие, место на диске не тратится" });
      await refreshModels();
      onDone();
    } catch (e) {
      toast.error("Не удалось создать вариант", { description: errText(e) });
    }
    setBusy(false);
  };

  return (
    <SubPanel>
      <div className="mb-1.5 text-fg2">
        Новая модель с зашитым <span className="font-mono">num_ctx</span>. Так CodeWhale и другие OpenAI-совместимые клиенты получат нужный
        контекст.
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        <CtxSelect value={ctx} onChange={onCtx} max={max} />
        <input
          className={`${inputCls} flex-1 font-mono`}
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            setNameTouched(true);
          }}
          onKeyDown={(e) => e.key === "Enter" && valid && !busy && create()}
        />
        <Button variant="primary" className={smallBtn} disabled={!valid || busy} onClick={create}>
          {busy ? <LoaderCircle size={12} className="animate-spin" /> : null} Создать
        </Button>
      </div>
      <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-[11px] text-fg3">
        {exists && <span className="text-red">Такое имя уже занято.</span>}
        {!isValidModelName(name) && <span className="text-red">Допустимы латиница, цифры и . _ : - /</span>}
        {est.data && (
          <>
            <FitChip total={est.data.totalF16Bytes} gpuTotal={gpuTotal} />
            {fitOf(est.data.totalF16Bytes, gpuTotal) === "over" && fitOf(est.data.totalQ80Bytes, gpuTotal) !== "over" && (
              <span>влезет с KV-кэшем q8_0 ({fmtBytes(est.data.totalQ80Bytes)})</span>
            )}
          </>
        )}
        {est.error && <span title={est.error}>оценка VRAM недоступна</span>}
      </div>
    </SubPanel>
  );
}

function CopyForm({ m, isCurrent, onDone }: { m: OllamaModel; isCurrent: boolean; onDone: () => void }) {
  const models = useApp((s) => s.models);
  const refreshModels = useApp((s) => s.refreshModels);
  const setModel = useApp((s) => s.setModel);
  const [to, setTo] = useState(`${m.name.replace(/:latest$/, "")}-copy`);
  const [rename, setRename] = useState(false);
  const [busy, setBusy] = useState(false);
  const exists = models.some((x) => sameModel(x.name, to));
  const valid = isValidModelName(to) && !exists;

  const run = async () => {
    const target = to.trim();
    setBusy(true);
    try {
      await modelsApi.copy(m.name, target);
      if (rename) {
        try {
          await modelsApi.remove(m.name);
        } catch (e) {
          toast.error(`Копия ${target} создана, но ${m.name} удалить не удалось`, { description: errText(e) });
        }
        if (isCurrent) await setModel(target);
      }
      toast.success(rename ? `Модель переименована в ${target}` : `Создана копия ${target}`);
      await refreshModels();
      onDone();
    } catch (e) {
      toast.error(rename ? "Не удалось переименовать" : "Не удалось скопировать", { description: errText(e) });
    }
    setBusy(false);
  };

  return (
    <SubPanel>
      <div className="flex items-center gap-1.5">
        <input
          className={`${inputCls} flex-1 font-mono`}
          value={to}
          onChange={(e) => setTo(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && valid && !busy && run()}
          autoFocus
        />
        <Button variant="primary" className={smallBtn} disabled={!valid || busy} onClick={run}>
          {rename ? "Переименовать" : "Копировать"}
        </Button>
      </div>
      <label className="mt-1.5 flex cursor-pointer items-center gap-1.5 text-fg2">
        <input type="checkbox" checked={rename} onChange={(e) => setRename(e.target.checked)} />
        Удалить исходную (переименование)
      </label>
      <div className="mt-1 text-[11px] text-fg3">
        {exists ? <span className="text-red">Такое имя уже занято.</span> : "Копия делит файлы весов с исходной и не занимает место на диске."}
      </div>
    </SubPanel>
  );
}

function DeleteConfirm({ m, isCurrent, loaded, onDone }: { m: OllamaModel; isCurrent: boolean; loaded: boolean; onDone: () => void }) {
  const refreshModels = useApp((s) => s.refreshModels);
  const refreshOllama = useApp((s) => s.refreshOllama);
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      await modelsApi.remove(m.name);
      toast.success(`Модель ${m.name} удалена`);
      onDone();
    } catch (e) {
      toast.error("Не удалось удалить модель", { description: errText(e) });
    }
    setBusy(false);
    refreshModels();
    refreshOllama();
  };
  return (
    <SubPanel>
      <div className="text-fg">
        Удалить <span className="font-mono">{m.name}</span>?
      </div>
      <ul className="mt-1 list-disc pl-4 text-[11px] text-fg3">
        {isCurrent && <li className="text-yellow">Это модель по умолчанию: после удаления выберите другую.</li>}
        {loaded && <li>Модель загружена в память и будет сначала выгружена.</li>}
        <li>Место освободится, только если веса не используются другими моделями (копиями и вариантами с num_ctx).</li>
      </ul>
      <div className="mt-2 flex gap-1.5">
        <Button variant="danger" className={smallBtn} disabled={busy} onClick={run}>
          {busy ? <LoaderCircle size={12} className="animate-spin" /> : <Trash2 size={12} />} Удалить
        </Button>
        <Button variant="ghost" className={smallBtn} onClick={onDone}>
          Отмена
        </Button>
      </div>
    </SubPanel>
  );
}

// ---------------------------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------------------------

function PullTab() {
  const models = useApp((s) => s.models);
  const refreshModels = useApp((s) => s.refreshModels);
  const pulls = usePulls((s) => s.pulls);
  const start = usePulls((s) => s.start);
  const gpuTotal = useGpuTotal();
  const [name, setName] = useState("");
  const trimmed = name.trim();
  const valid = isValidModelName(trimmed);
  const installed = (n: string) => models.some((m) => sameModel(m.name, n));
  const entries = Object.values(pulls).sort((a, b) => b.startedAt - a.startedAt);

  const pull = (n: string) => {
    start(n, () => refreshModels());
  };

  return (
    <div className="px-3 py-2">
      <div className="flex items-center gap-1.5">
        <input
          className={`${inputCls} h-8 flex-1 font-mono`}
          placeholder="qwen3:8b или hf.co/user/repo:Q4_K_M"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && valid) {
              pull(trimmed);
              setName("");
            }
          }}
        />
        <Button
          variant="primary"
          disabled={!valid}
          onClick={() => {
            pull(trimmed);
            setName("");
          }}
        >
          <Download size={13} /> Скачать
        </Button>
      </div>
      <div className="mt-1 flex flex-wrap gap-x-3 px-0.5 text-[11px] text-fg3">
        {trimmed && !valid && <span className="text-red">Недопустимое имя модели</span>}
        {trimmed && valid && installed(trimmed) && <span className="text-yellow">Уже установлена: загрузка обновит её до последней версии</span>}
        <button className="inline-flex items-center gap-1 hover:text-fg" onClick={() => openLink("https://ollama.com/library")}>
          Каталог Ollama <ExternalLink size={10} />
        </button>
        <button className="inline-flex items-center gap-1 hover:text-fg" onClick={() => openLink("https://huggingface.co/models?library=gguf&sort=trending")}>
          GGUF на Hugging Face <ExternalLink size={10} />
        </button>
      </div>

      {entries.length > 0 && (
        <div className="mt-3">
          {entries.map((p) => (
            <PullRow key={canonicalName(p.name)} p={p} onRetry={() => pull(p.name)} />
          ))}
        </div>
      )}

      <div className="mb-1 mt-4 flex items-baseline gap-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-fg3">Рекомендуемые для кода</span>
        <span className="text-[11px] text-yellow">рекомендации, проверьте актуальность</span>
      </div>
      <div className="mb-2 text-[11px] text-fg3">
        Агенту нужны вызов инструментов и контекст от 16k. Размеры приблизительные; оценка VRAM — веса плюс ~1–2 ГБ на кэш при контексте
        16–32k.
      </div>
      {RECOMMENDED_MODELS.map((r) => {
        const p = pulls[canonicalName(r.name)];
        const isInstalled = installed(r.name);
        const need = r.sizeGb * GIB * 1.1 + 1.5 * GIB;
        return (
          <div key={r.name} className="mb-1.5 rounded-[var(--radius-md)] border border-line px-3 py-2">
            <div className="flex items-center gap-2">
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[13px] font-medium">{r.title}</span>
                <span className="block truncate font-mono text-[11px] text-fg3">{r.name}</span>
              </span>
              {isInstalled ? (
                <span className="inline-flex items-center gap-1 text-[12px] text-green">
                  <Check size={12} /> Установлена
                </span>
              ) : p?.phase === "running" ? (
                <span className="inline-flex items-center gap-1 text-[12px] text-fg2">
                  <LoaderCircle size={12} className="animate-spin" /> Загрузка
                </span>
              ) : (
                <Button className={smallBtn} onClick={() => pull(r.name)}>
                  <Download size={12} /> Скачать
                </Button>
              )}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-fg3">
              <CapBadges caps={r.caps} />
              <span>≈ {r.sizeGb} ГБ</span>
              <span>контекст до {fmtTokens(r.ctx)}</span>
              <FitChip total={need} gpuTotal={gpuTotal} prefix="нужно ≈" />
            </div>
            <div className="mt-0.5 text-[11.5px] text-fg2">{r.note}</div>
          </div>
        );
      })}
    </div>
  );
}

function PullRow({ p, onRetry }: { p: PullEntry; onRetry: () => void }) {
  const cancel = usePulls((s) => s.cancel);
  const dismiss = usePulls((s) => s.dismiss);
  const fraction = p.overallTotal > 0 ? p.overallCompleted / p.overallTotal : null;
  const left = p.bytesPerSec > 0 ? (p.overallTotal - p.overallCompleted) / p.bytesPerSec : 0;
  const tone = p.phase === "error" ? "red" : p.phase === "done" ? "green" : p.phase === "cancelled" ? "yellow" : "blue";
  return (
    <div className="mb-1.5 rounded-[var(--radius-md)] border border-line px-3 py-2">
      <div className="flex items-center gap-2">
        {p.phase === "running" ? (
          <LoaderCircle size={13} className="shrink-0 animate-spin text-bluefg" />
        ) : p.phase === "done" ? (
          <Check size={13} className="shrink-0 text-green" />
        ) : (
          <TriangleAlert size={13} className={`shrink-0 ${p.phase === "error" ? "text-red" : "text-yellow"}`} />
        )}
        <span className="min-w-0 flex-1 truncate font-mono text-[12.5px]">{p.name}</span>
        {p.phase === "running" ? (
          <IconButton size="sm" label="Отменить загрузку" onClick={() => cancel(p.name)}>
            <X size={13} />
          </IconButton>
        ) : (
          <>
            {(p.phase === "error" || p.phase === "cancelled") && (
              <Button variant="ghost" className={smallBtn} onClick={onRetry}>
                <RefreshCw size={12} /> Повторить
              </Button>
            )}
            <IconButton size="sm" label="Убрать из списка" onClick={() => dismiss(p.name)}>
              <X size={13} />
            </IconButton>
          </>
        )}
      </div>
      <div className="mt-1.5">
        <ProgressBar value={p.phase === "done" ? 1 : p.phase === "running" ? fraction : fraction ?? 0} tone={tone} />
      </div>
      <div className="mt-1 flex flex-wrap gap-x-3 text-[11px] text-fg3">
        <span>{pullStatusLabel(p.status)}</span>
        {p.overallTotal > 0 && (
          <span>
            {fmtBytes(p.overallCompleted)} из {fmtBytes(p.overallTotal)}
            {fraction != null && ` (${Math.floor(fraction * 100)}%)`}
          </span>
        )}
        {p.phase === "running" && p.bytesPerSec > 0 && (
          <span>
            {fmtBytes(p.bytesPerSec)}/с {fmtEta(left) && `· осталось ${fmtEta(left)}`}
          </span>
        )}
        {p.orphan && <span>прогресс недоступен после перезагрузки окна</span>}
      </div>
      {p.phase === "error" && p.error && <div className="mt-1 break-words text-[11.5px] text-red">{p.error}</div>}
      {p.phase === "cancelled" && <div className="mt-1 text-[11px] text-fg3">Скачанные части сохранены: повторная загрузка продолжится с места остановки.</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------------

function expiresLabel(iso?: string | null): string | null {
  if (!iso) return null;
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return null;
  const ms = t - Date.now();
  if (ms > 365 * 86_400_000) return "держится в памяти бессрочно";
  if (ms <= 0) return "выгружается";
  const min = Math.round(ms / 60_000);
  return min < 1 ? "выгрузится меньше чем через минуту" : `выгрузится через ${min} мин`;
}

/** Same split `ollama ps` prints in its PROCESSOR column. */
function processorLabel(r: RunningModel): string {
  if (!r.size) return "";
  if (r.sizeVram >= r.size) return "100% GPU";
  if (r.sizeVram === 0) return "100% CPU";
  const gpuPct = Math.round((r.sizeVram / r.size) * 100);
  return `${100 - gpuPct}% CPU / ${gpuPct}% GPU`;
}

function RunningTab() {
  const gpu = useApp((s) => s.gpu);
  const refreshGpu = useApp((s) => s.refreshGpu);
  const refreshOllama = useApp((s) => s.refreshOllama);
  const [list, setList] = useState<RunningModel[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let alive = true;
    const load = () => {
      modelsApi.running().then(
        (l) => {
          if (!alive) return;
          setList(l);
          setErr(null);
        },
        (e) => alive && setErr(errText(e)),
      );
      refreshGpu();
    };
    load();
    const id = setInterval(load, 3000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [refreshGpu, tick]);

  const unload = async (name: string) => {
    setBusy(name);
    try {
      await api.ollamaUnload(name);
      toast.success(`${name} выгружена из памяти`);
    } catch (e) {
      toast.error("Не удалось выгрузить модель", { description: errText(e) });
    }
    setBusy(null);
    setTick((t) => t + 1);
    refreshOllama();
  };

  const used = gpu?.memUsedMb != null && gpu.memTotalMb ? gpu.memUsedMb / gpu.memTotalMb : null;

  return (
    <div className="px-3 py-2">
      {gpu?.memTotalMb ? (
        <div className="mb-3">
          <div className="mb-1 flex justify-between text-[11px] text-fg3">
            <span>Видеопамять {gpu.name}</span>
            <span>
              {fmtBytes((gpu.memUsedMb ?? 0) * 1024 * 1024)} из {fmtBytes(gpu.memTotalMb * 1024 * 1024)}
              {gpu.util != null && ` · загрузка ${gpu.util}%`}
              {gpu.tempC != null && ` · ${gpu.tempC} °C`}
            </span>
          </div>
          <ProgressBar value={used} tone={used != null && used > 0.95 ? "red" : used != null && used > 0.85 ? "yellow" : "blue"} />
        </div>
      ) : null}
      {err && <div className="mb-2 text-[12px] text-red">{err}</div>}
      {list === null && !err && <div className="skeleton h-14" />}
      {list?.length === 0 && (
        <div className="px-1 py-4 text-center text-[12.5px] text-fg2">
          Сейчас ни одна модель не загружена. Ollama загружает модель при первом запросе и выгружает после 5 минут простоя
          (<span className="font-mono">OLLAMA_KEEP_ALIVE</span>).
        </div>
      )}
      {list?.map((r) => {
        const partial = r.sizeVram < r.size;
        return (
          <div key={r.name} className="mb-1.5 rounded-[var(--radius-md)] border border-line px-3 py-2">
            <div className="flex items-center gap-2">
              <Cpu size={14} className="shrink-0 text-fg3" />
              <span className="min-w-0 flex-1 truncate font-mono text-[13px]">{r.name}</span>
              <Button variant="ghost" className={smallBtn} disabled={busy === r.name} onClick={() => unload(r.name)}>
                {busy === r.name ? <LoaderCircle size={12} className="animate-spin" /> : <Power size={12} />} Выгрузить
              </Button>
            </div>
            <div className="mt-1 flex flex-wrap gap-x-3 text-[11px] text-fg3">
              {r.parameterSize && <span>{r.parameterSize}</span>}
              {r.quantization && <span>{r.quantization}</span>}
              <span>память {fmtBytes(r.size)}</span>
              <span className={partial ? "text-yellow" : "text-green"} title={partial ? "Часть слоёв на CPU: генерация медленнее" : undefined}>
                {processorLabel(r)}
              </span>
              {r.contextLength ? <span>контекст {fmtTokens(r.contextLength)}</span> : null}
              {expiresLabel(r.expiresAt) && <span>{expiresLabel(r.expiresAt)}</span>}
            </div>
            {partial && (
              <div className="mt-1 text-[11px] text-yellow">
                Модель не поместилась в видеопамять. Уменьшите контекст (вариант с меньшим num_ctx), включите KV-кэш q8_0 или выберите модель
                меньше.
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
