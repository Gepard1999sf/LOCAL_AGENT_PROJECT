#!/usr/bin/env python3
"""Sampling proxy in front of Ollama's OpenAI-compatible API (standard library only).

Why: CodeWhale 0.10.1 sends `max_tokens`, `stream_options`, `tool_choice` and (with a
reasoning_effort in its config) `think`, but no temperature, top_p or penalties. Ollama v0.40.4's
/v1 layer (openai/openai.go, FromChatRequest) then puts temperature 1.0 and top_p 1.0 into the
request options itself, which beats the Modelfile's PARAMETER temperature/top_p, and applies no
presence penalty unless the Modelfile has one: in live run 3 qwen3-4b-32k looped on one sentence
until max_tokens (8192 tokens, 204 s).

The proxy listens on 127.0.0.1:<port> and forwards every request to the real Ollama. Only
POST /v1/chat/completions is changed: the configured fields are added when the request lacks
them. Responses (SSE included) are relayed piece by piece as they arrive, with nothing held
back, so tool calls and long generations reach the client when they would without the proxy.
When the client hangs up, the upstream connection is shut at once and Ollama stops generating.

What Ollama v0.40.4 reads from a /v1/chat/completions body (openai/openai.go):
  temperature and top_p (missing -> 1.0, overriding the Modelfile), presence_penalty and
  frequency_penalty (missing -> Modelfile/default), seed, stop, max_tokens (-> num_predict,
  overrides the Modelfile's num_predict), reasoning_effort / reasoning.effort (-> think),
  response_format, logprobs, top_logprobs, keep_alive. Everything else (top_k, min_p,
  repeat_penalty, repeat_last_n, max_completion_tokens, `options`, `think`) is silently dropped,
  so those can only come from the Modelfile.

Sampling spec, comma-separated rules:
  key=value    default: set only when the request has no such field (or null)
  key<=value   ceiling: set when missing, lowered when the request asks for more (numbers only)
  key:=value   override: always replaces the request's value
Values are JSON (0.6, 4096, "x", ["a", "b"]); other words are strings; 'none' means no rules.
Default: temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens=4096
CodeWhale always sends max_tokens (2048 for an 8k window, 8192 for 32k), so `max_tokens=4096`
never fires against it; `max_tokens<=4096` caps it (a capped reply ends with
finish_reason "length").

think -> reasoning_effort (on by default, --no-think-to-effort turns it off): Ollama's /v1
ignores CodeWhale's `think` field, so `reasoning_effort = "none"` in CodeWhale's config does not
switch thinking off. A request with "think": false and no reasoning_effort/reasoning gets
"reasoning_effort": "none", which Ollama maps to think=false. "think": true is left alone (the
model's default is used, as without the proxy).

Every request is one JSONL line in --log: ts, method, path, status, first_byte_ms,
duration_ms, response_bytes and, for chat requests, model, stream, messages, tools,
prompt_chars (the messages as JSON), tools_chars, body_bytes_in/out, injected / lowered /
overridden / kept (field -> value), finish_reason, tool_calls (names), usage, and
client_disconnected / upstream_error when that happened.

Usage:
  py -I -X utf8 ollama_proxy.py --port 11435 [--upstream http://127.0.0.1:11434]
      [--sampling SPEC] [--log proxy.jsonl] [--no-think-to-effort] [--verbose]
  then point CodeWhale at it: OLLAMA_BASE_URL=http://127.0.0.1:11435/v1 (and [providers.ollama]
  base_url in its config.toml).
  py -I -X utf8 ollama_proxy.py --self-test     # fake upstream, no GPU, no Ollama needed
"""

import argparse
import http.client
import json
import re
import select
import socket
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

DEFAULT_SAMPLING = "temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens=4096"
CHAT_PATHS = ("/v1/chat/completions",)
# The /v1/chat/completions fields Ollama v0.40.4 reads (openai.ChatCompletionRequest); a rule for
# any other key is still applied, but Ollama drops the field, so the parser warns about it.
OLLAMA_V1_FIELDS = {
    "model", "messages", "stream", "stream_options", "max_tokens", "seed", "stop", "temperature",
    "frequency_penalty", "presence_penalty", "top_p", "response_format", "tools", "reasoning",
    "reasoning_effort", "logprobs", "top_logprobs", "keep_alive", "tool_choice",
}
MODELFILE_ONLY = {"top_k", "min_p", "repeat_penalty", "repeat_last_n", "num_predict", "num_ctx", "typical_p"}
HOP_BY_HOP = {"connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "proxy-connection",
              "te", "trailer", "trailers", "transfer-encoding", "upgrade"}
MODES = {"=": "default", "<=": "ceiling", ":=": "override"}
RULE_RE = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*(<=|:=|=)\s*(.*?)\s*$", re.S)
READ_SIZE = 65536
JSON_BODY_LIMIT = 8 * 1024 * 1024  # non-streamed responses larger than this are not parsed for usage


# --- sampling rules ---------------------------------------------------------------------------

def _split_top_level(spec):
    """Splits on commas outside brackets, braces and double quotes (so stop=["a","b"] stays whole)."""
    parts, depth, quoted, escaped, cur = [], 0, False, False, []
    for ch in spec:
        if quoted:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                quoted = False
        elif ch == '"':
            quoted = True
        elif ch in "[{":
            depth += 1
        elif ch in "]}":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
            continue
        cur.append(ch)
    parts.append("".join(cur))
    return parts


