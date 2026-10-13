#!/usr/bin/env python3
"""Record live CodeWhale runtime API fixtures for the desktop client.

Runs against a real `codewhale app-server --http` process and writes one JSON
file per exchange into the output directory:

  {"endpoint", "phase", "request": {method, path, auth, body},
   "status", "response": {content_type, body}, "recorded_at"}

SSE captures are written as {"endpoint", "phase", "request", "status",
"events": [{"event", "id", "data"}]} where `data` is the parsed JSON payload
(the wire carries it as a JSON string). `model.tools.snapshot` payloads are
shortened to their tool names to keep files small.

Phases:
  offline - no model backend reachable (turns must fail cleanly)
  mock    - mock_ollama.py serving on OLLAMA_BASE_URL (successful turns,
            tool calls, approvals, interrupt)

Usage:
  python3 -I record_live.py --base http://127.0.0.1:18765 --token testtoken \
      --out . --phase offline
"""

import argparse
import datetime as dt
import http.client
import json
import os
import socket
import threading
import time
import urllib.parse

LITE_TOOLS = ["read", "write", "edit", "bash", "todo_write"]

ARGS = None
STATE = {}


def now_iso():
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")


def conn(timeout=60):
    u = urllib.parse.urlparse(ARGS.base)
    return http.client.HTTPConnection(u.hostname, u.port, timeout=timeout)


WORKSPACE_PLACEHOLDER = "/work/demo-project"
HOME_PLACEHOLDER = "/home/demo"


def normalize(text):
    """Replaces machine-specific absolute paths with stable placeholders."""
    pairs = []
    if getattr(ARGS, "workspace", None):
        pairs.append((ARGS.workspace.rstrip("/"), WORKSPACE_PLACEHOLDER))
    if getattr(ARGS, "server_home", None):
        pairs.append((ARGS.server_home.rstrip("/"), HOME_PLACEHOLDER))
    for src, dst in sorted(pairs, key=lambda p: -len(p[0])):
        text = text.replace(src, dst)
    return text


def save(name, payload):
    path = os.path.join(ARGS.out, f"{name}.json")
    text = json.dumps(payload, ensure_ascii=False, indent=2)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(normalize(text))
        fh.write("\n")
    print(f"  -> {name}.json")


def request(name, method, path, body=None, auth=True, endpoint=None, note=None, timeout=60):
    headers = {"Accept": "application/json"}
    if isinstance(auth, str):
        headers["Authorization"] = f"Bearer {auth}"
    elif auth:
        headers["Authorization"] = f"Bearer {ARGS.token}"
    raw = None
    if body is not None:
        raw = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    c = conn(timeout)
    started = time.time()
    c.request(method, path, body=raw, headers=headers)
    resp = c.getresponse()
    data = resp.read()
    elapsed = round((time.time() - started) * 1000)
    ctype = resp.getheader("Content-Type") or ""
    try:
        parsed = json.loads(data) if data else None
    except json.JSONDecodeError:
        parsed = data.decode("utf-8", "replace")
    c.close()
    print(f"{method} {path} -> {resp.status} ({elapsed} ms)")
    fixture = {
        "endpoint": endpoint or f"{method} {path.split('?')[0]}",
        "phase": ARGS.phase,
        "request": {"method": method, "path": path, "auth": auth, "body": body},
        "status": resp.status,
        "response": {"content_type": ctype, "body": parsed},
        "elapsed_ms": elapsed,
        "recorded_at": now_iso(),
    }
    if note:
        fixture["note"] = note
    if name:
        save(name, fixture)
    return resp.status, parsed


def shorten(event, data):
    if event == "model.tools.snapshot" and isinstance(data, dict):
        payload = data.get("payload") or {}
        names = []
        tools = payload.get("tools") if isinstance(payload, dict) else None
        if isinstance(tools, list):
            for tool in tools:
                if isinstance(tool, dict):
                    names.append(tool.get("name") or (tool.get("function") or {}).get("name"))
        short = dict(data)
        short["payload"] = {
            "_shortened_by_recorder": True,
            "payload_keys": sorted(payload.keys()) if isinstance(payload, dict) else None,
            "tool_names": names,
        }
        return short
    return data


