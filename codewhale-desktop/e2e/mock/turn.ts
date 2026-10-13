/**
 * Scripted turns: what the fake runtime "does" after POST /v1/threads/{id}/turns.
 *
 * A script is an async function over a `TurnCtx`. Each helper appends events to
 * the thread journal exactly like the real runtime (runtime_threads.rs) and, when
 * the UI holds a stream for the thread, delivers them as `cw-event`s.
 *
 *   backend.scriptNextTurn(async (t) => {
 *     await t.start();
 *     await t.gate("before-answer");            // test calls backend.release("before-answer")
 *     await t.message("Привет! Это **markdown**.");
 *     await t.complete();
 *   });
 */
import { itemRecord, recordId } from "./events";

export class TurnInterrupted extends Error {
  constructor() {
    super("turn interrupted");
  }
}

/** What a TurnCtx needs from the backend (kept narrow to avoid a cycle). */
export interface TurnHost {
  journal(threadId: string, event: string, payload: unknown, ids?: { turnId?: string | null; itemId?: string | null }): Promise<void>;
  gate(name: string): Promise<void>;
  registerApproval(a: PendingApproval): Promise<{ decision: string; remember: boolean }>;
  registerUserInput(threadId: string, turnId: string, inputId: string, request: unknown): Promise<unknown>;
  turnRecord(threadId: string, turnId: string): Record<string, any>;
}

export interface PendingApproval {
  id: string;
  threadId: string;
  turnId: string;
  toolCallId: string;
  toolName: string;
  summary: string;
  description: string;
  intentSummary: string;
}

export interface ToolSpec {
  name: string;
  args?: Record<string, unknown>;
  /** Tool result text (becomes the completed item's `detail`). */
  output?: string;
  /** Item kind; defaults from the tool name like `tool_kind_for_name`. */
  kind?: "tool_call" | "command_execution" | "file_change";
  /** Unified diff for file mutations (metadata.mutation.diff). */
  diff?: string;
  files?: Array<{ path: string; outcome?: string }>;
  /** Ask for approval before running (emits approval.required and waits for POST /v1/approvals/{id}). */
  approval?: { summary?: string; description?: string; intent?: string; id?: string };
  fail?: boolean;
  /** Delay between start and completion. */
  durationMs?: number;
}

export interface MessageOptions {
  /** Characters per item.delta (default 8). */
  chunk?: number;
  /** Delay between deltas in ms (default 12). */
  delayMs?: number;
  /** Item kind (agent_message or agent_reasoning). */
  kind?: string;
  /** Gate name awaited after the first delta (to assert mid-stream state). */
  pauseAfterFirstDelta?: string;
}

export type TurnScript = (t: TurnCtx) => Promise<void>;

function toolKind(name: string): ToolSpec["kind"] {
  if (["bash", "Run", "task_shell_start", "exec_shell"].includes(name)) return "command_execution";
  if (["write", "edit", "apply_patch", "fim_edit"].includes(name)) return "file_change";
  return "tool_call";
}

export class TurnCtx {
  interrupted = false;
  private wake!: () => void;
  /** Settles when the turn is interrupted, so sleeps and gates stop waiting. */
  private readonly interruptSignal = new Promise<void>((r) => (this.wake = r));
  /** Seeding mode: no delays, no gates. */
  readonly fast: boolean;
  private readonly startedAt = new Date().toISOString();
  private outputTokens = 0;
  /** Streaming items (message/reasoning) not finished yet, with the text streamed so far. */
  private readonly open = new Map<string, { item: Record<string, any>; text: string }>();

  constructor(
    private readonly host: TurnHost,
    readonly threadId: string,
    readonly turnId: string,
    readonly prompt: string,
    readonly model: string,
    opts: { fast?: boolean } = {},
  ) {
    this.fast = !!opts.fast;
  }

  /** POST .../interrupt: the script stops at its next sleep/gate/emit boundary. */
  requestInterrupt(): void {
    this.interrupted = true;
    this.wake();
  }

  get record(): Record<string, any> {
    return this.host.turnRecord(this.threadId, this.turnId);
  }

  async emit(event: string, payload: unknown, itemId: string | null = null): Promise<void> {
    await this.host.journal(this.threadId, event, payload, { turnId: this.turnId, itemId });
  }

  async sleep(ms: number): Promise<void> {
    if (this.interrupted) throw new TurnInterrupted();
    if (this.fast || ms <= 0) return;
    await Promise.race([new Promise((r) => setTimeout(r, ms)), this.interruptSignal]);
    if (this.interrupted) throw new TurnInterrupted();
  }

