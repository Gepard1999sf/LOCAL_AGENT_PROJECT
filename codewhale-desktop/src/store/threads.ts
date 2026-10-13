import { create } from "zustand";
import type { CwEvent, CwEventEnvelope, ItemRecord, ThreadRecord, TurnRecord, Usage } from "../lib/types";
import { findToolArgs, findToolName } from "../lib/tools";

export interface ItemView extends ItemRecord {
  /** Text accumulated from deltas; replaced by `detail` when the item completes. */
  text: string;
  toolName?: string;
  toolArgs?: Record<string, unknown>;
  /** Provider-independent tool call id (matches approval.tool_call_id). */
  toolUseId?: string;
  /** Unified diff of a file mutation (metadata.mutation.diff). */
  diff?: string;
  changedFiles?: Array<{ path: string; outcome?: string }>;
  internal?: boolean;
}

export interface Approval {
  id: string;
  turnId: string | null;
  toolCallId?: string;
  toolName?: string;
  args?: Record<string, unknown>;
  payload: any;
  status: "pending" | "allowed" | "denied" | "timeout" | "cancelled";
  createdAt: string;
  seq: number;
}

export interface UserInputRequest {
  id: string;
  turnId: string | null;
  payload: any;
  status: "pending" | "answered" | "canceled";
}

export interface TurnView {
  id: string;
  status: string;
  prompt: string;
  createdAt: string;
  startedAt?: string;
  endedAt?: string;
  durationMs?: number;
  usage?: Usage;
  model?: string;
  /** Ordered item ids, excluding the user message. */
  blocks: string[];
  approvals: string[];
  error?: string;
  changedPaths: string[];
  /** Last generation speed in tokens per second. */
  tokPerSec?: number;
  firstTokenMs?: number;
  outputTokens: number;
  requests: number;
}

export interface ToolCallLog {
  itemId: string;
  turnId: string;
  name: string;
  arg?: string;
  status: string;
  startedAt?: string;
  endedAt?: string;
  kind: string;
}

export interface ThreadView {
  id: string;
  thread?: ThreadRecord;
  turns: TurnView[];
  turnIndex: Record<string, number>;
  items: Record<string, ItemView>;
  approvals: Record<string, Approval>;
  userInputs: Record<string, UserInputRequest>;
  lastSeq: number;
  /** Replay of history finished (stream reported "live"). */
  live: boolean;
  streamState: string;
  /** Files read / changed during the session (from tool items and snapshots). */
  filesChanged: Record<string, number>;
  filesRead: Record<string, number>;
  /** tool_use_id -> item id, to show arguments on approval cards. */
  toolCalls: Record<string, string>;
  activeTurnId?: string;
  lastError?: string;
}

function emptyThread(id: string): ThreadView {
  return {
    id,
    turns: [],
    turnIndex: {},
    items: {},
    approvals: {},
    userInputs: {},
    lastSeq: 0,
    live: false,
    streamState: "connecting",
    filesChanged: {},
    filesRead: {},
    toolCalls: {},
  };
}

function ensureTurn(t: ThreadView, turnId: string, ts: string): TurnView {
  let idx = t.turnIndex[turnId];
  if (idx === undefined) {
    const turn: TurnView = {
      id: turnId,
      status: "in_progress",
      prompt: "",
      createdAt: ts,
      blocks: [],
      approvals: [],
      changedPaths: [],
      outputTokens: 0,
      requests: 0,
    };
    t.turns = [...t.turns, turn];
    idx = t.turns.length - 1;
    t.turnIndex = { ...t.turnIndex, [turnId]: idx };
  }
  return t.turns[idx];
}

function patchTurn(t: ThreadView, turnId: string, ts: string, fn: (turn: TurnView) => TurnView) {
  const cur = ensureTurn(t, turnId, ts);
  const idx = t.turnIndex[turnId];
  const next = fn({ ...cur });
  const turns = t.turns.slice();
  turns[idx] = next;
  t.turns = turns;
}

function mergeTurnRecord(turn: TurnView, rec: TurnRecord): TurnView {
  return {
    ...turn,
    status: rec.status ?? turn.status,
    prompt: turn.prompt || rec.input_summary || "",
    createdAt: rec.created_at ?? turn.createdAt,
    startedAt: rec.started_at ?? turn.startedAt,
    endedAt: rec.ended_at ?? turn.endedAt,
    durationMs: rec.duration_ms ?? turn.durationMs,
    usage: rec.usage ?? turn.usage,
    model: rec.effective_model ?? turn.model,
    error: rec.error ? errorText(rec.error) : turn.error,
  };
}

