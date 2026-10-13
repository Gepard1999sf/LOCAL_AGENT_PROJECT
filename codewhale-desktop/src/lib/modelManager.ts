// Typed wrappers for the Rust model-manager commands (src-tauri/src/models.rs) and a small
// store that keeps model downloads alive while the panel is closed.

import { Channel, invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { toast } from "sonner";

export interface PullProgress {
  name: string;
  status: string;
  digest?: string | null;
  total?: number | null;
  completed?: number | null;
  /** Sum over every layer announced so far. */
  overallTotal: number;
  overallCompleted: number;
  done: boolean;
}

export interface PullResult {
  name: string;
  cancelled: boolean;
}

export interface ArchParams {
  architecture?: string | null;
  blockCount?: number | null;
  headCount?: number | null;
  /** Per layer, or a single value for all layers; 0 = layer without attention. */
  headCountKv: number[];
  embeddingLength?: number | null;
  keyLength?: number | null;
  valueLength?: number | null;
  contextLength?: number | null;
  slidingWindow?: number | null;
}

export interface ModelDetails {
  name: string;
  family?: string | null;
  families: string[];
  format?: string | null;
  parentModel?: string | null;
  parameterSize?: string | null;
  parameterCount?: number | null;
  quantizationLevel?: string | null;
  contextLength?: number | null;
  numCtx?: number | null;
  capabilities: string[];
  parameters?: string | null;
  licenseSnippet?: string | null;
  licenseTruncated: boolean;
  modifiedAt?: string | null;
  hasProjector: boolean;
  arch: ArchParams;
}

export interface RunningModel {
  name: string;
  model: string;
  size: number;
  sizeVram: number;
  contextLength?: number | null;
  expiresAt?: string | null;
  parameterSize?: string | null;
  quantization?: string | null;
  family?: string | null;
}

/** total = weights + KV cache + fixed runtime overhead (see models.rs for the formula). */
export interface VramEstimate {
  name: string;
  numCtx: number;
  nativeCtx?: number | null;
  ctxClamped: boolean;
  weightsBytes: number;
  kvKnown: boolean;
  kvBytesPerTokenF16: number;
  kvF16Bytes: number;
  kvQ80Bytes: number;
  kvQ40Bytes: number;
  overheadBytes: number;
  totalF16Bytes: number;
  totalQ80Bytes: number;
  totalQ40Bytes: number;
}

export const modelsApi = {
  pull: (name: string, onProgress: (p: PullProgress) => void) => {
    const channel = new Channel<PullProgress>();
    channel.onmessage = onProgress;
    return invoke<PullResult>("models_pull", { name, onProgress: channel });
  },
  pullCancel: (name: string) => invoke<boolean>("models_pull_cancel", { name }),
  pullsActive: () => invoke<string[]>("models_pulls_active"),
  remove: (name: string) => invoke<void>("models_delete", { name }),
  copy: (from: string, to: string) => invoke<void>("models_copy", { from, to }),
  show: (name: string) => invoke<ModelDetails>("models_show", { name }),
  running: () => invoke<RunningModel[]>("models_running"),
  estimateVram: (name: string, numCtx: number) => invoke<VramEstimate>("models_estimate_vram", { name, numCtx }),
};

/** Same normalization as Rust `canonical_name`: `qwen3` == `qwen3:latest`, case-insensitive. */
export function canonicalName(name: string): string {
  const n = name.trim().toLowerCase();
  const last = n.split("/").pop() ?? n;
  return last.includes(":") ? n : `${n}:latest`;
}

export function sameModel(a: string, b: string): boolean {
  return canonicalName(a) === canonicalName(b);
}

/** Model name rule shared with Rust (`validate_name`). */
export function isValidModelName(name: string): boolean {
  return /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,199}$/.test(name.trim());
}

// ---------------------------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------------------------

export type PullPhase = "running" | "done" | "error" | "cancelled";

export interface PullEntry {
  name: string;
  phase: PullPhase;
  status: string;
  overallTotal: number;
  overallCompleted: number;
  /** Smoothed download speed, bytes per second. */
  bytesPerSec: number;
  startedAt: number;
  error?: string;
  /** Started before a webview reload: still running in Rust, but its progress channel is gone. */
  orphan?: boolean;
}

