// Typed wrappers for the ComfyUI "asset factory" commands (src-tauri/src/comfy.rs) and the
// live progress feed from ComfyUI's WebSocket.

import { invoke } from "@tauri-apps/api/core";
export type { ComfySettings } from "./types";

export interface ComfyDevice {
  name: string;
  vramTotalMb: number;
  vramFreeMb: number;
}

export interface ComfyTemplate {
  name: string;
  path: string;
  placeholders: string[];
  nodeCount: number;
  nodeTypes: string[];
  savePrefixes: Array<[string, string]>;
  description: string;
  meta: unknown;
}

export interface ComfyStatus {
  url: string;
  online: boolean;
  version?: string | null;
  devices: ComfyDevice[];
  queueRunning: number;
  queuePending: number;
  error?: string | null;
  startedByApp: boolean;
  dir?: string | null;
  dirValid: boolean;
  python: string;
  checkpoint?: string | null;
  checkpoints: string[];
  workflowsDir: string;
  templates: ComfyTemplate[];
  mcpRegistered: boolean;
  mcpPath: string;
  skillInstalled: boolean;
  ollamaLoaded: string[];
  extraModelPaths: string;
}

export const comfyApi = {
  status: () => invoke<ComfyStatus>("comfy_status"),
  start: () => invoke<string>("comfy_start"),
  stop: () => invoke<boolean>("comfy_stop"),
  registerMcp: () => invoke<string>("comfy_register_mcp"),
  unregisterMcp: () => invoke<string>("comfy_unregister_mcp"),
  installTemplates: () => invoke<string[]>("comfy_install_templates"),
  free: () => invoke<string>("comfy_free"),
  generate: (tool: string, args: Record<string, unknown>) => invoke<unknown>("comfy_generate", { tool, args }),
};

export interface ComfyProgress {
  value: number;
  max: number;
  node: string | null;
  promptId: string | null;
}

/**
 * Follows ComfyUI's WebSocket (`/ws?clientId=…`): JSON frames for `progress`/`executing`/
 * `status`, binary frames for live previews (4-byte event type 1, 4-byte image format
 * 1=JPEG 2=PNG, then the image). Reconnects while `open` and stays quiet when ComfyUI is down.
 */
export function watchComfy(
  httpUrl: string,
  handlers: { onProgress?: (p: ComfyProgress | null) => void; onPreview?: (url: string | null) => void; onQueue?: (remaining: number) => void },
): () => void {
  let ws: WebSocket | null = null;
  let open = true;
  let retry: ReturnType<typeof setTimeout> | null = null;
  let lastPreview: string | null = null;
  const wsUrl = `${httpUrl.replace(/^http/, "ws").replace(/\/$/, "")}/ws?clientId=codewhale-desktop-panel`;

  const connect = () => {
    if (!open) return;
    try {
      ws = new WebSocket(wsUrl);
    } catch {
      retry = setTimeout(connect, 5000);
      return;
    }
    ws.binaryType = "arraybuffer";
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        let msg: any;
        try {
          msg = JSON.parse(ev.data);
        } catch {
          return;
        }
        if (msg.type === "progress") {
          handlers.onProgress?.({ value: msg.data?.value ?? 0, max: msg.data?.max ?? 0, node: msg.data?.node ?? null, promptId: msg.data?.prompt_id ?? null });
        } else if (msg.type === "executing" && msg.data?.node === null) {
          handlers.onProgress?.(null);
          if (lastPreview) URL.revokeObjectURL(lastPreview);
          lastPreview = null;
          handlers.onPreview?.(null);
        } else if (msg.type === "status") {
          handlers.onQueue?.(msg.data?.status?.exec_info?.queue_remaining ?? 0);
        }
        return;
      }
      const buf = ev.data as ArrayBuffer;
      if (buf.byteLength < 8) return;
      const view = new DataView(buf);
      const eventType = view.getUint32(0);
      if (eventType !== 1) return; // PREVIEW_IMAGE
      const format = view.getUint32(4);
      const blob = new Blob([buf.slice(8)], { type: format === 2 ? "image/png" : "image/jpeg" });
      if (lastPreview) URL.revokeObjectURL(lastPreview);
      lastPreview = URL.createObjectURL(blob);
      handlers.onPreview?.(lastPreview);
    };
    ws.onclose = () => {
      ws = null;
      if (open) retry = setTimeout(connect, 5000);
    };
    ws.onerror = () => ws?.close();
  };
  connect();
  return () => {
    open = false;
    if (retry) clearTimeout(retry);
    ws?.close();
    if (lastPreview) URL.revokeObjectURL(lastPreview);
  };
}