def _is_number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def parse_sampling(spec):
    """(rules, warnings) for a sampling spec; rules are dicts {key, mode, value}.

    None, "", "none", "off" and "-" give no rules; "default" gives DEFAULT_SAMPLING.
    Raises ValueError for a malformed rule, a ceiling that is not a number or a key given twice."""
    if spec is None or spec.strip().lower() in ("", "none", "off", "-"):
        return [], []
    if spec.strip().lower() == "default":
        spec = DEFAULT_SAMPLING
    rules, warnings = [], []
    for part in _split_top_level(spec):
        if not part.strip():
            continue
        m = RULE_RE.match(part)
        if not m:
            raise ValueError(f"bad sampling rule {part.strip()!r}: expected key=value, key<=value or key:=value")
        key, op, raw = m.groups()
        if raw == "":
            raise ValueError(f"sampling rule {part.strip()!r} has no value")
        try:
            value = json.loads(raw)
        except ValueError:
            value = raw
        mode = MODES[op]
        if mode == "ceiling" and not _is_number(value):
            raise ValueError(f"sampling rule {part.strip()!r}: a ceiling (<=) needs a number")
        if any(r["key"] == key for r in rules):
            raise ValueError(f"sampling key {key!r} is given twice")
        if key in MODELFILE_ONLY:
            warnings.append(f"{key}: Ollama v0.40.4 /v1 drops this field; set PARAMETER {key} in the Modelfile instead")
        elif key not in OLLAMA_V1_FIELDS:
            warnings.append(f"{key}: not a field Ollama v0.40.4 /v1 reads; it is added but Ollama ignores it")
        rules.append({"key": key, "mode": mode, "value": value})
    return rules, warnings


def apply_sampling(body, rules, think_to_effort=True):
    """Applies `rules` to a chat request dict in place; returns what changed:
    {"injected": {k: v}, "lowered": {k: [old, new]}, "overridden": {k: [old, new]}, "kept": {k: v}}."""
    changes = {"injected": {}, "lowered": {}, "overridden": {}, "kept": {}}
    for rule in rules:
        key, mode, value = rule["key"], rule["mode"], rule["value"]
        current = body.get(key)
        if current is None:
            body[key] = value
            changes["injected"][key] = value
        elif mode == "override" and current != value:
            body[key] = value
            changes["overridden"][key] = [current, value]
        elif mode == "ceiling" and _is_number(current) and current > value:
            body[key] = value
            changes["lowered"][key] = [current, value]
        else:
            changes["kept"][key] = current
    if (think_to_effort and body.get("think") is False
            and body.get("reasoning_effort") is None and body.get("reasoning") is None):
        body["reasoning_effort"] = "none"
        changes["injected"]["reasoning_effort"] = "none"
    return changes


# --- response scanning (never delays the relay) -----------------------------------------------

class ResponseScanner:
    """Picks finish_reason, tool call names and usage out of a relayed response. SSE is scanned
    line by line as it passes; a JSON body is kept (up to JSON_BODY_LIMIT) and parsed at the end."""

    def __init__(self, content_type):
        self.sse = (content_type or "").lower().startswith("text/event-stream")
        self.pending = b""
        self.body = bytearray()
        self.events = 0
        self.finish_reason = None
        self.tool_calls = []
        self.usage = None
        self.error = None

    def feed(self, data):
        if not self.sse:
            if len(self.body) + len(data) <= JSON_BODY_LIMIT:
                self.body += data
            return
        lines = (self.pending + data).split(b"\n")
        self.pending = lines.pop()
        if len(self.pending) > JSON_BODY_LIMIT:  # a pathological line: give up on it
            self.pending = b""
        for line in lines:
            self._sse_line(line.strip())

    def close(self):
        if self.sse:
            if self.pending.strip():
                self._sse_line(self.pending.strip())
            return
        if self.body:
            try:
                self._chunk(json.loads(bytes(self.body)), streamed=False)
            except ValueError:
                pass

    def _sse_line(self, line):
        if not line.startswith(b"data:"):
            return
        data = line[5:].strip()
        if not data or data == b"[DONE]":
            return
        self.events += 1
        # Cheap path: most chunks are text deltas with nothing to record.
        if (b'"finish_reason":"' not in data.replace(b" ", b"") and b'"usage"' not in data
                and b'"tool_calls"' not in data and b'"error"' not in data):
            return
        try:
            self._chunk(json.loads(data), streamed=True)
        except ValueError:
            pass

    def _chunk(self, obj, streamed):
        if not isinstance(obj, dict):
            return
        if obj.get("error") is not None:
            self.error = obj["error"]
        for choice in obj.get("choices") or []:
            if not isinstance(choice, dict):
                continue
            if choice.get("finish_reason"):
                self.finish_reason = choice["finish_reason"]
            part = choice.get("delta" if streamed else "message") or {}
            for call in (part.get("tool_calls") if isinstance(part, dict) else None) or []:
                name = ((call or {}).get("function") or {}).get("name")
                if name:
                    self.tool_calls.append(name)
        if isinstance(obj.get("usage"), dict):
            self.usage = obj["usage"]


