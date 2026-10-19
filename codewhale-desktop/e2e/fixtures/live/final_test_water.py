#!/usr/bin/env python3
"""Final knowledge-drive test: a real CodeWhale app-server with the ai-drive MCP server,
driven by the scripted mock Ollama, writes a water shader into a Godot project, checks it
with `godot_check`, and then Godot renders the scene to a PNG.

The only fake part is the model text (mock_ollama.py, scenario "[shader]"); CodeWhale, its
tool execution, the MCP transport, our MCP server and Godot are real.

Usage:
  python3 final_test_water.py --codewhale <codewhale-app-server .exe> --desktop <codewhale-desktop binary>
      --godot <godot binary> --project <Godot/WaterDemo folder> --shader <water.gdshader>
      --check-project <src-tauri/resources/godot-check> --out <folder for the report and the PNG>
      [--render] [--keep-tmp] [--stream-open-timeout 300] [--deny-tools NAMES] [--base-config PATH]
      [--drive PATH|temp] [--kiwix-url URL] [--always-load | --no-always-load]

Live mode (a real model instead of the mock, Windows or Linux; --shader is not needed):
  ... --ollama-url http://127.0.0.1:11434 --model qwen3-4b-32k --timeout 1500 [--render]
The bundled knowledge-drive skills are copied into the temporary CodeWhale home so the agent
gets the same instructions as in the desktop app. In live mode <home>/config.toml is built from
the user's own config.toml (read-only, --base-config), so CodeWhale knows the model's context
window like in the desktop app; without it every Ollama model gets 8192 tokens and the agent
compacts its context every few steps. The `[stream] open_timeout_secs` budget the desktop app
sets is written exactly once (Ollama emits the first bytes of a tool call only when the whole
call is generated, so CodeWhale's 45 s default aborts long writes). Approvals are answered
"allow" except for --deny-tools (live default: Web, which the desktop app does not auto-approve
either). On Windows --render runs Godot directly (no xvfb). The "agent reported the check" and
"ledger_record" steps are informational in live mode.

The ai-drive MCP server reads the knowledge drive given by --drive (live default: F:\\AI_Drive
when it exists; mock default and `--drive temp`: an empty temporary drive). Its ledger and
bookmarks (AI_DRIVE_STATE_DIR) always go to the temporary folder, so a run never adds entries to
the user's ledger. --kiwix-url (default http://127.0.0.1:8091) is passed as KIWIX_URL.
--always-load (default on) writes `[tools] always_load` with godot_check, ledger_record and
math_eval into the temporary config.toml (merged with the base config's list): CodeWhale connects
MCP servers lazily, and without it tool_search cannot find the drive tools before the server is
connected. The step "always_load tools at step 0" checks the first model.tools.snapshot.

--sampling SPEC (meant for live mode) runs ollama_proxy.py (in this process) between CodeWhale and
Ollama: CodeWhale 0.10.1 sends no temperature/top_p/penalties, so Ollama's /v1 layer uses
temperature 1.0 and top_p 1.0. The proxy adds the fields the request lacks (key=value), caps
(key<=value) or replaces (key:=value) them, e.g.
'temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens=4096' ('default' is that string),
and turns "think": false into reasoning_effort "none" (--no-think-to-effort keeps it). Both
OLLAMA_BASE_URL and [providers.ollama] base_url point at the proxy; every request is logged to
ollama_proxy.jsonl and report.json gets settings.sampling and summary.sampling_proxy. In mock mode
it tests the proxy chain (the mock's ollama_requests.jsonl shows what arrived).
--context-window N writes [providers.ollama.model_context_windows] "<model>" = N (and
"<model>:latest" for an untagged name) into the temporary config.toml, for a model the base
config does not know yet.

Exit code 0 when the agent wrote the shader, called godot_check through MCP, the check passed
and (with --render) the PNG was saved; the folder gets report.json (with a "summary" of model
requests, approvals, compactions and the tools called in order), events.jsonl, codewhale.log,
codewhale-config.toml, ollama_requests.jsonl (mock mode), ollama_proxy.jsonl (--sampling) and
water.png. Stale copies of those files are removed at start, and report.json is written on every
exit path, including a crash of the script itself.
"""

import argparse
import datetime
import http.client
import importlib.util
import json
import math
import os
import re
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from pathlib import Path
from urllib.parse import urlsplit

try:
    import tomllib  # Python 3.11+; needed only to merge a base config.toml
except ImportError:  # pragma: no cover
    tomllib = None

TOKEN = "final-test-token"
OUTPUT_FILES = ("report.json", "water.png", "events.jsonl", "codewhale.log", "codewhale-config.toml",
                "ollama_requests.jsonl", "ollama_proxy.jsonl")
PROXY_LOG = "ollama_proxy.jsonl"
SHIM_SUFFIXES = (".cmd", ".bat", ".ps1")
TOOL_ITEM_KINDS = ("tool_call", "command_execution", "file_change")
ITEM_END_EVENTS = {"item.completed": "completed", "item.failed": "failed",
                   "item.canceled": "canceled", "item.interrupted": "interrupted"}
TURN_END_EVENTS = ("turn.completed", "turn.failed", "turn.interrupted", "turn.canceled")
GODOT_CHECK_TOOL = "mcp_ai-drive_godot_check"
LEDGER_RECORD_TOOL = "mcp_ai-drive_ledger_record"
# Loaded from step 0 by `[tools] always_load` (single hyphen: the registry name, not the one the
# model is shown). Everything else on the drive is found with tool_search "ai-drive <tool>".
ALWAYS_LOAD_TOOLS = (GODOT_CHECK_TOOL, LEDGER_RECORD_TOOL, "mcp_ai-drive_math_eval")
DEFAULT_LIVE_DRIVE = r"F:\AI_Drive"
DEFAULT_KIWIX_URL = "http://127.0.0.1:8091"  # 8090 is taken by Docker on the user's machine
SSE_RECONNECTS = 5  # consecutive failed reconnects before the reader gives up


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_http(host, port, path, timeout=60, scheme="http"):
    conn_cls = http.client.HTTPSConnection if scheme == "https" else http.client.HTTPConnection
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            c = conn_cls(host, port, timeout=2)
            c.request("GET", path)
            if c.getresponse().status < 500:
                return True
        except OSError:
            pass
        time.sleep(0.2)
    return False


def resolve_binary(ap, flag, value, refuse_shims=False):
    """Absolute path of a CLI binary given as a path or a bare name on PATH."""
    found = shutil.which(value) or value
    path = Path(found).resolve()
    if not path.is_file():
        ap.error(f"{flag}: {value!r} is not a file (looked up {path})")
    if refuse_shims and path.suffix.lower() in SHIM_SUFFIXES:
        ap.error(f"{flag}: {path} is a {path.suffix} shim; pass the .exe itself (it is spawned and killed by PID)")
    return path