  async gate(name: string): Promise<void> {
    if (this.interrupted) throw new TurnInterrupted();
    if (this.fast) return;
    await Promise.race([this.host.gate(name), this.interruptSignal]);
    if (this.interrupted) throw new TurnInterrupted();
  }

  /** turn.started + the user_message item + turn.lifecycle in_progress (the runtime's opening sequence). */
  async start(): Promise<void> {
    const rec = this.record;
    rec.status = "in_progress";
    rec.started_at = this.startedAt;
    await this.emit("turn.started", { turn: { ...rec } });
    const user = itemRecord({
      turnId: this.turnId,
      kind: "user_message",
      status: "completed",
      summary: this.prompt.slice(0, 120),
      detail: this.prompt,
      endedAt: this.startedAt,
      startedAt: this.startedAt,
    });
    rec.item_ids = [...(rec.item_ids ?? []), user.id];
    await this.emit("item.started", { item: user }, user.id);
    await this.emit("item.completed", { item: user }, user.id);
    await this.emit("turn.lifecycle", { status: "in_progress" });
  }

  /** Streams an agent message as item.started -> item.delta* -> item.completed. Returns the item id. */
  async message(text: string, opts: MessageOptions = {}): Promise<string> {
    const kind = opts.kind ?? "agent_message";
    const chunk = Math.max(1, opts.chunk ?? 8);
    const delay = opts.delayMs ?? 12;
    const item = itemRecord({ turnId: this.turnId, kind, summary: "", detail: "" });
    this.record.item_ids = [...(this.record.item_ids ?? []), item.id];
    const open = { item, text: "" };
    this.open.set(item.id, open);
    await this.emit("item.started", { item }, item.id);
    for (let i = 0; i < text.length; i += chunk) {
      const delta = text.slice(i, i + chunk);
      open.text += delta;
      // Real deltas carry `delta` and `kind` only (fixtures/live/mock_10_turn_text__sse.json).
      await this.emit("item.delta", { delta, kind }, item.id);
      if (i === 0 && opts.pauseAfterFirstDelta) await this.gate(opts.pauseAfterFirstDelta);
      await this.sleep(delay);
    }
    this.open.delete(item.id);
    this.outputTokens += Math.ceil(text.length / 4);
    const done = { ...item, status: "completed", summary: text.slice(0, 120), detail: text, ended_at: new Date().toISOString() };
    await this.emit("item.completed", { item: done }, item.id);
    return item.id;
  }

  reasoning(text: string, opts: Omit<MessageOptions, "kind"> = {}): Promise<string> {
    return this.message(text, { ...opts, kind: "agent_reasoning" });
  }

  /** A tool call, optionally gated by an approval. Resolves with the decision ("allow" when none was needed). */
  async tool(spec: ToolSpec): Promise<string> {
    const kind = spec.kind ?? toolKind(spec.name);
    const toolUseId = `call_${recordId("t").slice(2, 18)}`;
    const input = JSON.stringify(spec.args ?? {});
    const metadata: Record<string, unknown> = {
      tool_use_id: toolUseId,
      tool_name: spec.name,
      tool_input: input,
      execution_id: toolUseId,
    };
    const item = itemRecord({ turnId: this.turnId, kind: kind!, summary: `${spec.name} started`, detail: input, metadata });
    this.record.item_ids = [...(this.record.item_ids ?? []), item.id];
    await this.emit("item.started", { item, tool: { id: toolUseId, name: spec.name, input: spec.args ?? {} } }, item.id);

    let decision = "allow";
    if (spec.approval) {
      const approvalId = spec.approval.id ?? recordId("appr");
      const summary = spec.approval.summary ?? `${spec.name}: ${Object.values(spec.args ?? {})[0] ?? ""}`.trim();
      const description = spec.approval.description ?? `Tool \`${spec.name}\` requires approval.`;
      const intentSummary = spec.approval.intent ?? "";
      const wait = this.host.registerApproval({
        id: approvalId,
        threadId: this.threadId,
        turnId: this.turnId,
        toolCallId: toolUseId,
        toolName: spec.name,
        summary,
        description,
        intentSummary,
      });
      await this.emit("approval.required", {
        id: approvalId,
        approval_id: approvalId,
        tool_call_id: toolUseId,
        tool_name: spec.name,
        summary,
        description,
        intent_summary: intentSummary,
      });
      const d = await wait;
      decision = d.decision;
      await this.emit("approval.decided", {
        approval_id: approvalId,
        tool_call_id: toolUseId,
        decision: d.decision,
        remember: d.remember,
        auto: false,
      });
    }

    await this.sleep(spec.durationMs ?? 30);
    const denied = decision !== "allow";
    const failed = denied || !!spec.fail;
    const output = denied ? `Tool call denied by user: ${spec.name}` : spec.output ?? "ok";
    const meta: Record<string, unknown> = { ...metadata, tool_result_for: toolUseId };
    if (spec.diff || spec.files) {
      meta.mutation = { diff: spec.diff, files: spec.files ?? [] };
    }
    const done = {
      ...item,
      status: failed ? "failed" : "completed",
      summary: `${spec.name}: ${failed ? "error" : "ok"}`,
      detail: output,
      metadata: meta,
      ended_at: new Date().toISOString(),
    };
    await this.emit(failed ? "item.failed" : "item.completed", { item: done }, item.id);
    return decision;
  }