class SseCapture(threading.Thread):
    """Reads an SSE stream until `stop()` / deadline / `stream.end`."""

    def __init__(self, path, deadline_s=30.0, auth=True, stop_when=None):
        super().__init__(daemon=True)
        self.path = path
        self.deadline = time.time() + deadline_s
        self.auth = auth
        self.stop_when = stop_when
        self.events = []
        self.status = None
        self.headers = {}
        self.error_body = None
        self._stop = threading.Event()
        self.ready = threading.Event()

    def stop(self):
        self._stop.set()

    def _watchdog(self, c):
        while not self._stop.is_set() and time.time() < self.deadline:
            time.sleep(0.1)
        self._stop.set()
        try:
            if c.sock is not None:
                c.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass

    def run(self):
        headers = {"Accept": "text/event-stream"}
        if self.auth:
            headers["Authorization"] = f"Bearer {ARGS.token}"
        # Blocking reads; a watchdog shuts the socket down at the deadline or
        # on stop(). (A socket timeout would poison the http.client stream.)
        c = conn(timeout=None)
        try:
            c.request("GET", self.path, headers=headers)
            resp = c.getresponse()
        except Exception as exc:  # noqa: BLE001
            self.status = f"error: {exc}"
            self.ready.set()
            return
        self.status = resp.status
        self.headers = {k.lower(): v for k, v in resp.getheaders()}
        if resp.status != 200:
            raw = resp.read()
            try:
                self.error_body = json.loads(raw)
            except json.JSONDecodeError:
                self.error_body = raw.decode("utf-8", "replace")
            self.ready.set()
            return
        threading.Thread(target=self._watchdog, args=(c,), daemon=True).start()
        self.ready.set()
        frame = {"event": "message", "id": None, "data": []}
        buf = b""
        while not self._stop.is_set():
            try:
                chunk = resp.read1(65536)
            except Exception:  # noqa: BLE001
                break
            if not chunk:
                break
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                text = line.decode("utf-8", "replace").rstrip("\r")
                if text == "":
                    if frame["data"]:
                        raw = "\n".join(frame["data"])
                        try:
                            data = json.loads(raw)
                        except json.JSONDecodeError:
                            data = raw
                        data = shorten(frame["event"], data)
                        self.events.append({"event": frame["event"], "id": frame["id"], "data": data})
                        if frame["event"] == "stream.end":
                            self._stop.set()
                        if self.stop_when and self.stop_when(frame["event"], data):
                            # keep reading briefly so trailing events land too
                            self.deadline = min(self.deadline, time.time() + 1.5)
                    frame = {"event": "message", "id": None, "data": []}
                elif text.startswith(":"):
                    self.events.append({"event": ":comment", "id": None, "data": text[1:].strip()})
                elif text.startswith("event:"):
                    frame["event"] = text[6:].strip()
                elif text.startswith("data:"):
                    frame["data"].append(text[6:] if text.startswith("data: ") else text[5:])
                elif text.startswith("id:"):
                    frame["id"] = text[3:].strip()
        self._stop.set()
        try:
            c.close()
        except Exception:  # noqa: BLE001
            pass

    def save(self, name, note=None):
        fixture = {
            "endpoint": "GET /v1/threads/{id}/events",
            "phase": ARGS.phase,
            "request": {"method": "GET", "path": self.path, "auth": self.auth},
            "status": self.status,
            "response_headers": {k: v for k, v in self.headers.items()
                                 if k in ("content-type", "x-codewhale-stream-end", "cache-control")},
            "events": self.events,
            "event_names": [e["event"] for e in self.events],
            "recorded_at": now_iso(),
        }
        if self.error_body is not None:
            fixture["response"] = {"body": self.error_body}
        if note:
            fixture["note"] = note
        save(name, fixture)


def events_path(tid, since=0):
    return f"/v1/threads/{tid}/events?replay_limit=4096&progress=true&since_seq={since}"