def parse_ollama_url(raw):
    """(scheme, host, port, prefix) of an Ollama server URL; `prefix` is the path before /v1."""
    url = raw if "://" in raw else "http://" + raw
    u = urlsplit(url)
    scheme = u.scheme or "http"
    host = u.hostname or "127.0.0.1"
    port = u.port or (443 if scheme == "https" else 11434)
    prefix = u.path.rstrip("/")
    if prefix.endswith("/v1"):
        prefix = prefix[:-3]
    return scheme, host, port, prefix


def kill_tree(proc):
    """Stops a child and everything it spawned (the MCP server and Godot under the app-server)."""
    if proc.poll() is not None:
        return
    if sys.platform == "win32":
        subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)
    else:
        proc.terminate()
    try:
        proc.wait(10)
    except Exception:
        proc.kill()


def remove_tree(path, attempts=6, delay=0.5):
    """rmtree for Windows: CodeWhale's snapshot repo under <home>/.codewhale/snapshots holds read-only
    git objects, which plain rmtree cannot delete, and a killed process releases its handles a moment
    after it dies, so failures are retried briefly."""
    def unlock(func, p, _exc):
        try:
            os.chmod(p, stat.S_IWRITE)
            func(p)
        except OSError:
            pass

    handler = {"onexc": unlock} if sys.version_info >= (3, 12) else {"onerror": unlock}
    for i in range(attempts):
        try:
            shutil.rmtree(path, **handler)
        except OSError:
            pass
        if not path.exists():
            return True
        if i < attempts - 1:
            time.sleep(delay)
    print(f"[cleanup] could not remove {path}", file=sys.stderr, flush=True)
    return False


def tail_of(path, n=400):
    try:
        return path.read_bytes()[-n:].decode("utf-8", "replace")
    except OSError:
        return ""