function errorText(e: unknown): string {
  if (!e) return "";
  if (typeof e === "string") return e;
  const any = e as any;
  return any.message ?? any.error?.message ?? JSON.stringify(e);
}

const PATH_KEYS = ["path", "file_path", "file", "target"];

function trackFiles(t: ThreadView, item: ItemView) {
  if (item.changedFiles?.length) {
    const fc = { ...t.filesChanged };
    for (const f of item.changedFiles) fc[f.path] = (fc[f.path] ?? 0) + 1;
    t.filesChanged = fc;
    return;
  }
  const args = item.toolArgs;
  if (!args) return;
  const p = PATH_KEYS.map((k) => args[k]).find((v) => typeof v === "string") as string | undefined;
  if (!p) return;
  const name = item.toolName ?? "";
  if (["write", "edit", "apply_patch", "fim_edit"].includes(name) || item.kind === "file_change") {
    t.filesChanged = { ...t.filesChanged, [p]: (t.filesChanged[p] ?? 0) + 1 };
  } else if (["read", "read_file", "handle_read"].includes(name)) {
    t.filesRead = { ...t.filesRead, [p]: (t.filesRead[p] ?? 0) + 1 };
  }
}

function upsertItem(t: ThreadView, rec: ItemRecord, ts: string, completed: boolean) {
  const prev = t.items[rec.id];
  const meta = (rec.metadata ?? {}) as any;
  const isTool = ["tool_call", "file_change", "command_execution"].includes(rec.kind);
  const toolName = (isTool ? findToolName(meta) : undefined) ?? prev?.toolName;
  const toolArgs = (isTool ? findToolArgs(meta) : undefined) ?? prev?.toolArgs;
  const toolUseId: string | undefined = meta.tool_use_id ?? meta.tool_result_for ?? prev?.toolUseId;
  const diff: string | undefined = meta.mutation?.diff ?? prev?.diff;
  const changedFiles = Array.isArray(meta.mutation?.files)
    ? meta.mutation.files.map((f: any) => ({ path: f.path, outcome: f.outcome }))
    : prev?.changedFiles;
  // For a started tool item, detail holds the raw arguments; keep text empty until output arrives.
  let text = completed && rec.detail ? rec.detail : prev?.text || "";
  if (!isTool && !text) text = rec.detail || "";
  if (isTool && completed) text = text.replace(/^\[approval\][^\n]*\n+/, "");
  const internal = meta.visibility === "internal";
  const item: ItemView = { ...prev, ...rec, text, toolName, toolArgs, toolUseId, diff, changedFiles, internal };
  t.items = { ...t.items, [rec.id]: item };
  if (toolUseId && isTool) t.toolCalls = { ...t.toolCalls, [toolUseId]: rec.id };
  if (internal) return;
  if (rec.kind === "user_message") {
    patchTurn(t, rec.turn_id, ts, (turn) => ({ ...turn, prompt: rec.detail || rec.summary || turn.prompt }));
  } else {
    patchTurn(t, rec.turn_id, ts, (turn) =>
      turn.blocks.includes(rec.id) ? turn : { ...turn, blocks: [...turn.blocks, rec.id] },
    );
  }
  if (completed) trackFiles(t, item);
}