# --- proxy ------------------------------------------------------------------------------------

def parse_upstream(raw):
    """(scheme, host, port, prefix) of the upstream URL; a trailing /v1 is dropped from prefix."""
    url = raw if "://" in raw else "http://" + raw
    u = urlsplit(url)
    scheme = u.scheme or "http"
    host = u.hostname or "127.0.0.1"
    port = u.port or (443 if scheme == "https" else 11434)
    prefix = u.path.rstrip("/")
    if prefix.endswith("/v1"):
        prefix = prefix[:-3]
    return scheme, host, port, prefix


class ProxyConfig:
    def __init__(self, upstream, rules, log_path=None, think_to_effort=True, timeout=3600.0, verbose=False):
        self.scheme, self.host, self.port, self.prefix = parse_upstream(upstream)
        self.upstream_url = f"{self.scheme}://{self.host}:{self.port}{self.prefix}"
        default_port = 443 if self.scheme == "https" else 80
        self.host_header = self.host if self.port == default_port else f"{self.host}:{self.port}"
        self.rules = rules
        self.think_to_effort = think_to_effort
        self.timeout = timeout
        self.verbose = verbose
        self.log_path = log_path
        self._log_lock = threading.Lock()
        self._log = open(log_path, "a", encoding="utf-8") if log_path else None

    def connect(self):
        cls = http.client.HTTPSConnection if self.scheme == "https" else http.client.HTTPConnection
        return cls(self.host, self.port, timeout=self.timeout)

    def log(self, entry):
        if self.verbose:
            short = {k: entry.get(k) for k in ("method", "path", "status", "model", "injected", "lowered",
                                                "finish_reason", "duration_ms") if entry.get(k) not in (None, {}, [])}
            print("[ollama-proxy] " + json.dumps(short, ensure_ascii=False), file=sys.stderr, flush=True)
        if self._log is None:
            return
        line = json.dumps(entry, ensure_ascii=False, default=str) + "\n"
        with self._log_lock:
            try:
                self._log.write(line)
                self._log.flush()
            except (OSError, ValueError):
                pass

    def close(self):
        with self._log_lock:
            if self._log is not None:
                try:
                    self._log.close()
                except OSError:
                    pass
                self._log = None


class ProxyHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ollama-sampling-proxy/1"

    def setup(self):
        super().setup()
        try:  # SSE pieces are small: do not let Nagle hold them back
            self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except OSError:
            pass

    def log_message(self, fmt, *args):
        if self.server.cfg.verbose:
            sys.stderr.write("[ollama-proxy] " + (fmt % args) + "\n")

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = lambda self: self._proxy()

    # --- request side ---

    def _read_body(self):
        if "chunked" in (self.headers.get("Transfer-Encoding") or "").lower():
            chunks = []
            while True:
                line = self.rfile.readline(65537)
                if not line:
                    raise ValueError("request body ended inside a chunk header")
                try:
                    size = int(line.split(b";", 1)[0].strip(), 16)
                except ValueError:
                    raise ValueError(f"bad chunk size {line[:40]!r}") from None
                if size == 0:
                    while self.rfile.readline(65537) not in (b"\r\n", b"\n", b""):
                        pass  # trailers
                    return b"".join(chunks)
                chunks.append(self.rfile.read(size))
                self.rfile.readline(65537)
        length = self.headers.get("Content-Length")
        if length is None:
            return None
        try:
            n = int(length)
        except ValueError:
            raise ValueError(f"bad Content-Length {length!r}") from None
        return self.rfile.read(n) if n > 0 else b""

    def _rewrite_chat(self, body, entry):
        cfg = self.server.cfg
        try:
            payload = json.loads(body)
        except ValueError:
            entry["note"] = "body is not JSON: forwarded unchanged"
            return body
        if not isinstance(payload, dict):
            return body
        messages = payload.get("messages") if isinstance(payload.get("messages"), list) else []
        tools = payload.get("tools") if isinstance(payload.get("tools"), list) else []
        changes = apply_sampling(payload, cfg.rules, cfg.think_to_effort)
        entry.update({
            "chat": True, "model": payload.get("model"), "stream": payload.get("stream"),
            "messages": len(messages), "tools": len(tools),
            "prompt_chars": len(json.dumps(messages, ensure_ascii=False)),
            "tools_chars": len(json.dumps(tools, ensure_ascii=False)),
            "body_bytes_in": len(body), **changes,
        })
        if changes["injected"] or changes["lowered"] or changes["overridden"]:
            body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        entry["body_bytes_out"] = len(body)
        return body

    def _send_error(self, status, message, entry):
        raw = json.dumps({"error": {"message": message, "type": "proxy_error", "status": status}}).encode()
        entry["status"] = status
        entry["upstream_error"] = message
        try:
            self.send_response_only(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
        except OSError:
            self.close_connection = True

    # --- the exchange ---

    def _proxy(self):
        cfg = self.server.cfg
        t0 = time.monotonic()
        entry = {"ts": round(time.time(), 3), "method": self.command, "path": self.path}
        try:
            self._exchange(cfg, t0, entry)
        finally:
            entry["duration_ms"] = round((time.monotonic() - t0) * 1000)
            cfg.log(entry)

    def _exchange(self, cfg, t0, entry):
        try:
            body = self._read_body()
        except ValueError as e:
            self.close_connection = True
            return self._send_error(400, str(e), entry)
        if self.command == "POST" and urlsplit(self.path).path.rstrip("/") in CHAT_PATHS and body:
            body = self._rewrite_chat(body, entry)

        drop = set(HOP_BY_HOP) | {"host", "content-length"}
        drop |= {t.strip().lower() for t in (self.headers.get("Connection") or "").split(",") if t.strip()}
        conn = cfg.connect()
        upstream_sock = None
        try:
            conn.putrequest(self.command, cfg.prefix + self.path, skip_host=True, skip_accept_encoding=True)
            conn.putheader("Host", cfg.host_header)
            for name, value in self.headers.items():
                if name.lower() not in drop:
                    conn.putheader(name, value)
            if body is not None:
                conn.putheader("Content-Length", str(len(body)))
            conn.endheaders(body)
            upstream_sock = conn.sock
            try:
                upstream_sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            except (OSError, AttributeError):
                pass
            # Ollama answers only when the prompt is processed (or a tool call is complete), so a
            # client that gives up meanwhile must also be noticed while we wait for the headers.
            watcher = self._watch_client(upstream_sock)
            try:
                resp = conn.getresponse()
            except (OSError, http.client.HTTPException) as e:
                watcher.set()
                if self._client_gone:
                    entry["client_disconnected"] = True
                    self.close_connection = True
                    return None
                return self._send_error(502, f"upstream {cfg.upstream_url} failed: {type(e).__name__}: {e}", entry)
            try:
                self._relay(resp, entry, t0)
            finally:
                watcher.set()
                resp.close()
        except (OSError, http.client.HTTPException) as e:
            return self._send_error(502, f"upstream {cfg.upstream_url} unreachable: {type(e).__name__}: {e}", entry)
        finally:
            conn.close()

    def _watch_client(self, upstream_sock):
        """Shuts the upstream socket when the client closes its connection during the exchange, so
        Ollama notices the cancel at once (a blocked relay would notice it only at the next write).
        Returns the Event that stops the watcher."""
        self._client_gone = False
        stop = threading.Event()
        client = self.connection

        def watch():
            while not stop.is_set():
                try:
                    readable, _, _ = select.select([client], [], [], 0.25)
                except (OSError, ValueError):
                    return
                if not readable or stop.is_set():
                    continue
                try:
                    peek = client.recv(1, socket.MSG_PEEK)
                except BlockingIOError:
                    continue
                except OSError:
                    peek = b""
                if peek:
                    return  # the client sent more data (pipelining): not a hang-up
                self._client_gone = True
                try:
                    upstream_sock.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                return

        threading.Thread(target=watch, name="client-watch", daemon=True).start()
        return stop

    def _relay(self, resp, entry, t0):
        entry["status"] = resp.status
        no_body = self.command == "HEAD" or resp.status in (204, 304) or 100 <= resp.status < 200
        length = resp.getheader("Content-Length")
        self.send_response_only(resp.status, resp.reason)
        for name, value in resp.getheaders():
            if name.lower() not in HOP_BY_HOP and name.lower() != "content-length":
                self.send_header(name, value)
        chunked = False
        if length is not None and not resp.chunked:
            self.send_header("Content-Length", length)
        elif not no_body:
            self.send_header("Transfer-Encoding", "chunked")
            chunked = True
        try:
            self.end_headers()
        except OSError:
            self._client_gone = True
            entry["client_disconnected"] = True
            self.close_connection = True
            return
        scanner = ResponseScanner(resp.getheader("Content-Type")) if entry.get("chat") else None
        total = 0
        complete = True
        while not no_body:
            try:
                data = resp.read1(READ_SIZE)
            except (OSError, http.client.HTTPException) as e:
                if not self._client_gone:
                    entry["upstream_error"] = f"{type(e).__name__}: {e}"
                complete = False
                break
            if not data:
                # b"" ends the body; it also comes back when the watcher shut the upstream socket.
                # A chunked body cut short raises IncompleteRead instead, and a Content-Length body
                # is complete when every byte came, so only a close-delimited body needs the flag.
                if resp.chunked:
                    complete = True
                elif length is not None:
                    complete = length.isdigit() and total == int(length)
                else:
                    complete = not self._client_gone
                break
            if "first_byte_ms" not in entry:
                entry["first_byte_ms"] = round((time.monotonic() - t0) * 1000)
            try:
                self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data) if chunked else data)
            except OSError:
                self._client_gone = True
                complete = False
                break
            total += len(data)
            if scanner is not None:
                scanner.feed(data)
        entry["response_bytes"] = total
        if self._client_gone and not complete:
            entry["client_disconnected"] = True  # the hang-up cut the exchange short
        if complete:
            if chunked:
                try:
                    self.wfile.write(b"0\r\n\r\n")
                except OSError:
                    self.close_connection = True
        else:
            self.close_connection = True  # an unfinished body cannot be followed by another response
        if scanner is not None:
            scanner.close()
            entry["sse_events"] = scanner.events if scanner.sse else None
            entry["finish_reason"] = scanner.finish_reason
            entry["tool_calls"] = scanner.tool_calls
            entry["usage"] = scanner.usage
            if scanner.error is not None:
                entry["response_error"] = scanner.error


