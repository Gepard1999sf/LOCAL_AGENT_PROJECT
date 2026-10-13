/**
 * Runtime API fixture store.
 *
 * Two sources share one on-disk format (written by `fixtures/live/record_live.py`):
 *
 *   { "endpoint": "POST /v1/threads", "phase": "offline" | "mock" | "synthetic",
 *     "request": { method, path, body }, "status": 201,
 *     "response": { "content_type", "body" } }                      // JSON exchange
 *   { "endpoint": "GET /v1/threads/{id}/events", "events": [{ event, id, data }] }  // SSE capture
 *
 * - `fixtures/live/*.json`      recorded from a real `codewhale app-server --http`
 *                               (may be absent, or rewritten while tests run).
 * - `fixtures/synthetic/*.json` hand-written from the server structs; a file may
 *                               hold one recording or an array of them.
 *
 * Live recordings win over synthetic ones (and phase "mock" over "offline"), so
 * the fake backend answers with real server shapes whenever they are available.
 * Set `CW_E2E_FIXTURES=synthetic` to ignore the live directory.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
export const LIVE_DIR = path.resolve(here, "../fixtures/live");
export const SYNTHETIC_DIR = path.resolve(here, "../fixtures/synthetic");

export interface SseFrame {
  event: string;
  id: string | null;
  data: any;
}

export interface Recording {
  file: string;
  source: "live" | "synthetic";
  endpoint: string;
  phase: string;
  request?: { method: string; path: string; body?: unknown };
  status?: number;
  response?: { content_type?: string; body: any };
  events?: SseFrame[];
}

const PHASE_RANK: Record<string, number> = { mock: 0, offline: 1, synthetic: 2 };

function readRecordings(dir: string, source: Recording["source"]): Recording[] {
  if (!fs.existsSync(dir)) return [];
  const out: Recording[] = [];
  for (const name of fs.readdirSync(dir).sort()) {
    if (!name.endsWith(".json") || name.startsWith(".")) continue;
    const file = path.join(dir, name);
    let parsed: unknown;
    try {
      parsed = JSON.parse(fs.readFileSync(file, "utf8"));
    } catch {
      // Half-written by the recorder or malformed: the synthetic fallback covers it.
      continue;
    }
    for (const r of Array.isArray(parsed) ? parsed : [parsed]) {
      if (!r || typeof r !== "object" || typeof (r as any).endpoint !== "string") continue;
      out.push({ ...(r as any), file, source, phase: (r as any).phase ?? source });
    }
  }
  return out;
}

/** Endpoint key without the parenthesized variant note: "GET /v1/threads/{id} (unknown id)" -> "GET /v1/threads/{id}". */
export function baseEndpoint(endpoint: string): string {
  return endpoint.replace(/\s*\(.*\)\s*$/, "").trim();
}

export class FixtureStore {
  readonly recordings: Recording[];

  constructor(recordings: Recording[]) {
    this.recordings = recordings.slice().sort((a, b) => {
      if (a.source !== b.source) return a.source === "live" ? -1 : 1;
      return (PHASE_RANK[a.phase] ?? 9) - (PHASE_RANK[b.phase] ?? 9);
    });
  }

  static load(opts: { live?: boolean } = {}): FixtureStore {
    const useLive = opts.live ?? process.env.CW_E2E_FIXTURES !== "synthetic";
    return new FixtureStore([
      ...(useLive ? readRecordings(LIVE_DIR, "live") : []),
      ...readRecordings(SYNTHETIC_DIR, "synthetic"),
    ]);
  }

  get hasLive(): boolean {
    return this.recordings.some((r) => r.source === "live");
  }

  /** All exchanges for an endpoint (variants included), best first. */
  all(endpoint: string): Recording[] {
    const key = baseEndpoint(endpoint);
    return this.recordings.filter((r) => baseEndpoint(r.endpoint) === key);
  }

  /** The best recording whose status passes `ok` (2xx by default). */
  find(endpoint: string, ok: (status: number) => boolean = (s) => s >= 200 && s < 300): Recording | undefined {
    return this.all(endpoint).find((r) => r.response && typeof r.status === "number" && ok(r.status));
  }

  /** Deep copy of the best 2xx response body, or `fallback` when nothing is recorded. */
  body<T = any>(endpoint: string, fallback?: T): T {
    const r = this.find(endpoint);
    if (!r) {
      if (fallback !== undefined) return clone(fallback);
      throw new Error(`fixtures: no successful recording for "${endpoint}"`);
    }
    return clone(r.response!.body);
  }

  /** Error message the server returned for an endpoint variant, e.g. an unknown id. */
  errorMessage(endpoint: string, status: number, fallback: string): string {
    const r = this.all(endpoint).find((x) => x.status === status && x.response);
    const msg = r?.response?.body?.error?.message;
    return typeof msg === "string" ? msg : fallback;
  }

  /** Where the template of `endpoint` comes from (for test annotations). */
  sourceOf(endpoint: string): Recording["source"] | undefined {
    return this.find(endpoint)?.source;
  }

  /** SSE captures, best first. */
  sse(): Recording[] {
    return this.recordings.filter((r) => Array.isArray(r.events) && r.events.length > 0);
  }
}

export function clone<T>(v: T): T {
  return v === undefined ? v : JSON.parse(JSON.stringify(v));
}

/** Returns `template` with `values` applied; keys the template lacks are added. */
export function shaped<T extends Record<string, any>>(template: Record<string, any> | undefined, values: Partial<T>): T {
  return { ...(template ? clone(template) : {}), ...values } as T;
}