function applyOne(t: ThreadView, env: CwEventEnvelope, ev: CwEvent) {
  const p = ev.payload ?? {};
  const ts = ev.timestamp ?? new Date().toISOString();
  if (typeof ev.seq === "number" && ev.seq > t.lastSeq) t.lastSeq = ev.seq;

  switch (env.event) {
    case "stream.progress":
      if (p.state === "live") t.live = true;
      return;
    case "thread.started":
    case "thread.updated":
    case "thread.forked":
      if (p.thread) t.thread = { ...t.thread, ...p.thread };
      return;
    case "turn.started":
    case "turn.completed":
    case "turn.failed":
    case "turn.interrupted":
    case "turn.canceled":
      if (p.turn) {
        patchTurn(t, p.turn.id, ts, (turn) => mergeTurnRecord(turn, p.turn));
        if (env.event === "turn.started") t.activeTurnId = p.turn.id;
        else if (t.activeTurnId === p.turn.id) t.activeTurnId = undefined;
        if (env.event === "turn.failed") t.lastError = errorText(p.turn.error ?? p.error);
      }
      return;
    case "turn.lifecycle":
      if (ev.turn_id && p.status) {
        patchTurn(t, ev.turn_id, ts, (turn) => ({ ...turn, status: p.status }));
        if (["completed", "failed", "interrupted", "canceled"].includes(p.status) && t.activeTurnId === ev.turn_id) {
          t.activeTurnId = undefined;
        }
      }
      return;
    case "turn.usage":
      if (ev.turn_id) {
        const out = p.usage?.output_tokens ?? 0;
        const dur = p.duration_ms ?? 0;
        patchTurn(t, ev.turn_id, ts, (turn) => ({
          ...turn,
          usage: p.usage ?? turn.usage,
          outputTokens: turn.outputTokens + out,
          requests: turn.requests + 1,
          tokPerSec: out > 0 && dur > 0 ? out / (dur / 1000) : turn.tokPerSec,
          firstTokenMs: p.first_token_ms ?? turn.firstTokenMs,
        }));
      }
      return;
    case "turn.workspace_snapshot":
      if (ev.turn_id && Array.isArray(p.changed_paths) && p.changed_paths.length) {
        patchTurn(t, ev.turn_id, ts, (turn) => ({ ...turn, changedPaths: p.changed_paths }));
        const fc = { ...t.filesChanged };
        for (const path of p.changed_paths) fc[path] = (fc[path] ?? 0) + 1;
        t.filesChanged = fc;
      }
      return;
    case "item.started":
      if (p.item) upsertItem(t, p.item, ts, false);
      return;
    case "item.completed":
    case "item.failed":
      if (p.item) upsertItem(t, { ...p.item, status: env.event === "item.failed" ? "failed" : p.item.status }, ts, true);
      return;
    case "item.delta": {
      const id = ev.item_id;
      if (!id) return;
      const prev = t.items[id];
      if (!prev) {
        upsertItem(
          t,
          {
            id,
            turn_id: ev.turn_id ?? "",
            kind: p.kind ?? "agent_message",
            status: "in_progress",
            summary: "",
            detail: "",
          },
          ts,
          false,
        );
      }
      const cur = t.items[id];
      t.items = { ...t.items, [id]: { ...cur, text: (cur.text ?? "") + (p.delta ?? "") } };
      return;
    }
    case "approval.required": {
      const id = p.approval_id ?? p.id;
      if (!id) return;
      const approval: Approval = {
        id,
        turnId: ev.turn_id,
        toolCallId: p.tool_call_id,
        toolName: findToolName(p),
        args: findToolArgs(p),
        payload: p,
        status: "pending",
        createdAt: ts,
        seq: ev.seq,
      };
      t.approvals = { ...t.approvals, [id]: approval };
      if (ev.turn_id) {
        // Approvals live in `blocks` as "approval:<id>" so they render in arrival order.
        patchTurn(t, ev.turn_id, ts, (turn) =>
          turn.approvals.includes(id)
            ? turn
            : { ...turn, approvals: [...turn.approvals, id], blocks: [...turn.blocks, `approval:${id}`] },
        );
      }
      return;
    }
    case "approval.decided":
    case "approval.timeout": {
      const id = p.approval_id ?? p.id;
      const prev = id ? t.approvals[id] : undefined;
      if (!prev) return;
      let status: Approval["status"] = "allowed";
      if (env.event === "approval.timeout" || p.timeout) status = "timeout";
      else if (p.cancelled) status = "cancelled";
      else if (String(p.decision ?? p.outcome ?? "").includes("den")) status = "denied";
      t.approvals = { ...t.approvals, [id]: { ...prev, status } };
      return;
    }
    case "user_input.required": {
      const id = p.input_id ?? p.id;
      if (!id) return;
      t.userInputs = { ...t.userInputs, [id]: { id, turnId: ev.turn_id, payload: p, status: "pending" } };
      return;
    }
    case "user_input.answered":
    case "user_input.canceled": {
      const id = p.input_id ?? p.id;
      const prev = id ? t.userInputs[id] : undefined;
      if (!prev) return;
      t.userInputs = {
        ...t.userInputs,
        [id]: { ...prev, status: env.event === "user_input.answered" ? "answered" : "canceled" },
      };
      return;
    }
    default:
      return;
  }
}

