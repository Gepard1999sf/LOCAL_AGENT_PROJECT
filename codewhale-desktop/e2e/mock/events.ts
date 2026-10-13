/**
 * Builders for CodeWhale runtime SSE frames, as the desktop backend forwards
 * them in `cw-event` Tauri events (src-tauri/src/api.rs `CwEvent`):
 *
 *   { threadId, seq, event, data }   // `data` is the raw JSON text of the SSE frame
 *
 * The frame JSON follows `runtime_event_payload_with_previous` in
 * crates/tui/src/runtime_api.rs (journal events) and `thread_stream_progress`
 * (transport frames without a journal seq).
 */

export const ENVELOPE_SCHEMA_VERSION = 1;
export const RECORD_SCHEMA_VERSION = 2;

export interface CwEventEnvelope {
  threadId: string;
  seq: number | null;
  event: string;
  data: string;
}

export interface JournalEvent {
  schema_version: number;
  seq: number;
  event: string;
  kind: string;
  thread_id: string;
  turn_id: string | null;
  item_id: string | null;
  timestamp: string;
  created_at: string;
  payload: any;
  previous_seq: number;
}

let idCounter = 0;
/** Runtime-style record id: `<prefix>_<32 hex>`. */
export function recordId(prefix: string): string {
  idCounter += 1;
  const rand = Math.random().toString(16).slice(2).padEnd(14, "0").slice(0, 14);
  const n = idCounter.toString(16).padStart(6, "0");
  return `${prefix}_${(Date.now().toString(16) + n + rand).padEnd(32, "0").slice(0, 32)}`;
}

export function journalEvent(opts: {
  seq: number;
  previousSeq: number;
  event: string;
  threadId: string;
  turnId?: string | null;
  itemId?: string | null;
  payload: any;
  timestamp?: string;
}): JournalEvent {
  const ts = (opts.timestamp ?? new Date().toISOString()).replace(/Z$/, "+00:00");
  return {
    schema_version: ENVELOPE_SCHEMA_VERSION,
    seq: opts.seq,
    event: opts.event,
    kind: opts.event,
    thread_id: opts.threadId,
    turn_id: opts.turnId ?? null,
    item_id: opts.itemId ?? null,
    timestamp: ts,
    created_at: ts,
    payload: opts.payload,
    previous_seq: opts.previousSeq,
  };
}

export function toEnvelope(ev: JournalEvent): CwEventEnvelope {
  return { threadId: ev.thread_id, seq: ev.seq, event: ev.event, data: JSON.stringify(ev) };
}

/** `stream.progress` transport frame (no journal seq, SSE id is null). */
export function progressEnvelope(threadId: string, seq: number, live: boolean): CwEventEnvelope {
  return {
    threadId,
    seq: null,
    event: "stream.progress",
    data: JSON.stringify({
      schema_version: ENVELOPE_SCHEMA_VERSION,
      event: "stream.progress",
      kind: "stream.progress",
      thread_id: threadId,
      seq,
      state: live ? "live" : "replaying",
    }),
  };
}

/** `stream.end` frame the server sends before it closes a stream. */
export function streamEndEnvelope(threadId: string, lastSeq: number, reason = "runtime_shutdown"): CwEventEnvelope {
  return {
    threadId,
    seq: null,
    event: "stream.end",
    data: JSON.stringify({
      schema_version: ENVELOPE_SCHEMA_VERSION,
      event: "stream.end",
      kind: "stream.end",
      thread_id: threadId,
      reason,
      last_seq: lastSeq,
      retryable: true,
    }),
  };
}

/** A `TurnItemRecord` (crates/tui/src/runtime_threads.rs). */
export function itemRecord(fields: {
  id?: string;
  turnId: string;
  kind: string;
  status?: string;
  summary?: string;
  detail?: string | null;
  metadata?: Record<string, unknown> | null;
  startedAt?: string;
  endedAt?: string;
}) {
  const rec: Record<string, unknown> = {
    schema_version: RECORD_SCHEMA_VERSION,
    id: fields.id ?? recordId("item"),
    turn_id: fields.turnId,
    kind: fields.kind,
    status: fields.status ?? "in_progress",
    summary: fields.summary ?? "",
    artifact_refs: [],
    started_at: fields.startedAt ?? new Date().toISOString(),
  };
  if (fields.detail != null) rec.detail = fields.detail;
  if (fields.metadata) rec.metadata = fields.metadata;
  if (fields.endedAt) rec.ended_at = fields.endedAt;
  return rec as {
    id: string;
    turn_id: string;
    kind: string;
    status: string;
    summary: string;
    detail?: string;
    metadata?: Record<string, unknown>;
    [k: string]: unknown;
  };
}

/** A small unified diff, handy for diff-rendering tests. */
export const SAMPLE_DIFF = [
  "--- a/src/main.rs",
  "+++ b/src/main.rs",
  "@@ -1,4 +1,5 @@",
  " fn main() {",
  '-    println!("Hello, world!");',
  '+    let name = "CodeWhale";',
  '+    println!("Hello, {name}!");',
  " }",
  " ",
].join("\n");