TERMINAL_TURN_EVENTS = ("turn.completed", "turn.failed", "turn.interrupted", "turn.canceled")


def turn_id_of(resp):
    if isinstance(resp, dict):
        return (resp.get("turn") or {}).get("id") or resp.get("turn_id")
    return None


def run_turn(tid, prompt, name, extra=None, deadline=45.0, on_event=None):
    """Subscribes, posts a turn, waits for a terminal turn event, saves both."""
    target = {"turn": None}

    def stop_when(event, data):
        return (event in TERMINAL_TURN_EVENTS and isinstance(data, dict)
                and target["turn"] is not None and data.get("turn_id") == target["turn"])

    cap = SseCapture(events_path(tid, STATE.get(f"seq:{tid}", 0)), deadline_s=deadline,
                     stop_when=stop_when)
    cap.start()
    cap.ready.wait(5)
    time.sleep(0.3)
    body = {"prompt": prompt,
            "operation_key": f"desk-{int(time.time() * 1000)}-rec"}
    body["allowed_tools"] = LITE_TOOLS
    if extra:
        body.update(extra)
    status, resp = request(f"{name}__post_turn", "POST", f"/v1/threads/{tid}/turns", body,
                           endpoint="POST /v1/threads/{id}/turns")
    target["turn"] = turn_id_of(resp)
    for e in list(cap.events):
        if stop_when(e["event"], e["data"]):
            cap.deadline = min(cap.deadline, time.time() + 1.5)
    if on_event:
        on_event(cap, resp)
    cap.join(deadline + 2)
    cap.stop()
    cap.save(f"{name}__sse")
    seqs = [int(e["id"]) for e in cap.events if e.get("id") and str(e["id"]).isdigit()]
    if seqs:
        STATE[f"seq:{tid}"] = max(seqs)
    return status, resp, cap