class ProxyServer(ThreadingHTTPServer):
    daemon_threads = True
    # On Windows SO_REUSEADDR lets a second server bind a port that is already in use.
    allow_reuse_address = sys.platform != "win32"

    def __init__(self, address, cfg):
        self.cfg = cfg
        super().__init__(address, ProxyHandler)

    def handle_error(self, request, client_address):
        # A client that resets an idle keep-alive connection (CodeWhale exiting) is not an error.
        if isinstance(sys.exc_info()[1], (ConnectionError, TimeoutError)):
            return
        super().handle_error(request, client_address)

    def server_close(self):
        super().server_close()
        self.cfg.close()


def make_server(host, port, upstream, rules, log_path=None, think_to_effort=True, timeout=3600.0, verbose=False):
    """A ProxyServer bound to host:port (0 picks a free port: see server.server_address)."""
    cfg = ProxyConfig(upstream, rules, log_path=log_path, think_to_effort=think_to_effort,
                      timeout=timeout, verbose=verbose)
    return ProxyServer((host, port), cfg)


def start_in_thread(server):
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.2},
                              name="ollama-proxy", daemon=True)
    thread.start()
    return thread


# --- log summary ------------------------------------------------------------------------------

def summarize_log(path):
    """Counts over a proxy log for report.json: chat requests, how often each field was injected,
    lowered or overridden, the values the client sent itself, finish reasons, prompt sizes."""
    out = {"requests": 0, "chat_requests": 0, "injected": {}, "lowered": {}, "overridden": {}, "kept": {},
           "finish_reasons": {}, "length_stops": 0, "prompt_chars_max": None, "prompt_tokens_max": None,
           "completion_tokens_total": 0, "first_byte_ms_max": None, "client_disconnects": 0,
           "upstream_errors": 0, "statuses": {}}
    try:
        lines = open(path, encoding="utf-8").read().splitlines()
    except OSError:
        out["error"] = f"no log at {path}"
        return out
    for line in lines:
        try:
            e = json.loads(line)
        except ValueError:
            continue
        out["requests"] += 1
        status = str(e.get("status"))
        out["statuses"][status] = out["statuses"].get(status, 0) + 1
        if e.get("upstream_error"):
            out["upstream_errors"] += 1
        if e.get("client_disconnected"):
            out["client_disconnects"] += 1
        if not e.get("chat"):
            continue
        out["chat_requests"] += 1
        for kind in ("injected", "lowered", "overridden"):
            for key in e.get(kind) or {}:
                out[kind][key] = out[kind].get(key, 0) + 1
        for key, value in (e.get("kept") or {}).items():
            seen = out["kept"].setdefault(key, [])
            if value not in seen:
                seen.append(value)
        reason = e.get("finish_reason")
        if reason:
            out["finish_reasons"][reason] = out["finish_reasons"].get(reason, 0) + 1
            out["length_stops"] += reason == "length"
        usage = e.get("usage") or {}
        if isinstance(e.get("prompt_chars"), int):
            out["prompt_chars_max"] = max(out["prompt_chars_max"] or 0, e["prompt_chars"])
        if isinstance(usage.get("prompt_tokens"), int):
            out["prompt_tokens_max"] = max(out["prompt_tokens_max"] or 0, usage["prompt_tokens"])
        if isinstance(usage.get("completion_tokens"), int):
            out["completion_tokens_total"] += usage["completion_tokens"]
        if isinstance(e.get("first_byte_ms"), int):
            out["first_byte_ms_max"] = max(out["first_byte_ms_max"] or 0, e["first_byte_ms"])
    return out


# --- self-test --------------------------------------------------------------------------------