interface ThreadsState {
  threads: Record<string, ThreadView>;
  applyBatch: (envs: CwEventEnvelope[]) => void;
  setStream: (threadId: string, state: string, message?: string | null) => void;
  reset: (threadId: string) => void;
  markApproval: (threadId: string, approvalId: string, status: Approval["status"]) => void;
}

export const useThreads = create<ThreadsState>((set) => ({
  threads: {},
  applyBatch: (envs) =>
    set((s) => {
      const threads = { ...s.threads };
      const touched = new Map<string, ThreadView>();
      for (const env of envs) {
        let t = touched.get(env.threadId);
        if (!t) {
          t = { ...(threads[env.threadId] ?? emptyThread(env.threadId)) };
          touched.set(env.threadId, t);
        }
        let ev: CwEvent;
        try {
          ev = JSON.parse(env.data);
        } catch {
          continue;
        }
        // Drop duplicates after reconnects.
        if (typeof ev.seq === "number" && ev.seq <= t.lastSeq && env.event !== "stream.progress") continue;
        applyOne(t, env, ev);
      }
      for (const [id, t] of touched) threads[id] = t;
      return { threads };
    }),
  setStream: (threadId, state, message) =>
    set((s) => {
      const t = s.threads[threadId] ?? emptyThread(threadId);
      return {
        threads: {
          ...s.threads,
          [threadId]: { ...t, streamState: state, lastError: state === "error" ? message ?? t.lastError : t.lastError },
        },
      };
    }),
  reset: (threadId) => set((s) => ({ threads: { ...s.threads, [threadId]: emptyThread(threadId) } })),
  markApproval: (threadId, approvalId, status) =>
    set((s) => {
      const t = s.threads[threadId];
      const a = t?.approvals[approvalId];
      if (!t || !a) return s;
      return {
        threads: { ...s.threads, [threadId]: { ...t, approvals: { ...t.approvals, [approvalId]: { ...a, status } } } },
      };
    }),
}));

// ---- Frame-batched event intake ----
let queue: CwEventEnvelope[] = [];
let scheduled = false;

function flush() {
  scheduled = false;
  const batch = queue;
  queue = [];
  if (batch.length) useThreads.getState().applyBatch(batch);
}

export function enqueueEvent(env: CwEventEnvelope) {
  queue.push(env);
  if (!scheduled) {
    scheduled = true;
    if (document.visibilityState === "visible") requestAnimationFrame(flush);
    else setTimeout(flush, 50);
  }
}

/** Derived activity of a thread for the status bar. */
export function threadActivity(t?: ThreadView): "idle" | "thinking" | "tool" | "approval" | "error" {
  if (!t) return "idle";
  if (Object.values(t.approvals).some((a) => a.status === "pending")) return "approval";
  if (!t.activeTurnId) return t.turns.at(-1)?.status === "failed" ? "error" : "idle";
  const turn = t.turns[t.turnIndex[t.activeTurnId]];
  const last = turn?.blocks.at(-1);
  const item = last ? t.items[last] : undefined;
  if (item && item.status === "in_progress" && ["tool_call", "command_execution", "file_change"].includes(item.kind)) {
    return "tool";
  }
  return "thinking";
}

export function toolLog(t?: ThreadView): ToolCallLog[] {
  if (!t) return [];
  const out: ToolCallLog[] = [];
  for (const turn of t.turns) {
    for (const id of turn.blocks) {
      const it = t.items[id];
      if (!it || !["tool_call", "command_execution", "file_change"].includes(it.kind)) continue;
      const args = it.toolArgs;
      const arg = args
        ? (["command", "path", "file_path", "pattern", "query"].map((k) => args[k]).find((v) => typeof v === "string") as
            | string
            | undefined)
        : undefined;
      out.push({
        itemId: id,
        turnId: turn.id,
        name: it.toolName ?? it.kind,
        arg: arg ?? it.summary,
        status: it.status,
        startedAt: it.started_at,
        endedAt: it.ended_at,
        kind: it.kind,
      });
    }
  }
  return out;
}