def load_proxy_module():
    """ollama_proxy.py from this folder (python -I does not put the script's folder on sys.path)."""
    path = Path(__file__).resolve().parent / "ollama_proxy.py"
    spec = importlib.util.spec_from_file_location("ollama_proxy", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def window_keys(model):
    """The model_context_windows keys for `model`: the name itself, plus "<name>:latest" for an
    untagged name (the user's config lists both spellings)."""
    return [model] if ":" in model else [model, model + ":latest"]


# --- tool names -------------------------------------------------------------------------------

def canonical_tool(name):
    """Lower-cased tool name with CodeWhale's two MCP spellings folded into one: the model is shown
    `mcp_ai--drive_*` (the hyphen in the server name doubled), approvals and item metadata say
    `mcp_ai-drive_*`. Returns "" for a missing name."""
    if not isinstance(name, str):
        return ""
    name = name.strip().lower()
    if name.startswith("mcp_"):
        while "--" in name:
            name = name.replace("--", "-")
    return name


def is_drive_tool(name):
    return canonical_tool(name).startswith("mcp_ai-drive_")


def item_tool_name(payload):
    """The executed tool's name in an item.* payload: `item.metadata.tool_name` (started, completed
    and failed items), else `tool.name` (item.started only). Never the free-text summary: a Web
    search result can quote "godot_check" without the tool ever running."""
    item = payload.get("item") or {}
    meta = item.get("metadata") or {}
    name = meta.get("tool_name")
    if not isinstance(name, str) or not name:
        tool = payload.get("tool")
        name = tool.get("name") if isinstance(tool, dict) else None
    return name if isinstance(name, str) and name else None


def parse_tool_list(raw):
    """Canonical names from a comma-separated --deny-tools value; "none" or "" means no tools."""
    if raw is None or raw.strip().lower() in ("", "none", "-"):
        return set()
    return {canonical_tool(part) for part in raw.split(",") if part.strip()}


def snapshot_tool_names(payload):
    """(step, names) of a model.tools.snapshot payload. 0.10.1 nests the request's tools in
    payload.snapshot.tools[] as {"name": {"value": ...}}; older builds listed plain names in
    payload.tools / payload.names. step is None when the payload does not say."""
    snap = payload.get("snapshot") if isinstance(payload.get("snapshot"), dict) else payload
    tools = snap.get("tools") or snap.get("names") or []
    names = []
    for t in tools if isinstance(tools, list) else []:
        name = t.get("name") if isinstance(t, dict) else t
        if isinstance(name, dict):
            name = name.get("value")
        if isinstance(name, str) and name:
            names.append(name)
    step = snap.get("step")
    return (step if isinstance(step, int) else None), names


# --- config.toml ------------------------------------------------------------------------------

BARE_KEY = re.compile(r"[A-Za-z0-9_-]+")


def _toml_str(s):
    # A JSON string without ensure_ascii is a valid TOML basic string, except that TOML also
    # wants DEL escaped.
    return json.dumps(s, ensure_ascii=False).replace("\x7f", "\\u007f")


def _toml_key(key):
    return key if BARE_KEY.fullmatch(key) else _toml_str(key)


def _toml_value(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        if math.isnan(v):
            return "nan"
        if math.isinf(v):
            return "inf" if v > 0 else "-inf"
        return repr(v)
    if isinstance(v, str):
        return _toml_str(v)
    if isinstance(v, (datetime.datetime, datetime.date, datetime.time)):
        return v.isoformat()
    if isinstance(v, list):
        return "[" + ", ".join(_toml_value(x) for x in v) + "]"
    if isinstance(v, dict):
        return "{" + ", ".join(f"{_toml_key(k)} = {_toml_value(x)}" for k, x in v.items()) + "}"
    raise TypeError(f"cannot write {type(v).__name__} to TOML")


def dump_toml(data):
    """TOML text for a dict as tomllib returns it: tables become [headers], arrays (also arrays
    of tables) are written inline. Every table and key appears exactly once."""
    lines = []

    def table(node, path):
        scalars = [(k, v) for k, v in node.items() if not isinstance(v, dict)]
        children = [(k, v) for k, v in node.items() if isinstance(v, dict)]
        if path and (scalars or not children):
            if lines:
                lines.append("")
            lines.append("[" + ".".join(_toml_key(p) for p in path) + "]")
        for k, v in scalars:
            lines.append(f"{_toml_key(k)} = {_toml_value(v)}")
        for k, v in children:
            table(v, path + [k])

    table(data, [])
    return "\n".join(lines) + "\n" if lines else ""


def merge_always_load(data, tools):
    """Adds `tools` to data["tools"]["always_load"], keeping the base's entries and their order
    and skipping names that are already there. Returns the resulting list."""
    table = data.get("tools")
    if not isinstance(table, dict):
        table = data["tools"] = {}
    current = table.get("always_load")
    if isinstance(current, str):
        current = [current]
    merged = [t for t in current if isinstance(t, str)] if isinstance(current, list) else []
    for tool in tools:
        if tool not in merged:
            merged.append(tool)
    table["always_load"] = merged
    return merged


def merge_codewhale_config(base_text, stream_open_timeout, ollama_base_url=None, source=None, always_load=None,
                           model_windows=None):
    """config.toml text for the temporary CodeWhale home.

    `base_text` is the user's config.toml (None or empty: start from nothing), so the provider,
    the context windows and the features match the desktop app. `[stream] open_timeout_secs` is
    set to `stream_open_timeout`, when given `[providers.ollama] base_url` to the server under
    test, `always_load` (a list of tool names) is merged into `[tools] always_load`, and
    `model_windows` ({model: tokens}) is set in `[providers.ollama.model_context_windows]`
    (replacing the base's value for those models). The base is parsed and written back (its
    comments are dropped), so no table and no key can appear twice: TOML duplicates make
    CodeWhale refuse the whole file.
    Raises ValueError for a base that is not valid TOML."""
    data = {}
    if base_text is not None and base_text.strip():
        if tomllib is None:
            raise RuntimeError("tomllib is missing (Python < 3.11): cannot merge a base config.toml")
        try:
            data = tomllib.loads(base_text)
        except tomllib.TOMLDecodeError as e:
            raise ValueError(f"base config is not valid TOML: {e}") from e
    stream = data.get("stream")
    if not isinstance(stream, dict):
        stream = data["stream"] = {}
    stream["open_timeout_secs"] = int(stream_open_timeout)
    if ollama_base_url or model_windows:
        providers = data.get("providers")
        if not isinstance(providers, dict):
            providers = data["providers"] = {}
        ollama = providers.get("ollama")
        if not isinstance(ollama, dict):
            ollama = providers["ollama"] = {}
        if ollama_base_url:
            ollama["base_url"] = ollama_base_url
        if model_windows:
            windows = ollama.get("model_context_windows")
            if not isinstance(windows, dict):
                windows = ollama["model_context_windows"] = {}
            for model, tokens in model_windows.items():
                windows[model] = int(tokens)
    if always_load:
        merge_always_load(data, always_load)
    header = ["# Written by final_test_water.py" + (f" from {source} (its comments are dropped)." if source else ".")]
    header.append("# [stream] open_timeout_secs comes from --stream-open-timeout (the budget the desktop app sets).")
    if ollama_base_url:
        header.append("# [providers.ollama] base_url points at the Ollama server under test (or the --sampling proxy).")
    if model_windows:
        header.append("# [providers.ollama.model_context_windows] entries for "
                      + ", ".join(sorted(model_windows)) + " come from --context-window.")
    if always_load:
        header.append("# [tools] always_load (--always-load) shows the ai-drive tools to the model from step 0.")
    text = "\n".join(header) + "\n\n" + dump_toml(data)
    if tomllib is not None and dump_toml(tomllib.loads(text)) != dump_toml(data):
        raise RuntimeError("config.toml did not survive a TOML round trip")
    return text


def default_base_config():
    """The config.toml the desktop app edits (runtime.rs::codewhale_config_path):
    CODEWHALE_CONFIG_PATH, else $CODEWHALE_HOME/config.toml, else ~/.codewhale/config.toml."""
    explicit = os.environ.get("CODEWHALE_CONFIG_PATH", "").strip()
    if explicit:
        return Path(explicit)
    home = os.environ.get("CODEWHALE_HOME", "").strip()
    if home:
        return Path(home) / "config.toml"
    user = os.environ.get("USERPROFILE") or os.environ.get("HOME") or str(Path.home())
    return Path(user) / ".codewhale" / "config.toml"


def configured_window(config, model):
    """The context window the config gives `model` (per-model table first), or None."""
    ollama = ((config or {}).get("providers") or {}).get("ollama") or {}
    windows = ollama.get("model_context_windows") or {}
    for key in (model, model.split(":")[0] if ":" in model else model + ":latest"):
        if isinstance(windows.get(key), int):
            return windows[key]
    window = ollama.get("context_window")
    return window if isinstance(window, int) else None


# --- runtime API ------------------------------------------------------------------------------

class Api:
    def __init__(self, port):
        self.port = port

    def call(self, method, path, body=None, timeout=60):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
        try:
            headers = {"Authorization": f"Bearer {TOKEN}"}
            raw = None
            if body is not None:
                raw = json.dumps(body).encode()
                headers["Content-Type"] = "application/json"
            c.request(method, path, body=raw, headers=headers)
            r = c.getresponse()
            data = r.read()
            try:
                return r.status, json.loads(data or b"null")
            except json.JSONDecodeError:
                return r.status, data.decode("utf-8", "replace")
        finally:
            c.close()

    def events(self, thread_id, on_event, stop, since=0, timeout=None):
        """Streams SSE frames until `stop` is set; calls on_event(name, data).

        Returns "stopped" when the caller set `stop`, "ended" when the server closed the stream;
        connection errors propagate to the caller."""
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
        try:
            c.request("GET", f"/v1/threads/{thread_id}/events?replay_limit=4096&progress=true&since_seq={since}",
                      headers={"Authorization": f"Bearer {TOKEN}", "Accept": "text/event-stream"})
            r = c.getresponse()
            if r.status != 200:
                raise RuntimeError(f"events stream answered HTTP {r.status}: {r.read(300)!r}")
            name, data = None, []
            while not stop.is_set():
                line = r.readline()  # HTTPResponse.readline also undoes chunked transfer encoding
                if not line:
                    return "ended"
                line = line.decode("utf-8", "replace").rstrip("\r\n")
                if line.startswith("event:"):
                    name = line[6:].strip()
                elif line.startswith("data:"):
                    data.append(line[5:].lstrip())
                elif line == "":
                    if name and data:
                        try:
                            parsed = json.loads("\n".join(data))
                        except json.JSONDecodeError:
                            parsed = None
                        if parsed is not None:
                            on_event(name, parsed)
                    name, data = None, []
            return "stopped"
        finally:
            c.close()


def new_seen():
    return {
        "t0": time.time(), "last_seq": 0, "replayed": 0,
        "done": None, "turn_status": None, "turn_error": None, "turn_duration_ms": None, "turn_diagnostics": None,
        "final_text": "", "mcp_tools": set(), "step0_tools": None,
        "tool_calls": {}, "approvals": [], "model_requests": [], "compactions": [],
        "stream_ended": False, "stream_error": None, "stream_errors": [], "stream_end_reason": None,
        "reconnects": 0, "closed_after_turn": None, "event_errors": 0,
    }


def is_new_event(seen, name, data):
    """False for a frame we already had: after a reconnect the stream replays from the last seq.
    stream.* frames carry the cursor as seq, not an event number, and always pass."""
    seq = data.get("seq") if isinstance(data, dict) else None
    if name.startswith("stream.") or not isinstance(seq, int):
        return True
    if seq <= seen["last_seq"]:
        seen["replayed"] += 1
        return False
    seen["last_seq"] = seq
    return True


def follow_stream(api, thread_id, on_event, stop, done, seen, read_timeout=None, retry_delay=1.0):
    """Follows the thread's SSE stream until `stop` is set or the turn ended (seen["done"]),
    reconnecting from the last seq when the connection drops or the server closes it early.
    Each drop before the end of the turn is recorded in seen["stream_error(s)"]; a close after
    the turn is normal (seen["closed_after_turn"]). Gives up after SSE_RECONNECTS failed
    attempts in a row. Always sets `done` on the way out: nothing more can arrive."""
    failures = 0
    try:
        while not stop.is_set():
            before = seen["last_seq"]
            try:
                ending = api.events(thread_id, on_event, stop, since=seen["last_seq"], timeout=read_timeout)
                if ending == "stopped":
                    break
                reason = "server closed the stream" + (f" ({seen['stream_end_reason']})" if seen["stream_end_reason"] else "")
                seen["stream_ended"] = True
            except Exception as e:
                if stop.is_set():
                    break  # the driver is shutting the server down itself
                reason = f"{type(e).__name__}: {e}"
            if seen["done"]:
                seen["closed_after_turn"] = reason
                break
            seen["stream_error"] = reason
            seen["stream_errors"].append({"t_s": round(time.time() - seen["t0"], 1), "error": reason,
                                          "last_seq": seen["last_seq"]})
            failures = 1 if seen["last_seq"] > before else failures + 1
            if failures > SSE_RECONNECTS:
                break
            seen["reconnects"] += 1
            stop.wait(retry_delay)
    finally:
        done.set()


def _mean(values):
    return round(sum(values) / len(values)) if values else None


def build_summary(seen):
    """Counts and timings for report['summary'], from the events seen so far."""
    reqs = seen["model_requests"]
    req_ms = [r["request_ms"] for r in reqs if isinstance(r.get("request_ms"), (int, float))]
    first_ms = [r["first_token_ms"] for r in reqs if isinstance(r.get("first_token_ms"), (int, float))]
    inputs = [r["input_tokens"] for r in reqs if isinstance(r.get("input_tokens"), int)]
    outputs = [r["output_tokens"] for r in reqs if isinstance(r.get("output_tokens"), int)]
    calls = list(seen["tool_calls"].values())
    approvals = seen["approvals"]
    by_tool = {}
    for a in approvals:
        by_tool[a.get("tool") or "?"] = by_tool.get(a.get("tool") or "?", 0) + 1
    diag = seen["turn_diagnostics"] if isinstance(seen["turn_diagnostics"], dict) else {}
    compactions = seen["compactions"]
    return {
        # One turn.usage event per finished model request; modelRequestsStarted comes from the
        # runtime's own diagnostics in turn.completed (it also counts requests that failed).
        "model_requests": len(reqs),
        "model_requests_started": diag.get("modelRequestsStarted"),
        "transport_retries": diag.get("transportRetries"),
        "model_request_ms": req_ms,
        "model_request_ms_total": sum(req_ms),
        "model_request_ms_max": max(req_ms, default=None),
        "model_request_ms_avg": _mean(req_ms),
        "first_token_ms_avg": _mean(first_ms),
        "input_tokens_max": max(inputs, default=None),
        "output_tokens_total": sum(outputs),
        "approvals": len(approvals),
        "approvals_denied": sum(1 for a in approvals if a.get("decision") == "deny"),
        "approvals_by_tool": by_tool,
        "compactions": len(compactions),
        "compactions_auto": sum(1 for c in compactions if c.get("auto")),
        "first_compaction_at_s": compactions[0]["t_s"] if compactions else None,
        "tools_called": [c["tool"] for c in calls],
        "tool_calls_failed": sum(1 for c in calls if c["status"] not in ("completed", "in_progress")),
        "mcp_calls": [c["tool"] for c in calls if is_drive_tool(c["tool"])],
        # Drive tools in the first model request (None: no model.tools.snapshot seen).
        "drive_tools_step0": None if seen["step0_tools"] is None else [n for n in seen["step0_tools"] if is_drive_tool(n)],
        "turn_duration_ms": seen["turn_duration_ms"],
        "stream": {"reconnects": seen["reconnects"], "errors": len(seen["stream_errors"]),
                   "last_error": seen["stream_error"], "end_reason": seen["stream_end_reason"],
                   "closed_after_turn": seen["closed_after_turn"], "replayed_duplicates": seen["replayed"]},
    }


def main():
    # Windows consoles default to a legacy code page; the report prints Russian tool summaries.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser()
    ap.add_argument("--codewhale", required=True, help="codewhale-app-server .exe (a name on PATH is resolved; .cmd/.bat/.ps1 shims are refused)")
    ap.add_argument("--desktop", required=True, help="codewhale-desktop binary (serves `mcp-drive`)")
    ap.add_argument("--godot", required=True)
    ap.add_argument("--project", required=True, help="Godot project folder (water.gdshader is written by the agent)")
    ap.add_argument("--shader", help="shader text the scripted model 'writes' (mock mode only, required there)")
    ap.add_argument("--check-project", required=True, help="folder with check.gd (src-tauri/resources/godot-check)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--render", action="store_true",
                    help="after the turn render the project to water.png: under xvfb-run on Linux, Godot directly on win32")
    ap.add_argument("--timeout", type=int, default=240, help="seconds to wait for the agent's turn (then it is interrupted)")
    ap.add_argument("--ollama-url", help="live mode: a real Ollama server (e.g. http://127.0.0.1:11434) instead of mock_ollama.py")
    ap.add_argument("--model", default="qwen3:4b", help="model tag for the thread (live mode: a tag that exists in Ollama)")
    ap.add_argument("--prompt", help="override the prompt sent to the agent")
    ap.add_argument("--skills", help="folder with <name>/SKILL.md to copy into the temporary CodeWhale home (default: src-tauri/resources/skills)")
    ap.add_argument("--stream-open-timeout", type=int, default=300,
                    help="[stream] open_timeout_secs written to the temporary CodeWhale home's config.toml, the budget "
                         "the desktop app sets (CodeWhale's 45 s default aborts long tool calls from Ollama)")
    ap.add_argument("--base-config",
                    help="config.toml the temporary one is built from (read only). Default in live mode: the user's "
                         "config (CODEWHALE_CONFIG_PATH, $CODEWHALE_HOME/config.toml or %%USERPROFILE%%\\.codewhale\\config.toml), "
                         "skipped when missing; none in mock mode. 'none' disables it")
    ap.add_argument("--deny-tools",
                    help="comma-separated tools whose approvals are answered 'deny' (default: 'Web' in live mode, none in "
                         "mock mode; 'none' for none). Every other approval is answered 'allow'")
    ap.add_argument("--drive",
                    help="knowledge drive the ai-drive server reads (AI_DRIVE_PATH). Default in live mode: "
                         f"{DEFAULT_LIVE_DRIVE} when it exists, else an empty temporary drive; in mock mode the temporary "
                         "drive. 'temp' forces the temporary drive. The ledger and bookmarks (AI_DRIVE_STATE_DIR) always "
                         "go to the temporary folder, so nothing is added to the drive's own ledger")
    ap.add_argument("--kiwix-url", default=DEFAULT_KIWIX_URL,
                    help=f"kiwix-serve for kiwix_search/kiwix_read, passed as KIWIX_URL (default {DEFAULT_KIWIX_URL})")
    ap.add_argument("--always-load", action=argparse.BooleanOptionalAction, default=True,
                    help="write [tools] always_load = [" + ", ".join(ALWAYS_LOAD_TOOLS) + "] into the temporary "
                         "config.toml, merged with the base config's list (default: on)")
    ap.add_argument("--sampling",
                    help="put ollama_proxy.py between CodeWhale and Ollama and add these /v1/chat/completions fields: "
                         "key=value (when missing), key<=value (cap), key:=value (replace), comma-separated, e.g. "
                         "'temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens=4096'; 'default' is that string, "
                         "'none' logs only. Meant for live mode (in mock mode it tests the proxy chain)")
    ap.add_argument("--think-to-effort", action=argparse.BooleanOptionalAction, default=True,
                    help='with --sampling: turn "think": false into reasoning_effort "none" (Ollama /v1 ignores think)')
    ap.add_argument("--context-window", type=int, metavar="N",
                    help='write [providers.ollama.model_context_windows] "<--model>" = N into the temporary config.toml '
                         "(for a model the base config does not list; must match the model's num_ctx)")
    ap.add_argument("--keep-tmp", action="store_true", help="keep the temporary CodeWhale home, workspace and drive folder")
    args = ap.parse_args()
    live = bool(args.ollama_url)
    if args.context_window is not None and args.context_window < 1024:
        ap.error(f"--context-window: {args.context_window} is too small (at least 1024 tokens)")
    proxy_mod = sampling_rules = None
    sampling_warnings = []
    if args.sampling is not None:
        proxy_mod = load_proxy_module()
        try:
            sampling_rules, sampling_warnings = proxy_mod.parse_sampling(args.sampling)
        except ValueError as e:
            ap.error(f"--sampling: {e}")
    if not live and not args.shader:
        ap.error("--shader is required in mock mode (the scripted model writes its text)")
    if args.shader and not Path(args.shader).is_file():
        ap.error(f"--shader: {args.shader!r} is not a file")
    if not Path(args.project).is_dir():
        ap.error(f"--project: {args.project!r} is not a folder")
    codewhale = resolve_binary(ap, "--codewhale", args.codewhale, refuse_shims=True)
    desktop = resolve_binary(ap, "--desktop", args.desktop)
    godot = resolve_binary(ap, "--godot", args.godot)
    deny = parse_tool_list(args.deny_tools if args.deny_tools is not None else ("Web" if live else ""))
    base_note = None
    if args.base_config is not None and args.base_config.strip().lower() == "none":
        base_config = None
        base_note = "disabled (--base-config none)"
    elif args.base_config:
        base_config = Path(args.base_config)
        if not base_config.is_file():
            ap.error(f"--base-config: {args.base_config!r} is not a file")
    elif live:
        base_config = default_base_config()
        if not base_config.is_file():
            base_note = f"skipped: {base_config} does not exist"
            base_config = None
    else:
        base_config = None
        base_note = "not used in mock mode"
    if args.drive and args.drive.strip().lower() == "temp":
        user_drive, drive_note = None, "temporary (--drive temp)"
    elif args.drive:
        user_drive = Path(args.drive).resolve()
        if not user_drive.is_dir():
            ap.error(f"--drive: {args.drive!r} is not a folder")
        drive_note = "--drive"
    elif live and Path(DEFAULT_LIVE_DRIVE).is_dir():
        user_drive, drive_note = Path(DEFAULT_LIVE_DRIVE), "live default"
    else:
        user_drive = None
        drive_note = "temporary (mock mode)" if not live else f"temporary: {DEFAULT_LIVE_DRIVE} does not exist"
    kiwix_url = args.kiwix_url.strip().rstrip("/")
    if "://" not in kiwix_url:
        kiwix_url = "http://" + kiwix_url
    always_load = list(ALWAYS_LOAD_TOOLS) if args.always_load else None

    here = Path(__file__).resolve().parent
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    for name in OUTPUT_FILES:
        (out / name).unlink(missing_ok=True)
    report = {"ok": False, "mode": "live" if live else "mock", "model": args.model,
              "started_at": round(time.time(), 3),
              "settings": {"deny_tools": sorted(deny), "stream_open_timeout_secs": args.stream_open_timeout,
                           "base_config": str(base_config) if base_config else None, "base_config_note": base_note,
                           "timeout_s": args.timeout,
                           # Filled in once the temporary folder exists (drive, state dir, helper).
                           "drive": str(user_drive) if user_drive else None, "drive_kind": "user" if user_drive else "temp",
                           "drive_note": drive_note, "agent_state_dir": None, "sympy_helper": None,
                           "kiwix_url": kiwix_url, "always_load": always_load,
                           "context_window": args.context_window,
                           # Filled in when the proxy is up (its URL and upstream).
                           "sampling": None if args.sampling is None else {
                               "spec": args.sampling, "rules": sampling_rules, "warnings": sampling_warnings,
                               "think_to_effort": args.think_to_effort, "proxy_url": None, "upstream": None,
                               "log": PROXY_LOG}},
              "steps": []}
    seen = new_seen()
    proxy_server = None

    def step(name, ok, **info):
        report["steps"].append({"step": name, "ok": ok, **info})
        print(f"[{'ok' if ok else 'FAIL'}] {name} {json.dumps(info, ensure_ascii=False, default=str)[:300]}", flush=True)
        return ok

    def finalize(code):
        report["summary"] = build_summary(seen)
        if proxy_mod is not None and (out / PROXY_LOG).is_file():
            report["summary"]["sampling_proxy"] = proxy_mod.summarize_log(out / PROXY_LOG)
        report["details"] = {
            "tool_calls": list(seen["tool_calls"].values()),
            "approvals": seen["approvals"],
            "model_requests": seen["model_requests"],
            "compactions": seen["compactions"],
            "stream_errors": seen["stream_errors"],
        }
        return finish(report, out, code)

    procs = []
    server_log = None
    events_log = None
    reader = None
    tmp = None
    stop = threading.Event()
    try:
        tmp = Path(tempfile.mkdtemp(prefix="cwfinal-"))
        workspace = tmp / "proj"
        shutil.copytree(args.project, workspace)
        (workspace / "water.gdshader").unlink(missing_ok=True)
        home = tmp / "home"
        cw_home = home / ".codewhale"
        cw_home.mkdir(parents=True)
        resources = here.parent.parent.parent / "src-tauri" / "resources"
        bundled_helper = resources / "drive" / "sympy_helper.py"
        # The user's drive is only read: SymPy's helper comes from its bin folder (like in the
        # desktop app) or, when missing, from a copy in the temporary folder; the ledger and the
        # bookmarks always go to the temporary folder.
        if user_drive:
            drive = user_drive
            sympy_helper = drive / "bin" / "sympy_helper.py"
            helper_dir = tmp / "bin"
        else:
            drive = tmp / "drive"
            sympy_helper = None
            helper_dir = drive / "bin"
        helper_dir.mkdir(parents=True)
        if sympy_helper is None or not sympy_helper.is_file():
            sympy_helper = helper_dir / "sympy_helper.py"
            if bundled_helper.is_file():
                shutil.copy(bundled_helper, sympy_helper)
        state_dir = tmp / "agent_state"
        report["settings"].update({"drive": str(drive), "agent_state_dir": str(state_dir), "sympy_helper": str(sympy_helper)})
        skills_src = Path(args.skills) if args.skills else resources / "skills"
        for name in ("knowledge-drive", "math-physics-godot"):
            skill = skills_src / name / "SKILL.md"
            if skill.is_file():
                (cw_home / "skills" / name).mkdir(parents=True, exist_ok=True)
                shutil.copy(skill, cw_home / "skills" / name / "SKILL.md")

        ollama_port = free_port()
        server_port = free_port()
        if live:
            scheme, host, port, prefix = parse_ollama_url(args.ollama_url)
            upstream_url = f"{scheme}://{host}:{port}{prefix}"
        else:
            upstream_url = f"http://127.0.0.1:{ollama_port}"
        ollama_base_url = upstream_url + "/v1"
        if proxy_mod is not None:
            # In this process, on a port the OS picks (no race); it serves at once and forwards
            # to the upstream, which is checked below.
            proxy_server = proxy_mod.make_server("127.0.0.1", 0, upstream_url, sampling_rules,
                                                 log_path=str(out / PROXY_LOG), think_to_effort=args.think_to_effort)
            proxy_mod.start_in_thread(proxy_server)
            proxy_url = f"http://127.0.0.1:{proxy_server.server_address[1]}"
            ollama_base_url = proxy_url + "/v1"
            report["settings"]["sampling"].update({"proxy_url": proxy_url, "upstream": upstream_url})
        model_windows = {k: args.context_window for k in window_keys(args.model)} if args.context_window else None

        # config.toml: the user's provider config (live mode) plus the stream budget the desktop
        # app sets, each written once. The user's mcp.json is NOT copied: the driver writes its own.
        # base_url goes into the file whenever it gets a [providers.ollama] table (base config or
        # --context-window), so the file and OLLAMA_BASE_URL never disagree.
        config_path = cw_home / "config.toml"
        try:
            base_text = base_config.read_text(encoding="utf-8-sig") if base_config else None
            config_text = merge_codewhale_config(
                base_text, args.stream_open_timeout,
                ollama_base_url=ollama_base_url if (base_config or model_windows) else None,
                source=str(base_config) if base_config else None,
                always_load=always_load, model_windows=model_windows)
        except (OSError, ValueError, RuntimeError) as e:
            step("codewhale config", False, base_config=str(base_config), error=f"{type(e).__name__}: {e}")
            return finalize(1)
        config_path.write_text(config_text, encoding="utf-8")
        shutil.copy(config_path, out / "codewhale-config.toml")
        merged = tomllib.loads(config_text) if tomllib else {}
        expected_window = configured_window(merged, args.model)
        merged_always_load = ((merged.get("tools") or {}).get("always_load") if tomllib else always_load) or None
        report["settings"]["always_load"] = merged_always_load
        step("codewhale config", True, base_config=str(base_config) if base_config else None, note=base_note,
             stream_open_timeout_secs=args.stream_open_timeout, expected_window=expected_window,
             context_window_flag=args.context_window, always_load=merged_always_load, deny_tools=sorted(deny))

        # The same entry the desktop app writes (src-tauri/src/drive.rs::mcp_entry), except that
        # the state dir stays in the temporary folder.
        mcp = {"servers": {"ai-drive": {
            "command": str(desktop),
            "args": ["mcp-drive"],
            "env": {
                "AI_DRIVE_PATH": str(drive),
                "AI_DRIVE_STATE_DIR": str(state_dir),
                "KIWIX_URL": kiwix_url,
                "GODOT_BIN": str(godot),
                "GODOT_CHECK_PROJECT": str(Path(args.check_project).resolve()),
                "PYTHON_BIN": sys.executable,
                "SYMPY_HELPER": str(sympy_helper),
            },
            "connect_timeout": 30,
            "execute_timeout": 300,
        }}}
        (cw_home / "mcp.json").write_text(json.dumps(mcp, indent=2), encoding="utf-8")
        kiwix = urlsplit(kiwix_url)
        try:
            kiwix_up = wait_http(kiwix.hostname or "127.0.0.1", kiwix.port or (443 if kiwix.scheme == "https" else 80),
                                 "/", 3, kiwix.scheme or "http")
        except ValueError:  # a port that is not a number
            kiwix_up = False
        sections = {name: (drive / name).exists() for name in ("docsets", "git_vault", "kiwix_zim", "index/git_vault.sqlite")}
        step("knowledge drive (informational)", True, drive=str(drive), kind=report["settings"]["drive_kind"],
             note=drive_note, sections=sections, state_dir=str(state_dir), kiwix_url=kiwix_url, kiwix_reachable=kiwix_up)

        if live:
            if not step("real ollama", wait_http(host, port, prefix + "/api/version", 10, scheme),
                        url=f"{scheme}://{host}:{port}{prefix}", model=args.model):
                return finalize(1)
        else:
            mock = subprocess.Popen(
                [sys.executable, "-I", str(here / "mock_ollama.py"), "--port", str(ollama_port), "--log", str(out / "ollama_requests.jsonl")],
                env={**os.environ, "MOCK_SHADER_FILE": str(Path(args.shader).resolve()), "MOCK_WORKSPACE": str(workspace)},
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            procs.append(mock)
            if not step("mock ollama", wait_http("127.0.0.1", ollama_port, "/api/version", 20), port=ollama_port):
                return finalize(1)
        if proxy_server is not None:
            # /api/version through the proxy: proves the proxy forwards to the upstream just checked.
            sampling = report["settings"]["sampling"]
            if not step("sampling proxy", wait_http("127.0.0.1", proxy_server.server_address[1], "/api/version", 10),
                        proxy_url=sampling["proxy_url"], upstream=sampling["upstream"],
                        rules=", ".join(f"{r['key']} {r['mode']} {json.dumps(r['value'])}" for r in sampling_rules) or "none",
                        think_to_effort=args.think_to_effort, warnings=sampling_warnings):
                return finalize(1)

        env = {
            **os.environ,
            "HOME": str(home),
            "CODEWHALE_HOME": str(cw_home),
            # A CODEWHALE_CONFIG_PATH inherited from the user's shell would point CodeWhale at the
            # real config instead of the temporary one.
            "CODEWHALE_CONFIG_PATH": str(config_path),
            "CODEWHALE_RUNTIME_TOKEN": TOKEN,
            "CODEWHALE_TELEMETRY": "0",
            # The desktop app also hands the sidecar the stream budget through the environment.
            "CODEWHALE_STREAM_OPEN_TIMEOUT_SECS": str(args.stream_open_timeout),
            "NO_COLOR": "1",
            "OLLAMA_BASE_URL": ollama_base_url,
        }
        server_log = open(out / "codewhale.log", "wb")
        server = subprocess.Popen(
            [str(codewhale), "app-server", "--http", "--host", "127.0.0.1", "--port", str(server_port)],
            cwd=workspace, env=env, stdout=server_log, stderr=subprocess.STDOUT,
        )
        procs.append(server)
        if not step("codewhale app-server", wait_http("127.0.0.1", server_port, "/health", 90), port=server_port,
                    exit_code=server.poll(), log_tail=tail_of(out / "codewhale.log")):
            return finalize(1)

        api = Api(server_port)
        st, info = api.call("GET", "/v1/runtime/info")
        step("runtime info", st == 200, version=(info or {}).get("codewhale_version") if isinstance(info, dict) else None)

        st, thread = api.call("POST", "/v1/threads", {"model_provider": "ollama", "model": args.model, "workspace": str(workspace)})
        tid = thread.get("id") if isinstance(thread, dict) else None
        step("create thread", st in (200, 201) and bool(tid), thread_id=tid)
        if not tid:
            return finalize(1)

        # What CodeWhale made of config.toml: without the user's provider config every Ollama
        # model gets an 8192-token window and the agent compacts its context every few steps.
        st, ctx = api.call("GET", f"/v1/threads/{tid}/context")
        ctx = ctx if isinstance(ctx, dict) else {}
        window = ctx.get("window_tokens")
        report["context"] = {k: ctx.get(k) for k in ("window_tokens", "output_cap_tokens", "compaction_trigger_tokens", "input_tokens")}
        step("context window (informational)", True, status=st, window_tokens=window, expected=expected_window,
             matches_config=None if expected_window is None else window == expected_window,
             compaction_trigger_tokens=ctx.get("compaction_trigger_tokens"))

        events_log = open(out / "events.jsonl", "w", encoding="utf-8", errors="surrogatepass")
        done = threading.Event()

        def rel(ts):
            return round(ts - seen["t0"], 1)

        def on_event(name, data):
            try:
                if not isinstance(data, dict):
                    data = {"value": data}
                if not is_new_event(seen, name, data):
                    return
                now = time.time()
                if events_log.closed:  # a late frame while the driver is shutting down
                    return
                events_log.write(json.dumps({"ts": round(now, 3), "event": name, "data": data}, ensure_ascii=False) + "\n")
                events_log.flush()
                payload = data.get("payload") if isinstance(data.get("payload"), dict) else {}
                if name == "stream.end":
                    seen["stream_end_reason"] = data.get("reason") or payload.get("reason")
                elif name == "model.tools.snapshot":
                    # The tools of the next model request; the turn's first one (step 0) shows
                    # what the model can call before any tool_search.
                    snap_step, names = snapshot_tool_names(payload)
                    seen["mcp_tools"].update(n for n in names if is_drive_tool(n))
                    if seen["step0_tools"] is None and snap_step in (0, None):
                        seen["step0_tools"] = names
                elif name == "turn.usage":
                    usage = payload.get("usage") or {}
                    seen["model_requests"].append({
                        "t_s": rel(now), "request_ms": payload.get("request_ms"),
                        "first_token_ms": payload.get("first_token_ms"), "duration_ms": payload.get("duration_ms"),
                        "input_tokens": usage.get("input_tokens"), "output_tokens": usage.get("output_tokens")})
                elif name == "approval.required":
                    aid = payload.get("approval_id") or payload.get("id")
                    tool = payload.get("tool_name")
                    decision = "deny" if canonical_tool(tool) in deny else "allow"
                    record = {"t_s": rel(now), "id": aid, "tool": tool, "summary": payload.get("summary"), "decision": decision}
                    seen["approvals"].append(record)
                    if aid:
                        st_a, resp_a = api.call("POST", f"/v1/approvals/{aid}", {"decision": decision, "remember": False})
                        record["http_status"] = st_a
                        if st_a != 200:
                            record["response"] = resp_a
                elif name == "item.started" or name in ITEM_END_EVENTS:
                    item = payload.get("item") or {}
                    kind = item.get("kind")
                    if kind in TOOL_ITEM_KINDS:
                        key = item.get("id") or data.get("item_id") or f"anonymous-{len(seen['tool_calls'])}"
                        tool = item_tool_name(payload)
                        call = seen["tool_calls"].get(key)
                        if call is None:
                            call = seen["tool_calls"][key] = {"tool": tool, "kind": kind, "status": "in_progress", "t_s": rel(now)}
                        elif tool and not call["tool"]:
                            call["tool"] = tool
                        if name in ITEM_END_EVENTS:
                            meta = item.get("metadata") or {}
                            call["status"] = ITEM_END_EVENTS[name]
                            call["seconds"] = round(rel(now) - call["t_s"], 1)
                            call["is_error"] = meta.get("is_error")
                            call["summary"] = (item.get("summary") or "")[:300]
                            # The drive tools the agent actually saw: the executed tool and tool_search hits.
                            if is_drive_tool(call["tool"]):
                                seen["mcp_tools"].add(call["tool"])
                            for ref in meta.get("tool_references") or []:
                                ref_name = ref.get("tool_name") if isinstance(ref, dict) else ref
                                if is_drive_tool(ref_name):
                                    seen["mcp_tools"].add(ref_name)
                    elif name == "item.completed" and kind == "context_compaction":
                        seen["compactions"].append({
                            "t_s": rel(now), "auto": payload.get("auto"),
                            "messages_before": payload.get("messages_before"), "messages_after": payload.get("messages_after"),
                            "summary": (item.get("summary") or "")[:200]})
                    elif name == "item.completed" and kind == "agent_message":
                        seen["final_text"] = item.get("detail") or item.get("summary") or ""
                if name in TURN_END_EVENTS:
                    seen["done"] = name
                    turn = payload.get("turn")
                    if isinstance(turn, dict):
                        seen["turn_status"] = turn.get("status")
                        seen["turn_error"] = turn.get("error")
                        seen["turn_duration_ms"] = turn.get("duration_ms")
                        seen["turn_diagnostics"] = turn.get("modelRequestDiagnostics")
                    done.set()
            except Exception as e:  # a bad frame must not kill the reader thread
                seen["event_errors"] += 1
                print(f"[events] {name}: {type(e).__name__}: {e}", file=sys.stderr, flush=True)

        reader = threading.Thread(target=follow_stream, name="sse-reader", daemon=True,
                                  args=(api, tid, on_event, stop, done, seen, args.timeout + 60))
        reader.start()
        time.sleep(0.5)

        if args.prompt:
            prompt = args.prompt
        elif live:
            prompt = ("Напиши water.gdshader для Godot 4 с тремя волнами Герстнера, пеной на гребнях и френелем, "
                      "подключи его в water_material.tres, проверь шейдер инструментом godot_check и исправляй, "
                      "пока проверка не пройдёт. В конце запиши задачу через ledger_record.")
        else:
            prompt = "Напиши шейдер воды для Godot 4 с волнами Герстнера, проверь его в Godot и сообщи результат. [shader]"
        turn_started = time.time()
        seen["t0"] = turn_started
        st, turn = api.call("POST", f"/v1/threads/{tid}/turns", {"prompt": prompt})
        if not step("post turn", st in (200, 201, 202), status=st, response=turn if st not in (200, 201, 202) else None):
            return finalize(1)
        turn_id = ((turn or {}).get("turn") or {}).get("id") if isinstance(turn, dict) else None

        finished = done.wait(args.timeout)
        interrupt = None
        if not finished and not seen["done"] and turn_id:
            # Out of time: stop the agent so the runtime closes the turn and the events say so.
            st_i, resp_i = api.call("POST", f"/v1/threads/{tid}/turns/{turn_id}/interrupt", {}, timeout=30)
            interrupt = {"status": st_i, "error": resp_i if st_i != 200 else None}
            done.wait(30)
        stop.set()
        # The runtime ends every turn with turn.completed and puts the outcome into payload.turn.status.
        step("turn finished", seen["done"] == "turn.completed" and seen["turn_status"] in (None, "completed"),
             ending=seen["done"], turn_status=seen["turn_status"], turn_error=seen["turn_error"],
             timed_out=not finished, interrupt=interrupt, elapsed_s=round(time.time() - turn_started, 1),
             stream_ended=seen["stream_ended"], stream_error=seen["stream_error"], reconnects=seen["reconnects"],
             mcp_tools=sorted(seen["mcp_tools"]))

        # always_load must put the drive tools into the very first request: without it the model
        # sees them only after a tool_search that finds nothing while the server is unconnected.
        step0 = {canonical_tool(n) for n in seen["step0_tools"] or []}
        step0_drive = sorted(n for n in step0 if is_drive_tool(n))
        if always_load:
            missing = [t for t in always_load if canonical_tool(t) not in step0]
            step("always_load tools at step 0", seen["step0_tools"] is not None and not missing,
                 snapshot_seen=seen["step0_tools"] is not None, missing=missing, drive_tools=step0_drive)
        else:
            step("drive tools at step 0 (informational)", True, snapshot_seen=seen["step0_tools"] is not None,
                 drive_tools=step0_drive)

        if proxy_server is not None:
            # Every model request must have gone through the proxy, or the sampling did not apply.
            time.sleep(0.3)  # the handler of the last request writes its log line after the reply
            ps = proxy_mod.summarize_log(out / PROXY_LOG)
            st_reqs = build_summary(seen)["model_requests_started"]
            step("sampling proxy saw the model requests", ps["chat_requests"] > 0,
                 chat_requests=ps["chat_requests"], model_requests_started=st_reqs, injected=ps["injected"],
                 lowered=ps["lowered"], kept=ps["kept"], finish_reasons=ps["finish_reasons"],
                 length_stops=ps["length_stops"], upstream_errors=ps["upstream_errors"],
                 client_disconnects=ps["client_disconnects"])

        shader_path = workspace / "water.gdshader"
        step("shader written by agent", shader_path.is_file(), path=str(shader_path), bytes=shader_path.stat().st_size if shader_path.is_file() else 0)

        # Real MCP calls only, by the executed tool's name (both CodeWhale spellings); a Web search
        # whose text mentions godot_check does not count.
        calls = list(seen["tool_calls"].values())
        checks = [c for c in calls if canonical_tool(c["tool"]) == GODOT_CHECK_TOOL]
        checks_done = [c for c in checks if c["status"] == "completed"]
        step("godot_check called through MCP", bool(checks_done), calls=len(checks), completed=len(checks_done),
             statuses=[c["status"] for c in checks])
        ledger = [c for c in calls if canonical_tool(c["tool"]) == LEDGER_RECORD_TOOL]
        ledger_done = [c for c in ledger if c["status"] == "completed"]
        step("ledger_record called through MCP (informational)", True, called=bool(ledger_done), calls=len(ledger),
             completed=len(ledger_done))

        final_lower = seen["final_text"].lower()
        reported = "прошла" in final_lower and "не прошла" not in final_lower
        step("agent reported the check" + (" (informational)" if live else ""), reported or live, reported=reported, final_text=seen["final_text"][:600])

        # Independent verification with the tool itself.
        verify_cmd = [str(desktop), "mcp-drive", "call", "godot_check", json.dumps({"path": str(shader_path)})]
        try:
            verify = subprocess.run(
                verify_cmd, env={**os.environ, **mcp["servers"]["ai-drive"]["env"]},
                capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=180,
            )
        except subprocess.TimeoutExpired:
            step("godot_check verdict", False, error="timeout (180 s)")
        else:
            v_out, v_err = verify.stdout or "", verify.stderr or ""
            try:
                verdict = json.loads(v_out)
            except json.JSONDecodeError:
                verdict = None
            if not isinstance(verdict, dict):
                verdict = {"ok": False, "error": "godot_check did not return a JSON object",
                           "exit_code": verify.returncode, "raw": v_out[-500:], "stderr": v_err[-500:]}
            ok = bool(verdict.get("ok"))
            extra = {} if ok else {k: verdict.get(k) for k in ("error", "exit_code", "raw", "stderr") if verdict.get(k) is not None}
            step("godot_check verdict", ok, uniforms=verdict.get("uniforms"), errors=verdict.get("errors"), **extra)

        if args.render:
            png = out / "water.png"
            godot_cmd = [str(godot), "--path", str(workspace), "--resolution", "1280x720", "--quit-after", "120", "--", "--shot", str(png)]
            if sys.platform == "win32":
                cmd = godot_cmd
            else:
                cmd = ["xvfb-run", "-a", "-s", "-screen 0 1280x720x24", godot_cmd[0], "--rendering-driver", "opengl3", *godot_cmd[1:]]
            render_step = "render" + ("" if sys.platform == "win32" else " under xvfb")
            try:
                r = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=300)
            except subprocess.TimeoutExpired:
                step(render_step, False, error="timeout", png=str(png))
            else:
                output = (r.stdout or "") + (r.stderr or "")
                step(render_step, png.is_file() and "SHOT_SAVED" in output, png=str(png), tail=output[-200:])

        report["ok"] = all(s["ok"] for s in report["steps"])
        return finalize(0 if report["ok"] else 1)
    except KeyboardInterrupt:
        step("interrupted (Ctrl+C)", False)
        return finalize(130)
    except Exception as e:
        traceback.print_exc()
        step("crashed", False, error=f"{type(e).__name__}: {e}")
        return finalize(1)
    finally:
        stop.set()
        for p in reversed(procs):  # the app-server first, so it does not log the model vanishing
            try:
                kill_tree(p)
            except Exception as e:
                print(f"[cleanup] pid {p.pid}: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
        if proxy_server is not None:  # after the app-server: nothing sends through it any more
            try:
                proxy_server.shutdown()
                proxy_server.server_close()
            except Exception as e:
                print(f"[cleanup] sampling proxy: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
        if reader is not None:
            reader.join(5)
        for fh in (server_log, events_log):
            if fh is not None:
                try:
                    fh.close()
                except Exception:
                    pass
        if tmp is not None:
            if args.keep_tmp:
                print(f"temporary folder kept: {tmp}", flush=True)
            else:
                remove_tree(tmp)


def finish(report, out, code):
    report["elapsed_s"] = round(time.time() - report["started_at"], 1)
    (out / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2, default=str), encoding="utf-8")
    print("RESULT:", "PASS" if report["ok"] else "FAIL", "->", out / "report.json", flush=True)
    return code


if __name__ == "__main__":
    sys.exit(main())
