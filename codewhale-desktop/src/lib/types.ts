// Shapes observed from the CodeWhale runtime API (v1, codewhale 0.10.x).

export interface RuntimeInfo {
  state: "stopped" | "starting" | "running" | "failed";
  port?: number | null;
  pid?: number | null;
  binary?: string | null;
  version?: string | null;
  error?: string | null;
  startedAtMs?: number | null;
}

export interface ThreadRecord {
  id: string;
  created_at: string;
  updated_at: string;
  model: string;
  model_provider: string;
  model_provider_id?: string;
  workspace: string;
  mode: string;
  permission_posture?: string;
  allow_shell?: boolean;
  trust_mode?: boolean;
  auto_approve?: boolean;
  archived?: boolean;
  latest_turn_id?: string | null;
  title?: string;
  system_prompt?: string | null;
}

export interface ThreadSummary {
  id: string;
  title: string;
  preview: string;
  model: string;
  mode: string;
  workspace: string;
  branch: string | null;
  head: string | null;
  dirty: boolean;
  archived: boolean;
  updated_at: string;
  latest_turn_id: string | null;
  latest_turn_status: string | null;
  pending_attention_count: number;
}

export interface Usage {
  input_tokens?: number;
  output_tokens?: number;
  prompt_cache_hit_tokens?: number;
  prompt_cache_miss_tokens?: number;
  reasoning_tokens?: number;
}

export interface TurnRecord {
  id: string;
  thread_id: string;
  status: "queued" | "in_progress" | "completed" | "failed" | "interrupted" | "canceled" | string;
  input_summary?: string;
  created_at: string;
  started_at?: string;
  ended_at?: string;
  duration_ms?: number;
  usage?: Usage;
  effective_model?: string;
  error?: unknown;
  item_ids?: string[];
}

export type ItemKind =
  | "user_message"
  | "agent_message"
  | "agent_reasoning"
  | "tool_call"
  | "file_change"
  | "command_execution"
  | "context_compaction"
  | "status"
  | "error"
  | string;

export interface ItemRecord {
  id: string;
  turn_id: string;
  kind: ItemKind;
  status: "in_progress" | "completed" | "failed" | string;
  summary: string;
  detail: string;
  metadata?: Record<string, unknown> | null;
  artifact_refs?: unknown[];
  started_at?: string;
  ended_at?: string;
}

export interface ContextInfo {
  thread_id: string;
  model: string;
  provider: string;
  live: boolean;
  window_tokens: number | null;
  input_tokens: number | null;
  billed_input_tokens: number | null;
  output_cap_tokens: number | null;
  input_budget_ceiling: number | null;
  available_input_tokens: number | null;
  compaction_trigger_tokens: number | null;
  usage_percent: number | null;
  pressure: string;
}

export interface CwEventEnvelope {
  threadId: string;
  seq: number | null;
  event: string;
  data: string;
}

export interface CwEvent {
  schema_version: number;
  seq: number;
  event: string;
  kind: string;
  thread_id: string;
  turn_id: string | null;
  item_id: string | null;
  timestamp: string;
  payload: any;
}

export interface OllamaModel {
  name: string;
  size: number;
  family?: string | null;
  parameterSize?: string | null;
  quantization?: string | null;
  capabilities: string[];
  nativeCtx?: number | null;
  numCtx?: number | null;
  modifiedAt?: string | null;
  embedding: boolean;
}

export interface OllamaStatus {
  online: boolean;
  url: string;
  version?: string;
  loaded?: Array<{
    name: string;
    model: string;
    size: number;
    size_vram: number;
    context_length?: number;
    expires_at?: string;
  }>;
}

export interface GpuStats {
  name: string;
  memUsedMb: number | null;
  memTotalMb: number | null;
  util: number | null;
  tempC: number | null;
}

export interface SavedSession {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  message_count: number;
  total_tokens: number;
  model: string;
  workspace: string;
}

export interface AppSettings {
  ollamaUrl: string;
  codewhalePath: string | null;
  telemetry: boolean;
  workspace: string | null;
  recentWorkspaces: string[];
  defaultModel: string | null;
  toolProfile: "lite" | "full";
  theme: "dark" | "light" | "auto";
  fontMono: string;
  fontSize: number;
  density: "compact" | "comfortable";
  showToolCalls: boolean;
  showThinking: boolean;
  autoApprove: Record<string, boolean>;
  language: string;
  _file?: string;
}
