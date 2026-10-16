#!/usr/bin/env python3
"""Final knowledge-drive test: a real CodeWhale app-server with the ai-drive MCP server,
driven by the scripted mock Ollama, writes a water shader into a Godot project, checks it
with `godot_check`, and then Godot renders the scene to a PNG.

The only fake part is the model text (mock_ollama.py, scenario "[shader]"); CodeWhale, its
tool execution, the MCP transport, our MCP server and Godot are real.

Usage:
  python3 final_test_water.py --codewhale <codewhale binary> --desktop <codewhale-desktop binary>
      --godot <godot binary> --project <Godot/WaterDemo folder> --shader <water.gdshader>
      --out <folder for the report and the PNG> [--render]

Exit code 0 when the agent wrote the shader, godot_check passed and (with --render) the PNG
was saved; the folder gets report.json, events.jsonl, ollama_requests.jsonl and water.png.
"""

import argparse
import http.client
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

TOKEN = "final-test-token"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_http(port, path, timeout=60):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            c = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
            c.request("GET", path)
            if c.getresponse().status < 500:
                return True
        except OSError:
            pass
        time.sleep(0.2)
    return False


class Api:
    def __init__(self, port):
        self.port = port

    def call(self, method, path, body=None, timeout=60):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
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

    def events(self, thread_id, on_event, stop, since=0):
        """Streams SSE frames until `stop` is set; calls on_event(name, data)."""
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=600)
        c.request("GET", f"/v1/threads/{thread_id}/events?replay_limit=4096&progress=true&since_seq={since}",
                  headers={"Authorization": f"Bearer {TOKEN}", "Accept": "text/event-stream"})
        r = c.getresponse()
        name, data = None, []
        while not stop.is_set():
            line = r.fp.readline()
            if not line:
                break
            line = line.decode("utf-8", "replace").rstrip("\r\n")
            if line.startswith("event:"):
                name = line[6:].strip()
            elif line.startswith("data:"):
                data.append(line[5:].lstrip())
            elif line == "":
                if name and data:
                    try:
                        on_event(name, json.loads("\n".join(data)))
                    except json.JSONDecodeError:
                        pass
                name, data = None, []


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--codewhale", required=True)
    ap.add_argument("--desktop", required=True, help="codewhale-desktop binary (serves `mcp-drive`)")
    ap.add_argument("--godot", required=True)
    ap.add_argument("--project", required=True, help="Godot project folder (water.gdshader is written by the agent)")
    ap.add_argument("--shader", required=True, help="shader text the scripted model 'writes'")
    ap.add_argument("--check-project", required=True, help="folder with check.gd (src-tauri/resources/godot-check)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--render", action="store_true", help="render the project under xvfb-run after the turn")
    ap.add_argument("--timeout", type=int, default=240)
    args = ap.parse_args()

    here = Path(__file__).resolve().parent
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    report = {"ok": False, "steps": []}

    def step(name, ok, **info):
        report["steps"].append({"step": name, "ok": ok, **info})
        print(f"[{'ok' if ok else 'FAIL'}] {name} {json.dumps(info, ensure_ascii=False)[:300]}", flush=True)

    tmp = Path(tempfile.mkdtemp(prefix="cwfinal-"))
    workspace = tmp / "proj"
    shutil.copytree(args.project, workspace)
    (workspace / "water.gdshader").unlink(missing_ok=True)
    home = tmp / "home"
    cw_home = home / ".codewhale"
    cw_home.mkdir(parents=True)
    drive = tmp / "drive"
    (drive / "bin").mkdir(parents=True)
    helper = here.parent.parent.parent / "src-tauri" / "resources" / "drive" / "sympy_helper.py"
    if helper.is_file():
        shutil.copy(helper, drive / "bin" / "sympy_helper.py")

    # The same entry the desktop app writes (src-tauri/src/drive.rs::mcp_entry).
    mcp = {"servers": {"ai-drive": {
        "command": str(Path(args.desktop).resolve()),
        "args": ["mcp-drive"],
        "env": {
            "AI_DRIVE_PATH": str(drive),
            "AI_DRIVE_STATE_DIR": str(drive / ".agent_state"),
            "GODOT_BIN": str(Path(args.godot).resolve()),
            "GODOT_CHECK_PROJECT": str(Path(args.check_project).resolve()),
            "PYTHON_BIN": sys.executable,
            "SYMPY_HELPER": str(drive / "bin" / "sympy_helper.py"),
        },
        "connect_timeout": 30,
        "execute_timeout": 300,
    }}}
    (cw_home / "mcp.json").write_text(json.dumps(mcp, indent=2), encoding="utf-8")

    ollama_port = free_port()
    server_port = free_port()
    procs = []
    try:
        mock = subprocess.Popen(
            [sys.executable, "-I", str(here / "mock_ollama.py"), "--port", str(ollama_port), "--log", str(out / "ollama_requests.jsonl")],
            env={**os.environ, "MOCK_SHADER_FILE": str(Path(args.shader).resolve()), "MOCK_WORKSPACE": str(workspace)},
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        procs.append(mock)
        step("mock ollama", wait_http(ollama_port, "/api/version", 20), port=ollama_port)

        env = {
            **os.environ,
            "HOME": str(home),
            "CODEWHALE_HOME": str(cw_home),
            "CODEWHALE_RUNTIME_TOKEN": TOKEN,
            "CODEWHALE_TELEMETRY": "0",
            "NO_COLOR": "1",
            "OLLAMA_BASE_URL": f"http://127.0.0.1:{ollama_port}/v1",
        }
        server_log = open(out / "codewhale.log", "wb")
        server = subprocess.Popen(
            [args.codewhale, "app-server", "--http", "--host", "127.0.0.1", "--port", str(server_port)],
            cwd=workspace, env=env, stdout=server_log, stderr=subprocess.STDOUT,
        )
        procs.append(server)
        step("codewhale app-server", wait_http(server_port, "/health", 90), port=server_port)

        api = Api(server_port)
        st, info = api.call("GET", "/v1/runtime/info")
        step("runtime info", st == 200, version=(info or {}).get("codewhale_version") if isinstance(info, dict) else None)

        st, thread = api.call("POST", "/v1/threads", {"model_provider": "ollama", "model": "qwen3:4b", "workspace": str(workspace)})
        tid = thread.get("id") if isinstance(thread, dict) else None
        step("create thread", st in (200, 201) and bool(tid), thread_id=tid)
        if not tid:
            return finish(report, out, 1)

        events_log = open(out / "events.jsonl", "w", encoding="utf-8")
        seen = {"tools": [], "approvals": [], "final_text": "", "done": None, "mcp_tools": []}
        done = threading.Event()
        stop = threading.Event()

        def on_event(name, data):
            events_log.write(json.dumps({"event": name, "data": data}, ensure_ascii=False) + "\n")
            events_log.flush()
            payload = data.get("payload") or {}
            if name == "model.tools.snapshot":
                tools = payload.get("tools") or payload.get("names") or []
                names = [t.get("name") if isinstance(t, dict) else t for t in tools]
                seen["mcp_tools"] = sorted({n for n in names if isinstance(n, str) and n.startswith("mcp_ai-drive_")})
            if name == "approval.required":
                aid = payload.get("approval_id") or payload.get("id")
                seen["approvals"].append({"id": aid, "tool": payload.get("tool_name"), "summary": payload.get("summary")})
                if aid:
                    api.call("POST", f"/v1/approvals/{aid}", {"decision": "allow", "remember": False})
            if name == "item.completed":
                item = payload.get("item") or {}
                if item.get("kind") in ("tool_call", "command_execution", "file_change"):
                    seen["tools"].append({"kind": item.get("kind"), "summary": item.get("summary"), "status": item.get("status")})
                if item.get("kind") == "agent_message":
                    seen["final_text"] = item.get("detail") or item.get("summary") or ""
            if name in ("turn.completed", "turn.failed", "turn.interrupted", "turn.canceled"):
                seen["done"] = name
                done.set()

        t = threading.Thread(target=api.events, args=(tid, on_event, stop), daemon=True)
        t.start()
        time.sleep(0.5)

        prompt = "Напиши шейдер воды для Godot 4 с волнами Герстнера, проверь его в Godot и сообщи результат. [shader]"
        st, turn = api.call("POST", f"/v1/threads/{tid}/turns", {"prompt": prompt})
        step("post turn", st in (200, 201, 202), status=st)

        done.wait(args.timeout)
        stop.set()
        step("turn finished", seen["done"] == "turn.completed", ending=seen["done"], tools=seen["tools"], approvals=seen["approvals"],
             mcp_tools=seen["mcp_tools"])

        shader_path = workspace / "water.gdshader"
        step("shader written by agent", shader_path.is_file(), path=str(shader_path), bytes=shader_path.stat().st_size if shader_path.is_file() else 0)
        check_called = any("godot_check" in (x.get("summary") or "") for x in seen["tools"])
        step("godot_check called through MCP", check_called)
        step("agent reported the check", "прошла" in seen["final_text"] and "НЕ прошла" not in seen["final_text"], final_text=seen["final_text"][:300])

        # Independent verification with the tool itself.
        verify = subprocess.run(
            [args.desktop, "mcp-drive", "call", "godot_check", json.dumps({"path": str(shader_path)})],
            env={**os.environ, **mcp["servers"]["ai-drive"]["env"]}, capture_output=True, text=True, timeout=180,
        )
        try:
            verdict = json.loads(verify.stdout)
        except json.JSONDecodeError:
            verdict = {"ok": False, "raw": verify.stdout[-500:], "stderr": verify.stderr[-500:]}
        step("godot_check verdict", bool(verdict.get("ok")), uniforms=verdict.get("uniforms"), errors=verdict.get("errors"))

        if args.render:
            png = out / "water.png"
            cmd = ["xvfb-run", "-a", "-s", "-screen 0 1280x720x24", args.godot, "--rendering-driver", "opengl3", "--path", str(workspace),
                   "--resolution", "1280x720", "--quit-after", "120", "--", "--shot", str(png)]
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
            step("render under xvfb", png.is_file() and "SHOT_SAVED" in r.stdout, png=str(png), tail=r.stdout[-200:])

        report["ok"] = all(s["ok"] for s in report["steps"])
        return finish(report, out, 0 if report["ok"] else 1)
    finally:
        for p in procs:
            try:
                p.terminate()
                p.wait(10)
            except Exception:
                p.kill()


def finish(report, out, code):
    (out / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print("RESULT:", "PASS" if report["ok"] else "FAIL", "->", out / "report.json")
    return code


if __name__ == "__main__":
    sys.exit(main())