interface PullStore {
  pulls: Record<string, PullEntry>;
  /** Starts a download; `onInstalled` runs after Ollama reports success. */
  start: (name: string, onInstalled?: () => void) => Promise<void>;
  cancel: (name: string) => Promise<void>;
  dismiss: (name: string) => void;
  /**
   * Reconciles with the pulls Rust is running: adds downloads started before a reload and drops
   * them once they end. Returns true while such downloads remain (poll until false).
   */
  syncActive: (onFinished?: () => void) => Promise<boolean>;
}

const speedSamples = new Map<string, { t: number; bytes: number }>();

function errText(e: unknown): string {
  return typeof e === "string" ? e : (e as any)?.message ?? String(e);
}

export const usePulls = create<PullStore>((set, get) => {
  const patch = (key: string, p: Partial<PullEntry>) =>
    set((s) => (s.pulls[key] ? { pulls: { ...s.pulls, [key]: { ...s.pulls[key], ...p } } } : s));

  return {
    pulls: {},

    start: async (rawName, onInstalled) => {
      const name = rawName.trim();
      const key = canonicalName(name);
      if (get().pulls[key]?.phase === "running") return;
      set((s) => ({
        pulls: {
          ...s.pulls,
          [key]: { name, phase: "running", status: "подключение…", overallTotal: 0, overallCompleted: 0, bytesPerSec: 0, startedAt: Date.now() },
        },
      }));
      speedSamples.delete(key);
      try {
        const res = await modelsApi.pull(name, (p) => {
          const now = performance.now();
          const prev = speedSamples.get(key);
          let bytesPerSec = get().pulls[key]?.bytesPerSec ?? 0;
          if (!prev) {
            speedSamples.set(key, { t: now, bytes: p.overallCompleted });
          } else if (now - prev.t >= 500) {
            const inst = Math.max(0, ((p.overallCompleted - prev.bytes) * 1000) / (now - prev.t));
            bytesPerSec = bytesPerSec ? bytesPerSec * 0.7 + inst * 0.3 : inst;
            speedSamples.set(key, { t: now, bytes: p.overallCompleted });
          }
          patch(key, { status: p.status, overallTotal: p.overallTotal, overallCompleted: p.overallCompleted, bytesPerSec });
        });
        speedSamples.delete(key);
        if (res.cancelled) {
          patch(key, { phase: "cancelled", status: "отменено", bytesPerSec: 0 });
          return;
        }
        patch(key, { phase: "done", status: "готово", bytesPerSec: 0 });
        toast.success(`Модель ${name} загружена`);
        onInstalled?.();
      } catch (e) {
        speedSamples.delete(key);
        const msg = errText(e);
        patch(key, { phase: "error", status: "ошибка", error: msg, bytesPerSec: 0 });
        toast.error(`Не удалось загрузить ${name}`, { description: msg });
      }
    },

    cancel: async (name) => {
      try {
        await modelsApi.pullCancel(name);
      } catch (e) {
        toast.error("Не удалось отменить загрузку", { description: errText(e) });
      }
    },

    dismiss: (name) =>
      set((s) => {
        const pulls = { ...s.pulls };
        delete pulls[canonicalName(name)];
        return { pulls };
      }),

    syncActive: async (onFinished) => {
      const active = await modelsApi.pullsActive().catch(() => null);
      if (!active) return false;
      const activeSet = new Set(active.map(canonicalName));
      let finished = false;
      set((s) => {
        const pulls = { ...s.pulls };
        for (const key of activeSet) {
          if (!pulls[key] || pulls[key].phase !== "running") {
            pulls[key] = {
              name: key,
              phase: "running",
              status: "идёт в фоне",
              overallTotal: 0,
              overallCompleted: 0,
              bytesPerSec: 0,
              startedAt: Date.now(),
              orphan: true,
            };
          }
        }
        for (const [key, e] of Object.entries(pulls)) {
          if (e.orphan && e.phase === "running" && !activeSet.has(key)) {
            delete pulls[key];
            finished = true;
          }
        }
        return { pulls };
      });
      if (finished) onFinished?.();
      return Object.values(get().pulls).some((e) => e.orphan && e.phase === "running");
    },
  };
});

/** Human-readable stage of an Ollama pull status line. */
export function pullStatusLabel(status: string): string {
  if (status.startsWith("pulling manifest")) return "получение манифеста";
  if (status.startsWith("pulling ")) return "загрузка слоёв";
  if (status.startsWith("verifying")) return "проверка контрольной суммы";
  if (status.startsWith("writing manifest")) return "запись манифеста";
  if (status.startsWith("removing")) return "очистка старых слоёв";
  if (status === "success") return "готово";
  return status;
}