# ---------------------------------------------------------------------------
def phase_offline():
    p = "offline"
    request(f"{p}_01_health", "GET", "/health", auth=False)
    request(f"{p}_02_runtime_info_noauth", "GET", "/v1/runtime/info", auth=False,
            endpoint="GET /v1/runtime/info (no auth)")
    request(f"{p}_03_runtime_info", "GET", "/v1/runtime/info")
    request(f"{p}_04_threads_noauth", "GET", "/v1/threads/summary?limit=300", auth=False,
            endpoint="GET /v1/threads/summary (no auth)")
    request(f"{p}_05_threads_wrongauth", "GET", "/v1/threads/summary?limit=300", auth="wrong-token",
            endpoint="GET /v1/threads/summary (wrong token)")

    st, t1 = request(f"{p}_10_create_thread_minimal", "POST", "/v1/threads",
                     {"model_provider": "ollama", "model": "qwen3:4b"})
    tid = t1["id"] if isinstance(t1, dict) and "id" in t1 else None
    ws = os.getcwd()
    st, t2 = request(f"{p}_11_create_thread_client_body", "POST", "/v1/threads",
                     {"model_provider": "ollama", "model": "qwen3:4b",
                      "workspace": ws, "allowed_tools": LITE_TOOLS},
                     note="Exact body shape the desktop client sends from newThread() (lite tool profile).")
    tid2 = t2.get("id") if isinstance(t2, dict) else None
    request(f"{p}_12_create_thread_no_model", "POST", "/v1/threads", {"model_provider": "ollama"},
            note="Client omits `model` when no default model is configured.")
    request(f"{p}_13_create_thread_bad_tool", "POST", "/v1/threads",
            {"model_provider": "ollama", "model": "qwen3:4b", "allowed_tools": ["definitely_not_a_tool"]},
            note="Probe: how unknown allowed_tools names are treated.")

    request(f"{p}_20_threads_summary", "GET", "/v1/threads/summary?limit=300")
    request(f"{p}_21_threads_summary_search", "GET",
            "/v1/threads/summary?limit=100&search=" + urllib.parse.quote("qwen"))
    request(f"{p}_22_sessions", "GET", "/v1/sessions?limit=50")
    if not tid:
        print("no thread id; abort")
        return
    request(f"{p}_23_thread_get", "GET", f"/v1/threads/{tid}", endpoint="GET /v1/threads/{id}")
    request(f"{p}_24_thread_context", "GET", f"/v1/threads/{tid}/context",
            endpoint="GET /v1/threads/{id}/context")
    request(f"{p}_25_thread_get_unknown", "GET", "/v1/threads/thr_doesnotexist",
            endpoint="GET /v1/threads/{id} (unknown id)")

    request(f"{p}_30_patch_title", "PATCH", f"/v1/threads/{tid}", {"title": "Тестовая сессия"},
            endpoint="PATCH /v1/threads/{id} (title)")
    request(f"{p}_31_patch_model", "PATCH", f"/v1/threads/{tid}",
            {"model": "qwen3:0.6b", "model_provider": "ollama"},
            endpoint="PATCH /v1/threads/{id} (model + model_provider)")
    request(f"{p}_32_fork", "POST", f"/v1/threads/{tid}/fork", {}, endpoint="POST /v1/threads/{id}/fork")
    request(f"{p}_33_compact_empty", "POST", f"/v1/threads/{tid}/compact", {},
            endpoint="POST /v1/threads/{id}/compact")
    if tid2:
        request(f"{p}_34_patch_archived", "PATCH", f"/v1/threads/{tid2}", {"archived": True},
                endpoint="PATCH /v1/threads/{id} (archived)")

    # Turn without any model backend.
    status, resp, cap = run_turn(tid, "Привет! Ответь одним словом.", f"{p}_40_turn_no_backend",
                                 deadline=40.0)
    turn_id = None
    if isinstance(resp, dict):
        turn_id = (resp.get("turn") or {}).get("id") or resp.get("turn_id") or resp.get("id")
    if turn_id:
        request(f"{p}_41_interrupt_finished_turn", "POST",
                f"/v1/threads/{tid}/turns/{turn_id}/interrupt", {},
                endpoint="POST /v1/threads/{id}/turns/{turn_id}/interrupt (already finished)")
    request(f"{p}_42_interrupt_unknown_turn", "POST",
            f"/v1/threads/{tid}/turns/turn_doesnotexist/interrupt", {},
            endpoint="POST /v1/threads/{id}/turns/{turn_id}/interrupt (unknown turn)")
    request(f"{p}_43_thread_get_after_failed_turn", "GET", f"/v1/threads/{tid}",
            endpoint="GET /v1/threads/{id}")

    request(f"{p}_50_approvals_list", "GET", "/v1/approvals")
    request(f"{p}_51_approval_decide_unknown", "POST", "/v1/approvals/appr_doesnotexist",
            {"decision": "allow", "remember": False},
            endpoint="POST /v1/approvals/{approval_id} (unknown id)")
    request(f"{p}_52_approval_decide_bad_value", "POST", "/v1/approvals/appr_doesnotexist",
            {"decision": "approve", "remember": False},
            endpoint="POST /v1/approvals/{approval_id} (invalid decision)")

    request(f"{p}_60_files_search_dot", "GET", "/v1/workspace/files/search?query=.&limit=60",
            endpoint="GET /v1/workspace/files/search")
    request(f"{p}_61_files_search_hel", "GET", "/v1/workspace/files/search?query=hel&limit=60",
            endpoint="GET /v1/workspace/files/search")
    request(f"{p}_62_usage_day", "GET", "/v1/usage?group_by=day")

    st, sessions = request(None, "GET", "/v1/sessions?limit=50")
    sid = None
    if isinstance(sessions, dict):
        for s in sessions.get("sessions") or []:
            sid = s.get("id") or s.get("session_id")
            if sid:
                break
    if sid:
        request(f"{p}_70_session_resume_thread", "POST", f"/v1/sessions/{sid}/resume-thread", {},
                endpoint="POST /v1/sessions/{id}/resume-thread")
    request(f"{p}_71_session_resume_unknown", "POST", "/v1/sessions/sess_doesnotexist/resume-thread", {},
            endpoint="POST /v1/sessions/{id}/resume-thread (unknown id)")

    # Full replay of the thread (what the client gets when it (re)opens a thread).
    cap = SseCapture(events_path(tid, 0), deadline_s=4.0)
    cap.start()
    cap.join(6)
    cap.save(f"{p}_80_sse_replay_from_zero",
             note="Replay of the whole thread from since_seq=0, then ~4 s of live tail.")
    cap = SseCapture(events_path("thr_doesnotexist", 0), deadline_s=2.0)
    cap.start()
    cap.join(4)
    cap.save(f"{p}_81_sse_unknown_thread")
    cap = SseCapture(events_path(tid, 0), deadline_s=2.0, auth=False)
    cap.start()
    cap.join(4)
    cap.save(f"{p}_82_sse_noauth")
    cap = SseCapture(f"/v1/threads/{tid}/events?replay_limit=5000&progress=true&since_seq=0", deadline_s=2.0)
    cap.start()
    cap.join(4)
    cap.save(f"{p}_83_sse_replay_limit_too_big")


