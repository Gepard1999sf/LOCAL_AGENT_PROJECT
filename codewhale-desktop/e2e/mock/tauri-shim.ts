/**
 * Browser-side stand-in for the Tauri v2 IPC bridge.
 *
 * `installTauriShim` is passed to `page.addInitScript`, so Playwright serializes
 * it with `Function.prototype.toString()`: it must be fully self-contained (no
 * imports, no closures over module scope). It reproduces what
 * `@tauri-apps/api` v2 reads from `window.__TAURI_INTERNALS__`:
 *
 * - `invoke(cmd, args, options)`  - core.invoke() delegates here.
 * - `transformCallback(cb, once)` - used by event.listen() and Channel.
 * - `unregisterCallback(id)`, `runCallback(id, data)`, `callbacks`.
 * - `convertFileSrc(path, protocol)`, `metadata` (current window/webview).
 * - `window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener(event, id)`.
 *
 * The `plugin:event|listen|unlisten|emit` commands are served in the page so
 * listener registration is synchronous with the app. Every other command is
 * forwarded to the Playwright binding `window.__cwMockInvoke`, which the Node
 * side (`MockBackend`) answers. Node pushes events back with
 * `window.__CW_MOCK__.emit(event, payload)`.
 */
export interface ShimOptions {
  /** Name of the Playwright binding that answers non-event commands. */
  bindingName: string;
  /** Window label reported by `metadata`. */
  windowLabel: string;
  /** Commands answered in-page without a round trip to Node (cmd -> value). */
  localReplies?: Record<string, unknown>;
}

export function installTauriShim(opts: ShimOptions): void {
  const w = window as any;
  if (w.__CW_MOCK__) return; // init scripts run once per document; guard anyway

  const callbacks = new Map<number, (data: unknown) => unknown>();
  let nextCallbackId = 1;

  function transformCallback(cb?: (data: unknown) => unknown, once = false): number {
    const id = nextCallbackId++;
    callbacks.set(id, (data: unknown) => {
      if (once) callbacks.delete(id);
      return cb ? cb(data) : undefined;
    });
    return id;
  }

  function unregisterCallback(id: number) {
    callbacks.delete(id);
  }

  function runCallback(id: number, data: unknown) {
    const cb = callbacks.get(id);
    if (cb) cb(data);
  }

  // event name -> list of { eventId, handlerId }
  const listeners = new Map<string, Array<{ eventId: number; handler: number }>>();
  let nextEventId = 1;
  const emitted: Array<{ event: string; payload: unknown; delivered: number }> = [];
  const invokeLog: Array<{ cmd: string; args: unknown; t: number }> = [];

  function listen(event: string, handler: number): number {
    const eventId = nextEventId++;
    const list = listeners.get(event) ?? [];
    list.push({ eventId, handler });
    listeners.set(event, list);
    return eventId;
  }

  function unlisten(event: string, eventId: number) {
    const list = listeners.get(event);
    if (!list) return;
    const i = list.findIndex((l) => l.eventId === eventId);
    if (i >= 0) {
      unregisterCallback(list[i].handler);
      list.splice(i, 1);
    }
  }

  /** Delivers an event to every page listener, like Tauri's `Emitter::emit`. Returns the listener count. */
  function emit(event: string, payload: unknown): number {
    const list = (listeners.get(event) ?? []).slice();
    for (const l of list) {
      try {
        runCallback(l.handler, { event, id: l.eventId, payload });
      } catch (e) {
        // A throwing handler must not stop delivery to the others.
        setTimeout(() => {
          throw e;
        });
      }
    }
    emitted.push({ event, payload, delivered: list.length });
    if (emitted.length > 2000) emitted.splice(0, emitted.length - 2000);
    return list.length;
  }

  async function invoke(cmd: string, args: any = {}, _options?: unknown): Promise<unknown> {
    invokeLog.push({ cmd, args, t: Date.now() });
    if (invokeLog.length > 2000) invokeLog.splice(0, invokeLog.length - 2000);
    switch (cmd) {
      case "plugin:event|listen":
        return listen(args.event, args.handler);
      case "plugin:event|unlisten":
        unlisten(args.event, args.eventId);
        return null;
      case "plugin:event|emit":
      case "plugin:event|emit_to":
        emit(args.event, args.payload);
        return null;
    }
    if (opts.localReplies && Object.prototype.hasOwnProperty.call(opts.localReplies, cmd)) {
      return opts.localReplies[cmd];
    }
    const binding = w[opts.bindingName] as undefined | ((cmd: string, args: unknown) => Promise<any>);
    if (typeof binding !== "function") {
      throw `mock: binding ${opts.bindingName} is not installed (command ${cmd})`;
    }
    // Channels serialize through toJSON(); keep args JSON-clean for the binding.
    const res = await binding(cmd, JSON.parse(JSON.stringify(args ?? {})));
    if (res && res.ok) return res.value;
    // Tauri rejects with the command's error value (a string for Result<_, String>).
    throw res ? res.error : `mock: no reply for ${cmd}`;
  }

  w.__TAURI_INTERNALS__ = Object.assign(w.__TAURI_INTERNALS__ ?? {}, {
    invoke,
    transformCallback,
    unregisterCallback,
    runCallback,
    callbacks,
    convertFileSrc(filePath: string, protocol = "asset") {
      return `http://${protocol}.localhost/${encodeURIComponent(filePath)}`;
    },
    metadata: {
      currentWindow: { label: opts.windowLabel },
      currentWebview: { windowLabel: opts.windowLabel, label: opts.windowLabel },
    },
    plugins: {},
  });
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = Object.assign(w.__TAURI_EVENT_PLUGIN_INTERNALS__ ?? {}, {
    unregisterListener(event: string, eventId: number) {
      const list = listeners.get(event);
      const l = list?.find((x) => x.eventId === eventId);
      if (l) unregisterCallback(l.handler);
    },
  });

  w.__CW_MOCK__ = {
    emit,
    /** Emits a batch in order inside one task (used for history replays). */
    emitMany(batch: Array<[string, unknown]>) {
      let n = 0;
      for (const [event, payload] of batch) n += emit(event, payload);
      return n;
    },
    listenerCount(event: string) {
      return listeners.get(event)?.length ?? 0;
    },
    events() {
      return Array.from(listeners.keys()).filter((k) => (listeners.get(k)?.length ?? 0) > 0);
    },
    emitted,
    invokeLog,
  };
}
