#!/usr/bin/env python3
"""Deterministic Ollama stand-in used to record live CodeWhale fixtures.

Real Ollama (and model weights) could not be downloaded in the recording
sandbox, so this tiny stdlib server speaks just enough of the Ollama native
API (/api/tags, /api/show, /api/version) and the OpenAI-compatible
/v1/chat/completions streaming dialect for CodeWhale to run full turns,
including tool calls. Every request body is appended to a JSONL log so the
exact payload CodeWhale sends to the model can be inspected.

Scenario is chosen from the LAST user message text:
  contains "[tool:read]"   -> model calls the read tool, then answers
  contains "[tool:write]"  -> model calls the write tool (approval path)
  contains "[tool:bash]"   -> model calls the bash/shell tool (approval path)
  contains "[slow]"        -> slow stream (for interrupt tests)
  otherwise                -> plain streamed answer with reasoning deltas

Usage: python3 -I mock_ollama.py --port 11434 --log requests.jsonl
"""

import argparse
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL = "qwen3:4b"
LOG_LOCK = threading.Lock()
ARGS = None


def log_request(path, body):
    if not ARGS.log:
        return
    with LOG_LOCK, open(ARGS.log, "a", encoding="utf-8") as fh:
        fh.write(json.dumps({"ts": time.time(), "path": path, "body": body}) + "\n")


def last_user_text(messages):
    for msg in reversed(messages or []):
        if msg.get("role") == "user":
            content = msg.get("content")
            if isinstance(content, str):
                return content
            if isinstance(content, list):
                return " ".join(
                    part.get("text", "") for part in content if isinstance(part, dict)
                )
    return ""


def tool_names(body):
    names = []
    for tool in body.get("tools") or []:
        fn = tool.get("function") or {}
        if fn.get("name"):
            names.append(fn["name"])
    return names


def pick_tool(available, preferences):
    for name in preferences:
        if name in available:
            return name
    return None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # quieter stderr
        sys.stderr.write("[mock-ollama] " + (fmt % args) + "\n")

    # ---- helpers -------------------------------------------------------
    def _json(self, status, payload):
        raw = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _body(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            return json.loads(raw or b"{}")
        except json.JSONDecodeError:
            return {"_raw": raw.decode("utf-8", "replace")}

    def _sse_chunk(self, payload):
        data = ("data: " + json.dumps(payload) + "\n\n").encode()
        self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
        self.wfile.flush()

    def _sse_done(self):
        data = b"data: [DONE]\n\n"
        self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    # ---- routes --------------------------------------------------------
    def do_GET(self):
        log_request("GET " + self.path, None)
        if self.path.startswith("/api/tags"):
            return self._json(200, {
                "models": [{
                    "name": MODEL, "model": MODEL, "size": 2_600_000_000,
                    "digest": "mock", "modified_at": "2026-10-13T00:00:00Z",
                    "details": {"family": "qwen3", "parameter_size": "4.0B",
                                "quantization_level": "Q4_K_M", "format": "gguf"},
                }]
            })
        if self.path.startswith("/api/version"):
            return self._json(200, {"version": "0.12.6-mock"})
        if self.path.startswith("/api/ps"):
            return self._json(200, {"models": []})
        if self.path.startswith("/v1/models"):
            return self._json(200, {"object": "list", "data": [
                {"id": MODEL, "object": "model", "created": 0, "owned_by": "library"}]})
        if self.path == "/":
            body = b"Ollama is running"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        return self._json(404, {"error": "not found"})

    def do_POST(self):
        body = self._body()
        log_request("POST " + self.path, body)
        if self.path.startswith("/api/show"):
            return self._json(200, {
                "capabilities": ["completion", "tools", "thinking"],
                "model_info": {"general.architecture": "qwen3", "qwen3.context_length": 40960},
                "details": {"family": "qwen3", "parameter_size": "4.0B"},
            })
        if self.path.startswith("/v1/chat/completions"):
            return self._chat(body)
        return self._json(404, {"error": "not found"})

    # ---- chat ----------------------------------------------------------
    def _chat(self, body):
        messages = body.get("messages") or []
        prompt = last_user_text(messages)
        last_role = messages[-1].get("role") if messages else None
        available = tool_names(body)
        stream = bool(body.get("stream"))

        tool_call = None
        text = "Привет! Это ответ тестовой модели (mock Ollama)."
        reasoning = "Пользователь поздоровался, отвечаю кратко."
        delay = 0.03
        if last_role == "tool":
            text = "Готово: инструмент выполнен, результат получен."
            reasoning = "Инструмент вернул результат, подвожу итог."
        elif "[tool:read]" in prompt:
            name = pick_tool(available, ["read", "read_file", "handle_read"])
            if name:
                tool_call = (name, {"path": "hello.txt"})
        elif "[tool:write]" in prompt:
            name = pick_tool(available, ["write", "write_file"])
            if name:
                tool_call = (name, {"path": "created_by_agent.txt", "content": "hello from mock\n"})
        elif "[tool:bash]" in prompt:
            name = pick_tool(available, ["bash", "exec_shell", "Run", "shell"])
            if name:
                tool_call = (name, {"command": "echo hi-from-bash"})
        if "[slow]" in prompt and last_role != "tool":
            text = " ".join(["медленно"] * 60)
            delay = 0.5

        created = int(time.time())
        cid = f"chatcmpl-mock-{created}"
        usage = {"prompt_tokens": 1200, "completion_tokens": 42, "total_tokens": 1242}

        if not stream:
            message = {"role": "assistant", "content": text if not tool_call else ""}
            finish = "stop"
            if tool_call:
                message["tool_calls"] = [{
                    "id": "call_mock_1", "type": "function",
                    "function": {"name": tool_call[0], "arguments": json.dumps(tool_call[1])},
                }]
                finish = "tool_calls"
            return self._json(200, {
                "id": cid, "object": "chat.completion", "created": created, "model": MODEL,
                "choices": [{"index": 0, "message": message, "finish_reason": finish}],
                "usage": usage,
            })

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        base = {"id": cid, "object": "chat.completion.chunk", "created": created, "model": MODEL}

        def chunk(delta, finish=None):
            payload = dict(base)
            payload["choices"] = [{"index": 0, "delta": delta, "finish_reason": finish}]
            self._sse_chunk(payload)

        try:
            chunk({"role": "assistant", "content": ""})
            for word in reasoning.split(" "):
                chunk({"reasoning": word + " "})  # Ollama >= 0.12 OpenAI-compat field
                time.sleep(delay)
            if tool_call:
                chunk({"tool_calls": [{
                    "index": 0, "id": "call_mock_1", "type": "function",
                    "function": {"name": tool_call[0], "arguments": ""},
                }]})
                args = json.dumps(tool_call[1])
                for i in range(0, len(args), 12):
                    chunk({"tool_calls": [{"index": 0, "function": {"arguments": args[i:i + 12]}}]})
                    time.sleep(delay)
                chunk({}, "tool_calls")
            else:
                for word in text.split(" "):
                    chunk({"content": word + " "})
                    time.sleep(delay)
                chunk({}, "stop")
            payload = dict(base)
            payload["choices"] = []
            payload["usage"] = usage
            self._sse_chunk(payload)
            self._sse_done()
        except (BrokenPipeError, ConnectionResetError):
            pass


def main():
    global ARGS
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=11434)
    parser.add_argument("--log", default="")
    ARGS = parser.parse_args()
    server = ThreadingHTTPServer((ARGS.host, ARGS.port), Handler)
    server.daemon_threads = True
    print(f"mock ollama on http://{ARGS.host}:{ARGS.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