def phase_mock():
    p = "mock"
    request(f"{p}_01_runtime_info", "GET", "/v1/runtime/info")
    ws = os.getcwd()
    st, t = request(f"{p}_02_create_thread", "POST", "/v1/threads",
                    {"model_provider": "ollama", "model": "qwen3:4b", "workspace": ws,
                     "allowed_tools": LITE_TOOLS})
    tid = t["id"]
    request(f"{p}_03_create_thread_no_model", "POST", "/v1/threads", {"model_provider": "ollama"},
            note="Same body as offline_12, but with an Ollama catalog reachable.")

    run_turn(tid, "Привет! Ответь одним словом.", f"{p}_10_turn_text")
    run_turn(tid, "Прочитай файл hello.txt [tool:read]", f"{p}_11_turn_tool_read")

    # write -> approval required -> allow
    def approve(decision):
        def handler(cap, _resp):
            deadline = time.time() + 20
            while time.time() < deadline:
                for e in list(cap.events):
                    if e["event"] == "approval.required":
                        d = e["data"] if isinstance(e["data"], dict) else {}
                        payload = d.get("payload") or {}
                        aid = payload.get("approval_id") or payload.get("id")
                        if aid:
                            time.sleep(0.5)
                            request(f"{p}_13b_list_approvals_while_pending_{decision}", "GET", "/v1/approvals")
                            request(f"{p}_13c_approval_{decision}", "POST", f"/v1/approvals/{aid}",
                                    {"decision": decision, "remember": False},
                                    endpoint="POST /v1/approvals/{approval_id}")
                            return
                time.sleep(0.2)
            print(f"  (no approval.required seen for {decision})")
        return handler

    run_turn(tid, "Создай файл created_by_agent.txt [tool:write]", f"{p}_12_turn_tool_write_allow",
             on_event=approve("allow"))
    run_turn(tid, "Выполни команду [tool:bash]", f"{p}_13_turn_tool_bash_deny",
             on_event=approve("deny"))

    # interrupt mid-stream
    def interrupt(cap, resp):
        turn_id = None
        if isinstance(resp, dict):
            turn_id = (resp.get("turn") or {}).get("id") or resp.get("turn_id")
        time.sleep(2.5)
        if turn_id:
            request(f"{p}_14b_interrupt_running_turn", "POST",
                    f"/v1/threads/{tid}/turns/{turn_id}/interrupt", {},
                    endpoint="POST /v1/threads/{id}/turns/{turn_id}/interrupt")
    run_turn(tid, "Говори медленно [slow]", f"{p}_14_turn_interrupt", on_event=interrupt)

    request(f"{p}_20_thread_get_after_turns", "GET", f"/v1/threads/{tid}", endpoint="GET /v1/threads/{id}")
    request(f"{p}_21_context_after_turns", "GET", f"/v1/threads/{tid}/context",
            endpoint="GET /v1/threads/{id}/context")
    request(f"{p}_22_threads_summary", "GET", "/v1/threads/summary?limit=300")
    request(f"{p}_23_usage_day", "GET", "/v1/usage?group_by=day")
    request(f"{p}_24_fork_with_history", "POST", f"/v1/threads/{tid}/fork", {},
            endpoint="POST /v1/threads/{id}/fork")

    cap = SseCapture(events_path(tid, STATE.get(f"seq:{tid}", 0)), deadline_s=25.0,
                     stop_when=lambda ev, d: ev in ("thread.compacted", "compaction.completed",
                                                    "turn.completed", "turn.failed"))
    cap.start()
    cap.ready.wait(5)
    request(f"{p}_25_compact_with_history", "POST", f"/v1/threads/{tid}/compact", {},
            endpoint="POST /v1/threads/{id}/compact")
    cap.join(27)
    cap.save(f"{p}_26_compact__sse")

    request(f"{p}_27_files_search", "GET", "/v1/workspace/files/search?query=hel&limit=60",
            endpoint="GET /v1/workspace/files/search")
    request(f"{p}_28_sessions", "GET", "/v1/sessions?limit=50")

    cap = SseCapture(events_path(tid, 0), deadline_s=4.0)
    cap.start()
    cap.join(6)
    cap.save(f"{p}_30_sse_full_replay",
             note="Everything a freshly opened desktop window receives for this thread.")


