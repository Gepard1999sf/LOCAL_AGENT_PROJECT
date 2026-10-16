// Typed wrappers for the knowledge-drive commands (src-tauri/src/drive.rs): the removable
// 1 TB drive with models, docsets, git_vault, Kiwix dumps and the ai-drive MCP server.

import { Channel, invoke } from "@tauri-apps/api/core";

export interface DriveSection {
  key: string;
  title: string;
  path: string;
  exists: boolean;
  entries: number;
}

export interface DriveIndexInfo {
  path: string;
  bytes: number;
  files?: number | null;
  builtAt?: string | null;
}

export interface KiwixBook {
  name: string;
  title: string;
}

export interface DriveStatus {
  root: string;
  present: boolean;
  sections: DriveSection[];
  index?: DriveIndexInfo | null;
  docsets: string[];
  zims: string[];
  kiwixUrl: string;
  kiwixOnline?: boolean | null;
  godotBin?: string | null;
  godotFound: boolean;
  sympyHelper?: string | null;
  stateDir: string;
  exePath: string;
  mcpPath: string;
  mcpRegistered: boolean;
  ollamaModelsEnv?: string | null;
  ollamaModelsOnDrive: boolean;
  skillsInstalled: string[];
  helpersInstalled: boolean;
  godotCheckProject?: string | null;
  kiwixRunning: boolean;
  kiwixPort: number;
  kiwixBooks: KiwixBook[];
  indexRunning: boolean;
}

export interface IndexProgress {
  files: number;
  bytes: number;
  skipped: number;
}

export interface IndexSummary {
  path: string;
  files: number;
  bytes: number;
  skipped: number;
  elapsedMs: number;
}

export interface GoldenModel {
  name: string;
  role: string;
  approxGb: number;
  note: string;
}

export interface KiwixInfo {
  url: string;
  running: boolean;
  books: KiwixBook[];
}

export const driveApi = {
  status: () => invoke<DriveStatus>("drive_status"),
  registerMcp: () => invoke<string>("drive_register_mcp"),
  unregisterMcp: () => invoke<string>("drive_unregister_mcp"),
  setOllamaModels: () => invoke<string>("drive_set_ollama_models"),
  restartOllama: () => invoke<string>("drive_restart_ollama"),
  buildIndex: (onProgress: (p: IndexProgress) => void) => {
    const channel = new Channel<IndexProgress>();
    channel.onmessage = onProgress;
    return invoke<IndexSummary>("drive_index_build", { onProgress: channel });
  },
  kiwixStart: () => invoke<KiwixInfo>("drive_kiwix_start"),
  kiwixStop: () => invoke<boolean>("drive_kiwix_stop"),
  installSkills: (toWorkspace: boolean) => invoke<string[]>("drive_install_skills", { toWorkspace }),
  installHelpers: () => invoke<string[]>("drive_install_helpers"),
  goldenModels: () => invoke<GoldenModel[]>("drive_golden_models"),
};

export function fmtBytes(n: number): string {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} ГБ`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(0)} МБ`;
  if (n >= 1024) return `${(n / 1024).toFixed(0)} КБ`;
  return `${n} Б`;
}