  /** request_user_input: emits user_input.required and waits for POST /v1/user-input/{thread}/{id}. */
  async askUser(questions: unknown[]): Promise<unknown> {
    const inputId = recordId("input");
    const request = { questions };
    const wait = this.host.registerUserInput(this.threadId, this.turnId, inputId, request);
    await this.emit("user_input.required", { id: inputId, input_id: inputId, request });
    const answers = await wait;
    await this.emit("user_input.answered", { id: inputId, input_id: inputId });
    return answers;
  }

  async status(text: string): Promise<void> {
    const item = itemRecord({ turnId: this.turnId, kind: "status", status: "completed", summary: text, detail: text, endedAt: new Date().toISOString() });
    await this.emit("item.completed", { item }, item.id);
  }

  async usage(usage: { input_tokens?: number; output_tokens?: number } = {}, durationMs = 900): Promise<void> {
    await this.emit("turn.usage", {
      usage: { input_tokens: usage.input_tokens ?? 3120, output_tokens: usage.output_tokens ?? Math.max(1, this.outputTokens) },
      duration_ms: durationMs,
      first_token_ms: 240,
    });
  }

  private async finish(status: string, event: string, extra: Record<string, unknown> = {}): Promise<void> {
    const rec = this.record;
    const ended = new Date();
    Object.assign(rec, {
      status,
      ended_at: ended.toISOString(),
      duration_ms: Math.max(1, ended.getTime() - new Date(this.startedAt).getTime()),
      ...extra,
    });
    // The runtime ends a turn with turn.completed / turn.failed / turn.interrupted only
    // (no closing turn.lifecycle), see fixtures/live/mock_*__sse.json.
    await this.emit(event, { turn: { ...rec } });
  }

  async complete(usage?: { input_tokens?: number; output_tokens?: number }): Promise<void> {
    await this.usage(usage);
    const u = { input_tokens: usage?.input_tokens ?? 3120, output_tokens: usage?.output_tokens ?? Math.max(1, this.outputTokens) };
    await this.finish("completed", "turn.completed", { usage: u });
  }

  /** Closes streaming items the way runtime_threads.rs does when a turn ends early. */
  private async closeOpenItems(status: "interrupted" | "failed"): Promise<void> {
    for (const { item, text } of this.open.values()) {
      const done = { ...item, status, summary: text.slice(0, 120), detail: text, ended_at: new Date().toISOString() };
      await this.emit(status === "interrupted" ? "item.interrupted" : "item.failed", { item: done }, item.id);
    }
    this.open.clear();
  }

  /**
   * Failed turn as the runtime reports it: open items fail, an `error` item
   * carries the message, then `turn.completed` with status "failed" and a string
   * `error` (fixtures/live/offline_40_turn_no_backend__sse.json). The runtime
   * never emits `turn.failed`.
   */
  async fail(message: string): Promise<void> {
    await this.closeOpenItems("failed");
    const err = itemRecord({ turnId: this.turnId, kind: "error", status: "failed", summary: message.slice(0, 200), detail: message, endedAt: new Date().toISOString() });
    await this.emit("item.failed", { item: err }, err.id);
    await this.finish("failed", "turn.completed", { error: message, usage: { input_tokens: 0, output_tokens: 0 } });
  }

  /**
   * Interrupted turn (POST .../interrupt): open items end with `item.interrupted`,
   * a "Request cancelled" status item, then `turn.completed` with status
   * "interrupted" (fixtures/live/mock_14_turn_interrupt__sse.json).
   */
  async markInterrupted(): Promise<void> {
    await this.status("Request cancelled");
    await this.closeOpenItems("interrupted");
    await this.finish("interrupted", "turn.completed");
  }
}

/** Default answer: a short streamed reply. */
export const defaultTurnScript: TurnScript = async (t) => {
  await t.start();
  await t.message(`Готово. Вы написали: «${t.prompt}».`);
  await t.complete();
};