def phase_extra():
    """Shell approval -> allow, plus the thread snapshot while it is pending."""
    p = "mock"
    ws = os.getcwd()
    st, t = request(None, "POST", "/v1/threads",
                    {"model_provider": "ollama", "model": "qwen3:4b", "workspace": ws,
                     "allowed_tools": LITE_TOOLS})
    tid = t["id"]

    def handler(cap, _resp):
        deadline = time.time() + 20
        while time.time() < deadline:
            for e in list(cap.events):
                if e["event"] == "approval.required":
                    payload = (e["data"] or {}).get("payload") or {}
                    aid = payload.get("approval_id") or payload.get("id")
                    time.sleep(0.5)
                    request(f"{p}_15b_thread_get_with_pending_approval", "GET", f"/v1/threads/{tid}",
                            endpoint="GET /v1/threads/{id} (approval pending)")
                    request(f"{p}_15c_approval_allow", "POST", f"/v1/approvals/{aid}",
                            {"decision": "allow", "remember": False},
                            endpoint="POST /v1/approvals/{approval_id}")
                    return
            time.sleep(0.2)
        print("  (no approval.required seen)")

    run_turn(tid, "Выполни команду [tool:bash]", f"{p}_15_turn_tool_bash_allow", on_event=handler)


def phase_custom_url():
    """Server started with OLLAMA_BASE_URL=http://127.0.0.1:11435/v1 (desktop's custom Ollama URL path)."""
    p = "mock"
    ws = os.getcwd()
    st, t = request(f"{p}_90_custom_url_create_thread", "POST", "/v1/threads",
                    {"model_provider": "ollama", "model": "qwen3:4b", "workspace": ws,
                     "allowed_tools": LITE_TOOLS},
                    note="Server env OLLAMA_BASE_URL=http://127.0.0.1:11435/v1; mock Ollama on :11435 only.")
    tid = t["id"]
    run_turn(tid, "Привет! Ответь одним словом.", f"{p}_91_custom_url_turn_text")


def main():
    global ARGS
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="http://127.0.0.1:18765")
    parser.add_argument("--token", default="testtoken")
    parser.add_argument("--out", default=".")
    parser.add_argument("--workspace", default=os.getcwd(),
                        help="server cwd; replaced by %s in saved fixtures" % WORKSPACE_PLACEHOLDER)
    parser.add_argument("--server-home", default=None,
                        help="server HOME; replaced by %s in saved fixtures" % HOME_PLACEHOLDER)
    parser.add_argument("--phase", choices=["offline", "mock", "extra", "custom_url"], required=True)
    ARGS = parser.parse_args()
    os.makedirs(ARGS.out, exist_ok=True)
    {"offline": phase_offline, "mock": phase_mock, "extra": phase_extra,
     "custom_url": phase_custom_url}[ARGS.phase]()


if __name__ == "__main__":
    main()
