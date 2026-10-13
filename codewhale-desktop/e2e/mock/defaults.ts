/**
 * Default answers of the Rust-side Tauri commands (src-tauri/src/*.rs). These
 * never reach the CodeWhale HTTP API, so the live recorder cannot capture them;
 * the shapes follow the serde structs (camelCase) of the desktop backend.
 */

export const DEFAULT_SETTINGS = {
  ollamaUrl: "http://127.0.0.1:11434",
  codewhalePath: null,
  telemetry: false,
  workspace: "C:\\Users\\dev\\projects\\demo",
  recentWorkspaces: ["C:\\Users\\dev\\projects\\demo", "C:\\Users\\dev\\projects\\game"],
  defaultModel: null as string | null,
  toolProfile: "lite",
  theme: "dark",
  fontMono: "JetBrains Mono",
  fontSize: 14,
  density: "comfortable",
  showToolCalls: true,
  showThinking: true,
  autoApprove: {
    read: true,
    grep_files: true,
    list_dir: true,
    file_search: true,
    edit: false,
    write: false,
    apply_patch: false,
    bash: false,
    Web: false,
  } as Record<string, boolean>,
  language: "ru",
  _file: "C:\\Users\\dev\\AppData\\Roaming\\codewhale-desktop\\settings.json",
};

export type MockSettings = typeof DEFAULT_SETTINGS;

export const RUNNING_RUNTIME = {
  state: "running",
  port: 51234,
  pid: 4242,
  binary: "C:\\Program Files\\CodeWhale Desktop\\codewhale.exe",
  version: "0.10.2",
  error: null as string | null,
  startedAtMs: 1_791_900_000_000,
};

export type MockRuntime = Omit<typeof RUNNING_RUNTIME, "state"> & {
  state: "stopped" | "starting" | "running" | "failed";
};

export const OLLAMA_ONLINE = {
  online: true,
  url: "http://127.0.0.1:11434",
  version: "0.12.6",
  loaded: [
    {
      name: "qwen3-8b-16k:latest",
      model: "qwen3-8b-16k:latest",
      size: 7_400_000_000,
      size_vram: 7_100_000_000,
      context_length: 16384,
      expires_at: "2026-10-13T10:00:00Z",
    },
  ],
};

export const OLLAMA_MODELS = [
  {
    name: "qwen3-8b-16k",
    size: 5_225_000_000,
    family: "qwen3",
    parameterSize: "8.2B",
    quantization: "Q4_K_M",
    capabilities: ["completion", "tools", "thinking"],
    nativeCtx: 40960,
    numCtx: 16384,
    modifiedAt: "2026-10-01T12:00:00Z",
    embedding: false,
  },
  {
    name: "qwen3:4b",
    size: 2_497_000_000,
    family: "qwen3",
    parameterSize: "4.0B",
    quantization: "Q4_K_M",
    capabilities: ["completion", "tools", "thinking"],
    nativeCtx: 40960,
    numCtx: null,
    modifiedAt: "2026-09-20T12:00:00Z",
    embedding: false,
  },
  {
    name: "gemma3:4b",
    size: 3_338_000_000,
    family: "gemma3",
    parameterSize: "4.3B",
    quantization: "Q4_K_M",
    capabilities: ["completion", "vision"],
    nativeCtx: 131072,
    numCtx: null,
    modifiedAt: "2026-09-10T12:00:00Z",
    embedding: false,
  },
  {
    name: "nomic-embed-text",
    size: 274_000_000,
    family: "nomic-bert",
    parameterSize: "137M",
    quantization: "F16",
    capabilities: ["embedding"],
    nativeCtx: 2048,
    numCtx: null,
    modifiedAt: "2026-08-01T12:00:00Z",
    embedding: true,
  },
];

export const GPU = {
  name: "NVIDIA GeForce RTX 4070",
  memUsedMb: 7_480,
  memTotalMb: 12_282,
  util: 37,
  tempC: 58,
};

export const RUNTIME_LOGS = [
  "[codewhale] runtime api listening on 127.0.0.1:51234",
  "[codewhale] ollama provider ready (http://127.0.0.1:11434)",
];