def self_test():
    """Proxy against a fake upstream on loopback: rules, byte-exact pass-through, streaming timing,
    keep-alive, close-delimited bodies, upstream down, client hang-ups. No Ollama, no GPU."""
    import os
    import tempfile

    gap = 0.25  # seconds between upstream SSE pieces; a buffering proxy would add about this much
    state = {"bodies": [], "raw": [], "sent_at": [], "hangups": {}, "lock": threading.Lock()}

    class Upstream(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def _json(self, payload, status=200):
            raw = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def _piece(self, data):
            self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
            self.wfile.flush()

        def do_GET(self):
            if self.path == "/api/version":
                return self._json({"version": "0.40.4-fake"})
            if self.path == "/close-delimited":
                self.send_response(200)
                self.send_header("Content-Type", "text/plain")
                self.send_header("Connection", "close")
                self.end_headers()
                for i in range(3):
                    self.wfile.write(b"part%d;" % i)
                    self.wfile.flush()
                    time.sleep(0.05)
                self.close_connection = True
                return None
            return self._json({"error": "not found"}, 404)

        def do_HEAD(self):
            self.send_response(200)
            self.send_header("Content-Length", "17")
            self.end_headers()

        def do_POST(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            with state["lock"]:
                state["raw"].append((self.path, raw, self.headers.get("Transfer-Encoding")))
            if self.path == "/api/show":
                return self._json({"echo": raw.decode("utf-8")})
            body = json.loads(raw)
            with state["lock"]:
                state["bodies"].append(body)
            marker = (body.get("messages") or [{}])[-1].get("content", "")
            if not body.get("stream"):
                return self._json({"id": "x", "object": "chat.completion",
                                   "choices": [{"index": 0, "finish_reason": "stop",
                                                "message": {"role": "assistant", "content": "привет"}}],
                                   "usage": {"prompt_tokens": 11, "completion_tokens": 2, "total_tokens": 13}})
            if marker == "[silent]":
                # Not even the headers until the peer goes away: Ollama sends nothing while it
                # processes the prompt or builds a tool call.
                t = time.perf_counter()
                while time.perf_counter() - t < 10:
                    r, _, _ = select.select([self.connection], [], [], 0.02)
                    if r:
                        try:
                            gone = not self.connection.recv(1, socket.MSG_PEEK)
                        except OSError:
                            gone = True
                        if gone:
                            with state["lock"]:
                                state["hangups"]["silent"] = time.perf_counter()
                            break
                self.close_connection = True
                return None
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            if marker == "[endless]":
                try:
                    for _ in range(400):
                        self._piece(b'data: {"choices":[{"index":0,"delta":{"content":"x"}}]}\n\n')
                        time.sleep(0.025)
                except OSError:
                    with state["lock"]:
                        state["hangups"]["endless"] = time.perf_counter()
                self.close_connection = True
                return None
            events = [
                {"choices": [{"index": 0, "delta": {"role": "assistant", "reasoning": "думаю "}}]},
                {"choices": [{"index": 0, "delta": {"content": "вода "}}]},
                {"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "c1", "type": "function",
                    "function": {"name": "mcp_ai--drive_godot_check", "arguments": "{\"path\":\"water.gdshader\"}"}}]}}]},
                {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
                {"choices": [], "usage": {"prompt_tokens": 1234, "completion_tokens": 56, "total_tokens": 1290}},
            ]
            pieces = [("data: " + json.dumps(ev, ensure_ascii=False) + "\n\n").encode() for ev in events]
            pieces.append(b"data: [DONE]\n\n")
            for p in pieces:
                with state["lock"]:
                    state["sent_at"].append(time.perf_counter())
                self._piece(p)
                time.sleep(gap)
            self.wfile.write(b"0\r\n\r\n")
            return None

    checks = []

    def check(name, ok, detail=""):
        checks.append((name, bool(ok)))
        print(f"[{'ok' if ok else 'FAIL'}] {name}" + (f": {detail}" if detail and not ok else ""), flush=True)

    # Rule parsing.
    rules, warns = parse_sampling(DEFAULT_SAMPLING)
    check("default spec parses", [r["key"] for r in rules] == ["temperature", "top_p", "presence_penalty", "max_tokens"]
          and all(r["mode"] == "default" for r in rules) and not warns, (rules, warns))
    r2, w2 = parse_sampling('stop=["a,b","c"],max_tokens<=4096,seed:=7,top_k=20')
    check("lists keep their commas, modes and warnings",
          r2[0]["value"] == ["a,b", "c"] and r2[1]["mode"] == "ceiling" and r2[2]["mode"] == "override"
          and any(w.startswith("top_k") for w in w2), (r2, w2))
    for bad in ("temperature", "max_tokens<=abc", "temperature=0.6,temperature=0.7", "top_p="):
        try:
            parse_sampling(bad)
            check(f"bad spec {bad!r} refused", False)
        except ValueError:
            check(f"bad spec {bad!r} refused", True)
    check("'none' gives no rules", parse_sampling("none") == ([], []))
    body = {"max_tokens": 8192, "temperature": None}
    ch = apply_sampling(body, parse_sampling(DEFAULT_SAMPLING)[0])
    check("default max_tokens keeps the client's value, null counts as missing",
          body["max_tokens"] == 8192 and body["temperature"] == 0.6 and ch["kept"] == {"max_tokens": 8192}, ch)

    class UpstreamServer(ThreadingHTTPServer):
        daemon_threads = True
        allow_reuse_address = sys.platform != "win32"

    up = UpstreamServer(("127.0.0.1", 0), Upstream)
    start_in_thread(up)
    up_url = f"http://127.0.0.1:{up.server_address[1]}"
    tmpdir = tempfile.mkdtemp(prefix="ollama-proxy-test-")
    log_path = os.path.join(tmpdir, "proxy.jsonl")
    spec = "temperature=0.6,top_p=0.95,presence_penalty=1.5,max_tokens<=4096,seed:=7"
    proxy = make_server("127.0.0.1", 0, up_url + "/v1", parse_sampling(spec)[0], log_path=log_path)
    start_in_thread(proxy)
    pport = proxy.server_address[1]

    def conn():
        return http.client.HTTPConnection("127.0.0.1", pport, timeout=15)

    try:
        c = conn()
        c.request("GET", "/api/version")
        r = c.getresponse()
        check("GET passes through", r.status == 200 and json.loads(r.read())["version"] == "0.40.4-fake")
        first_sock = c.sock
        raw_show = json.dumps({"model": "qwen", "verbose": True, "x": "кириллица"}, ensure_ascii=False).encode()
        c.request("POST", "/api/show", body=raw_show, headers={"Content-Type": "application/json"})
        r = c.getresponse()
        echoed = json.loads(r.read())["echo"].encode("utf-8")
        check("non-chat body is forwarded byte for byte, on the same keep-alive connection",
              r.status == 200 and echoed == raw_show and state["raw"][-1][1] == raw_show
              and first_sock is not None and c.sock is first_sock)

        # Streaming chat: rules applied, every piece relayed when it is sent.
        req = {"model": "qwen3-4b-32k", "stream": True, "stream_options": {"include_usage": True},
               "max_tokens": 8192, "temperature": 0.2, "seed": 1, "think": False, "tool_choice": "auto",
               "messages": [{"role": "user", "content": "шейдер"}], "tools": [{"type": "function", "function": {"name": "write"}}]}
        c.request("POST", "/v1/chat/completions", body=json.dumps(req).encode(), headers={"Content-Type": "application/json"})
        r = c.getresponse()
        got, arrivals, buf = b"", [], b""
        while True:
            data = r.read1(READ_SIZE)
            if not data:
                break
            now = time.perf_counter()
            got += data
            buf += data
            while b"\n\n" in buf:
                _, buf = buf.split(b"\n\n", 1)
                arrivals.append(now)
        sent = list(state["sent_at"])
        up_body = state["bodies"][-1]
        check("upstream got the injected fields", up_body.get("top_p") == 0.95 and up_body.get("presence_penalty") == 1.5, up_body)
        check("client temperature kept, max_tokens lowered, seed overridden",
              up_body.get("temperature") == 0.2 and up_body.get("max_tokens") == 4096 and up_body.get("seed") == 7, up_body)
        check("think false -> reasoning_effort none", up_body.get("reasoning_effort") == "none" and up_body.get("think") is False, up_body)
        check("untouched fields survive", up_body.get("tool_choice") == "auto" and up_body.get("tools") == req["tools"]
              and up_body.get("messages") == req["messages"], up_body)
        check("stream content is byte-identical", got.count(b"data: ") == 6 and got.endswith(b"data: [DONE]\n\n")
              and "mcp_ai--drive_godot_check".encode() in got, got[-200:])
        lags = [round(a - s, 3) for a, s in zip(arrivals, sent)]
        check(f"every SSE piece arrives within 100 ms of being sent (gap {gap}s; lags {lags})",
              len(arrivals) == len(sent) == 6 and max(lags) < 0.1, (len(arrivals), len(sent), lags))

        # Non-streamed chat with nothing set: everything injected; usage parsed from the JSON body.
        req2 = {"model": "m", "stream": False, "messages": [{"role": "user", "content": "hi"}]}
        c.request("POST", "/v1/chat/completions", body=json.dumps(req2).encode(), headers={"Content-Type": "application/json"})
        r = c.getresponse()
        resp2 = json.loads(r.read())
        b2 = state["bodies"][-1]
        check("non-streamed chat: all rules injected", b2.get("temperature") == 0.6 and b2.get("max_tokens") == 4096
              and b2.get("seed") == 7 and "reasoning_effort" not in b2, b2)
        check("non-streamed response unchanged", resp2["choices"][0]["message"]["content"] == "привет")
        c.close()

        # A chunked request body (no Content-Length) reaches the upstream as a whole.
        s = socket.create_connection(("127.0.0.1", pport), timeout=10)
        payload = json.dumps({"model": "m", "stream": False, "messages": [{"role": "user", "content": "chunked"}]}).encode()
        half = len(payload) // 2
        s.sendall(b"POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n"
                  b"Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                  + b"%x\r\n%s\r\n%x\r\n%s\r\n0\r\n\r\n" % (half, payload[:half], len(payload) - half, payload[half:]))
        reply = b""
        while True:
            data = s.recv(65536)
            if not data:
                break
            reply += data
        s.close()
        check("chunked request body is de-chunked for the upstream",
              reply.startswith(b"HTTP/1.1 200") and state["bodies"][-1]["messages"][0]["content"] == "chunked"
              and state["raw"][-1][2] is None, reply[:120])

        c = conn()
        c.request("GET", "/close-delimited")
        r = c.getresponse()
        check("close-delimited upstream body is relayed whole", r.read() == b"part0;part1;part2;")
        c.close()
        c = conn()
        c.request("HEAD", "/api/version")
        r = c.getresponse()
        check("HEAD keeps Content-Length, sends no body", r.status == 200 and r.getheader("Content-Length") == "17" and r.read() == b"")
        c.close()
        # A client that reads the whole reply and closes at once (the driver's wait_http) did
        # not hang up early: the log must not say client_disconnected for it.
        for _ in range(5):
            s = socket.create_connection(("127.0.0.1", pport), timeout=10)
            s.sendall(b"GET /api/version HTTP/1.1\r\nHost: x\r\n\r\n")
            reply = b""
            while b"0.40.4-fake" not in reply:
                data = s.recv(4096)
                if not data:
                    break
                reply += data
            s.close()

        def hang_up(marker, read_first):
            """Sends a streaming chat request on a raw socket, hangs up, and returns how long the
            upstream took to notice."""
            s = socket.create_connection(("127.0.0.1", pport), timeout=10)
            raw = json.dumps({"model": "m", "stream": True, "messages": [{"role": "user", "content": marker}]}).encode()
            s.sendall(b"POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n"
                      b"Content-Length: %d\r\n\r\n%s" % (len(raw), raw))
            if read_first:
                s.recv(4096)
            else:
                time.sleep(0.4)
            t_close = time.perf_counter()
            s.close()
            key = marker.strip("[]")
            deadline = time.perf_counter() + 3
            while key not in state["hangups"] and time.perf_counter() < deadline:
                time.sleep(0.02)
            return state["hangups"].get(key, t_close + 99) - t_close

        # Ollama is silent while it processes the prompt or builds a tool call.
        lag = hang_up("[silent]", read_first=False)
        check(f"a hang-up before the first byte reaches the upstream fast ({lag:.2f}s)", lag < 1.5)
        lag = hang_up("[endless]", read_first=True)
        check(f"a hang-up mid-stream reaches the upstream fast ({lag:.2f}s)", lag < 1.5)

        # Upstream down.
        dead = make_server("127.0.0.1", 0, "http://127.0.0.1:9", [])
        start_in_thread(dead)
        c = http.client.HTTPConnection("127.0.0.1", dead.server_address[1], timeout=15)
        c.request("POST", "/v1/chat/completions", body=b'{"model":"m","messages":[{"role":"user","content":"x"}]}')
        r = c.getresponse()
        err = json.loads(r.read())
        check("upstream down -> 502 with a JSON error", r.status == 502 and "unreachable" in err["error"]["message"], err)
        c.close()
        dead.shutdown()
        dead.server_close()
    finally:
        time.sleep(0.3)  # let handler threads write their log lines
        proxy.shutdown()
        proxy.server_close()
        up.shutdown()
        up.server_close()

    entries = [json.loads(line) for line in open(log_path, encoding="utf-8")]
    chat = [e for e in entries if e.get("chat")]
    first = chat[0] if chat else {}
    check("log: one line per request", len(entries) >= 14, len(entries))
    check("log: injected/lowered/overridden/kept recorded",
          first.get("injected") == {"top_p": 0.95, "presence_penalty": 1.5, "reasoning_effort": "none"}
          and first.get("lowered") == {"max_tokens": [8192, 4096]} and first.get("overridden") == {"seed": [1, 7]}
          and first.get("kept") == {"temperature": 0.2}, first)
    check("log: finish_reason, tool call names, usage, prompt size",
          first.get("finish_reason") == "tool_calls" and first.get("tool_calls") == ["mcp_ai--drive_godot_check"]
          and (first.get("usage") or {}).get("prompt_tokens") == 1234 and first.get("prompt_chars", 0) > 10
          and first.get("first_byte_ms") is not None, first)
    check("log: the two hang-ups recorded, complete replies closed at once are not",
          [e["path"] for e in entries if e.get("client_disconnected")] == ["/v1/chat/completions"] * 2,
          [(e["path"], e.get("response_bytes")) for e in entries if e.get("client_disconnected")])
    summary = summarize_log(log_path)
    check("summary counts", summary["chat_requests"] == len(chat) and summary["injected"].get("temperature") >= 2
          and summary["lowered"] == {"max_tokens": 1} and summary["finish_reasons"].get("tool_calls") == 1, summary)
    failed = [name for name, ok in checks if not ok]
    print(f"SELF-TEST {'PASS' if not failed else 'FAIL'}: {len(checks) - len(failed)}/{len(checks)} checks", flush=True)
    return 0 if not failed else 1


def main():
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description="Sampling proxy for Ollama's /v1/chat/completions")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=11435, help="0 picks a free port (printed on start)")
    ap.add_argument("--upstream", default="http://127.0.0.1:11434", help="the real Ollama (a /v1 suffix is fine)")
    ap.add_argument("--sampling", default=DEFAULT_SAMPLING,
                    help=f"rules key=value (default), key<=value (ceiling), key:=value (override); default {DEFAULT_SAMPLING!r}")
    ap.add_argument("--think-to-effort", action=argparse.BooleanOptionalAction, default=True,
                    help='turn "think": false into "reasoning_effort": "none" (Ollama /v1 ignores think)')
    ap.add_argument("--log", help="JSONL file, one line per request")
    ap.add_argument("--timeout", type=float, default=3600.0, help="upstream socket timeout, seconds")
    ap.add_argument("--verbose", action="store_true")
    ap.add_argument("--self-test", action="store_true", help="run the built-in tests against a fake upstream and exit")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    try:
        rules, warnings = parse_sampling(args.sampling)
    except ValueError as e:
        ap.error(str(e))
    for w in warnings:
        print("warning: " + w, file=sys.stderr, flush=True)
    server = make_server(args.host, args.port, args.upstream, rules, log_path=args.log,
                         think_to_effort=args.think_to_effort, timeout=args.timeout, verbose=args.verbose)
    host, port = server.server_address[:2]
    print(f"ollama proxy on http://{host}:{port} -> {server.cfg.upstream_url}; rules: "
          + (", ".join(f"{r['key']} {r['mode']} {json.dumps(r['value'], ensure_ascii=False)}" for r in rules) or "none"),
          flush=True)
    try:
        server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
