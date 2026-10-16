#!/usr/bin/env python3
"""Plumbing test for the `comfy` MCP server against the ComfyUI stand-in (mock_comfy.py).

Starts mock_comfy on a free port, points the server at it (COMFY_URL, OLLAMA_URL for the
Ollama unload stand-in, AI_WORKFLOWS_DIR with the bundled templates) and drives the tools
through `codewhale-desktop mcp-comfy call …` and through the stdio JSON-RPC loop:
status, templates, validation (including a template with a missing custom node), icon,
PBR material with .tres, sprite sheet assembly, upload, free/interrupt. Verifies the VRAM
handshake order in the mock's request log. Exit code 0 on success.

Usage: python3 final_test_comfy.py --desktop <codewhale-desktop binary> --templates <resources/comfy> --out <folder>
"""

import argparse
import http.client
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from pathlib import Path


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_http(port, path, timeout=30):
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


def png_solid(w, h, rgb):
    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--desktop", required=True)
    ap.add_argument("--templates", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    here = Path(__file__).resolve().parent
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    report = {"ok": False, "steps": []}

    def step(name, ok, **info):
        report["steps"].append({"step": name, "ok": ok, **info})
        print(f"[{'ok' if ok else 'FAIL'}] {name} {json.dumps(info, ensure_ascii=False)[:300]}", flush=True)

    tmp = Path(tempfile.mkdtemp(prefix="cwcomfy-"))
    workflows = tmp / "AI_Workflows"
    shutil.copytree(args.templates, workflows)
    # A template that needs a custom node the mock does not have.
    broken = json.loads((workflows / "ui_icon_api.json").read_text(encoding="utf-8"))
    broken["10"] = {"class_type": "RMBG", "inputs": {"image": ["8", 0]}}
    (workflows / "icon_rmbg_api.json").write_text(json.dumps(broken), encoding="utf-8")
    project = tmp / "game"
    (project / "assets" / "textures").mkdir(parents=True)
    (project / "project.godot").write_text("config_version=5\n", encoding="utf-8")

    port = free_port()
    log = out / "comfy_requests.jsonl"
    mock = subprocess.Popen([sys.executable, "-I", str(here / "mock_comfy.py"), "--port", str(port), "--log", str(log), "--render-ms", "200"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    env = {**os.environ, "COMFY_URL": f"http://127.0.0.1:{port}", "OLLAMA_URL": f"http://127.0.0.1:{port}", "AI_WORKFLOWS_DIR": str(workflows), "COMFY_CHECKPOINT": "sdxl_base.safetensors"}

    def call(tool, params):
        r = subprocess.run([args.desktop, "mcp-comfy", "call", tool, json.dumps(params)], env=env, capture_output=True, text=True, timeout=120)
        text = r.stdout.strip()
        try:
            return r.returncode, json.loads(text) if text else None, r.stderr.strip()
        except json.JSONDecodeError:
            return r.returncode, text, r.stderr.strip()

    try:
        step("mock comfyui", wait_http(port, "/system_stats", 20), port=port)

        code, st, err = call("comfy_status", {})
        step("comfy_status", code == 0 and st and st.get("online") is True and st.get("version", "").startswith("0.11"), online=(st or {}).get("online"), templates=(st or {}).get("templates"), err=err)

        code, tl, err = call("comfy_templates", {})
        names = [t["name"] for t in tl] if isinstance(tl, list) else []
        step("comfy_templates", code == 0 and {"ui_icon", "pbr_material", "sprite_sheet", "icon_rmbg"} <= set(names), names=names, err=err)

        code, v, err = call("comfy_template_validate", {"template": "ui_icon"})
        step("validate ui_icon", code == 0 and v and v.get("ok") is True and v.get("missingNodeTypes") == [], result=v, err=err)
        code, v, err = call("comfy_template_validate", {"template": "icon_rmbg"})
        step("validate icon_rmbg reports missing RMBG", code == 0 and v and v.get("ok") is False and v.get("missingNodeTypes") == ["RMBG"], missing=(v or {}).get("missingNodeTypes"), err=err)

        code, nodes, err = call("comfy_nodes_search", {"query": "ksampler"})
        step("comfy_nodes_search", code == 0 and isinstance(nodes, list) and nodes and nodes[0]["name"] == "KSampler" and "seed" in nodes[0]["required"], first=(nodes or [{}])[0].get("name") if isinstance(nodes, list) else nodes, err=err)

        icon = project / "assets" / "ui" / "icons" / "sword.png"
        code, r, err = call("generate_ui_icon", {"prompt": "iron sword", "save_to": str(icon), "size": 256, "seed": 7})
        step("generate_ui_icon", code == 0 and icon.is_file() and r and r.get("seed") == 7 and r.get("freedVram") is True and r.get("unloadedLlm") == ["qwen2.5-coder:7b"], files=(r or {}).get("files"), unloaded=(r or {}).get("unloadedLlm"), err=err)

        base = project / "assets" / "textures" / "stone"
        code, r, err = call("generate_pbr_material", {"prompt": "mossy cobblestone", "save_to": str(base), "resolution": 512})
        tres = base.with_suffix(".tres")
        tres_text = tres.read_text(encoding="utf-8") if tres.is_file() else ""
        step("generate_pbr_material + tres", code == 0 and (project / "assets/textures/stone_albedo.png").is_file() and 'path="res://assets/textures/stone_albedo.png"' in tres_text and r.get("resPath") == "res://assets/textures/stone.tres", tres=str(tres), maps=(r or {}).get("files"), err=err)

        sheet = project / "assets" / "sprites" / "slime.png"
        code, r, err = call("generate_sprite_sheet", {"prompt": "green slime walking", "save_to": str(sheet), "frames": 6, "columns": 4, "frame_size": 32})
        ok_sheet = code == 0 and sheet.is_file() and r and r.get("hframes") == 4 and r.get("vframes") == 2 and r.get("frames") == 6
        if ok_sheet:
            w, h = struct.unpack(">II", sheet.read_bytes()[16:24])
            ok_sheet = (w, h) == (128, 64)
        step("generate_sprite_sheet", ok_sheet, result={k: (r or {}).get(k) for k in ("hframes", "vframes", "frames", "file")}, err=err)

        code, r, err = call("generate_3d_prop", {"prompt": "barrel", "save_to": str(project / "props" / "barrel.glb")})
        step("generate_3d_prop without template fails clearly", code == 1 and "prop3d" in err, err=err[:160])

        code, r, err = call("comfy_generate", {"template": "ui_icon", "params": {"POSITIVE_PROMPT": "x"}, "save_to": str(tmp / "generic.png")})
        step("comfy_generate generic", code == 0 and (tmp / "generic.png").is_file(), err=err)

        code, r, err = call("comfy_generate", {"template": "ui_icon", "params": {}})
        step("missing prompt placeholder is an error", code == 1 and "POSITIVE_PROMPT" in err, err=err[:160])

        ref = tmp / "ref.png"
        ref.write_bytes(png_solid(8, 8, (1, 2, 3)))
        code, r, err = call("comfy_upload_image", {"path": str(ref), "subfolder": "refs"})
        step("comfy_upload_image", code == 0 and r and r.get("name") == "ref.png" and r.get("subfolder") == "refs", result=r, err=err)

        code, r, err = call("comfy_free", {})
        step("comfy_free", code == 0, err=err)
        code, r, err = call("comfy_interrupt", {})
        step("comfy_interrupt", code == 0, err=err)

        # Handshake order from the mock's log: unload (POST /api/generate) → /prompt → /free.
        paths = [json.loads(l)["path"] for l in log.read_text(encoding="utf-8").splitlines()]
        first_prompt = paths.index("POST /prompt") if "POST /prompt" in paths else -1
        first_unload = paths.index("POST /api/generate") if "POST /api/generate" in paths else -1
        first_free = paths.index("POST /free") if "POST /free" in paths else -1
        step("VRAM handshake order", 0 <= first_unload < first_prompt < first_free, unload=first_unload, prompt=first_prompt, free=first_free)

        # The stdio loop itself.
        msgs = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "comfy_status", "arguments": {}}},
        ]
        p = subprocess.run([args.desktop, "mcp-comfy"], env=env, input="".join(json.dumps(m) + "\n" for m in msgs), capture_output=True, text=True, timeout=60)
        replies = [json.loads(l) for l in p.stdout.splitlines() if l.strip()]
        tools = replies[1]["result"]["tools"] if len(replies) > 1 else []
        step("stdio initialize/tools/list/call", len(replies) == 3 and replies[0]["result"]["serverInfo"]["name"] == "comfy" and any(t["name"] == "generate_pbr_material" for t in tools) and '"online": true' in replies[2]["result"]["content"][0]["text"], replies=len(replies), tools=len(tools))

        report["ok"] = all(s["ok"] for s in report["steps"])
    finally:
        mock.terminate()
        try:
            mock.wait(5)
        except Exception:
            mock.kill()
    (out / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print("RESULT:", "PASS" if report["ok"] else "FAIL", "->", out / "report.json")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
