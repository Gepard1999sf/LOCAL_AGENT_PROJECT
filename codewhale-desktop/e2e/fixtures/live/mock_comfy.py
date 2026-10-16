#!/usr/bin/env python3
"""Deterministic ComfyUI stand-in for testing the `comfy` MCP server plumbing.

Speaks the subset of ComfyUI's HTTP API that src-tauri/src/comfy_mcp.rs uses, with the
same shapes as the real server (ComfyUI v0.11.x server.py):
  GET  /system_stats, /queue, /object_info, /object_info/{class}, /history/{id}, /view
  POST /prompt (validates class_type like execution.validate_prompt's missing_node_type),
       /free, /interrupt, /upload/image (multipart)
Every queued prompt "renders" after --render-ms into a solid-colour PNG per SaveImage node
(batch_size images for EmptyLatentImage batches). Request bodies are logged as JSONL.
No websocket: the MCP tool polls /history, which is what this exercises.

Usage: python3 -I mock_comfy.py --port 8188 --log requests.jsonl [--render-ms 300]
"""

import argparse
import json
import re
import struct
import sys
import threading
import time
import uuid
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

ARGS = None
LOCK = threading.Lock()
QUEUE = []          # pending (prompt_id, prompt, queued_at)
HISTORY = {}        # prompt_id -> history entry
FILES = {}          # (type, subfolder, filename) -> bytes
FLAGS = {"free_calls": 0, "interrupts": 0, "uploads": 0}

# A vanilla subset of nodes with the real input shapes.
NODES = {
    "CheckpointLoaderSimple": {"input": {"required": {"ckpt_name": [["sdxl_base.safetensors", "dreamshaper_8.safetensors"], {}]}}, "output": ["MODEL", "CLIP", "VAE"], "output_name": ["MODEL", "CLIP", "VAE"], "category": "loaders", "display_name": "Load Checkpoint"},
    "CLIPTextEncode": {"input": {"required": {"text": ["STRING", {"multiline": True}], "clip": ["CLIP", {}]}}, "output": ["CONDITIONING"], "output_name": ["CONDITIONING"], "category": "conditioning", "display_name": "CLIP Text Encode (Prompt)"},
    "EmptyLatentImage": {"input": {"required": {"width": ["INT", {"default": 512, "min": 16, "max": 16384}], "height": ["INT", {"default": 512, "min": 16, "max": 16384}], "batch_size": ["INT", {"default": 1, "min": 1, "max": 4096}]}}, "output": ["LATENT"], "output_name": ["LATENT"], "category": "latent", "display_name": "Empty Latent Image"},
    "KSampler": {"input": {"required": {"model": ["MODEL", {}], "seed": ["INT", {"default": 0, "min": 0, "max": 18446744073709551615}], "steps": ["INT", {"default": 20, "min": 1, "max": 10000}], "cfg": ["FLOAT", {"default": 8.0, "min": 0.0, "max": 100.0}], "sampler_name": [["euler", "dpmpp_2m"], {}], "scheduler": [["normal", "karras"], {}], "positive": ["CONDITIONING", {}], "negative": ["CONDITIONING", {}], "latent_image": ["LATENT", {}], "denoise": ["FLOAT", {"default": 1.0, "min": 0.0, "max": 1.0}]}}, "output": ["LATENT"], "output_name": ["LATENT"], "category": "sampling", "display_name": "KSampler"},
    "VAEDecode": {"input": {"required": {"samples": ["LATENT", {}], "vae": ["VAE", {}]}}, "output": ["IMAGE"], "output_name": ["IMAGE"], "category": "latent", "display_name": "VAE Decode"},
    "SaveImage": {"input": {"required": {"images": ["IMAGE", {}], "filename_prefix": ["STRING", {"default": "ComfyUI"}]}}, "output": [], "output_name": [], "category": "image", "display_name": "Save Image", "output_node": True},
    "LoadImage": {"input": {"required": {"image": [[], {"image_upload": True}]}}, "output": ["IMAGE", "MASK"], "output_name": ["IMAGE", "MASK"], "category": "image", "display_name": "Load Image"},
}
for name, info in NODES.items():
    info.setdefault("name", name)
    info.setdefault("description", "")
    info.setdefault("output_node", False)
    info.setdefault("search_aliases", [])


def log_request(path, body):
    if not ARGS.log:
        return
    with LOCK, open(ARGS.log, "a", encoding="utf-8") as fh:
        fh.write(json.dumps({"ts": time.time(), "path": path, "body": body}) + "\n")


def png_solid(width, height, rgb):
    """Minimal RGB PNG without external libraries."""
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b"")


def validate(prompt):
    """Mirror of execution.validate_prompt for the parts the bridge relies on."""
    node_errors = {}
    if not isinstance(prompt, dict) or not prompt:
        return {"type": "prompt_no_outputs", "message": "Prompt has no outputs", "details": "", "extra_info": {}}, node_errors
    outputs = []
    for nid, node in prompt.items():
        ct = node.get("class_type") if isinstance(node, dict) else None
        if ct not in NODES:
            node_errors[nid] = {"errors": [{"type": "missing_node_type", "message": f"Node type '{ct}' is not installed", "details": "", "extra_info": {}}], "dependent_outputs": [], "class_type": ct}
            continue
        if NODES[ct].get("output_node"):
            outputs.append(nid)
        for key, spec in NODES[ct]["input"]["required"].items():
            if key not in node.get("inputs", {}):
                node_errors.setdefault(nid, {"errors": [], "dependent_outputs": [], "class_type": ct})["errors"].append(
                    {"type": "required_input_missing", "message": f"Required input is missing: {key}", "details": key, "extra_info": {"input_name": key}})
            else:
                val = node["inputs"][key]
                t = spec[0]
                if isinstance(val, list) and len(val) == 2:
                    if str(val[0]) not in prompt:
                        node_errors.setdefault(nid, {"errors": [], "dependent_outputs": [], "class_type": ct})["errors"].append(
                            {"type": "bad_linked_input", "message": "Bad linked input", "details": f"{key} -> {val}", "extra_info": {"input_name": key}})
                elif t == "INT" and not isinstance(val, int):
                    node_errors.setdefault(nid, {"errors": [], "dependent_outputs": [], "class_type": ct})["errors"].append(
                        {"type": "invalid_input_type", "message": f"Failed to convert an input value to a INT value", "details": f"{key}, {val!r}", "extra_info": {"input_name": key}})
    if node_errors:
        return {"type": "prompt_outputs_failed_validation", "message": "Prompt outputs failed validation", "details": "", "extra_info": {}}, node_errors
    if not outputs:
        return {"type": "prompt_no_outputs", "message": "Prompt has no outputs", "details": "", "extra_info": {}}, node_errors
    return None, node_errors


def render(prompt_id, prompt):
    """Produce one PNG per image of every SaveImage node (batch = EmptyLatentImage.batch_size)."""
    time.sleep(ARGS.render_ms / 1000.0)
    outputs = {}
    counter = 0
    for nid, node in prompt.items():
        if node.get("class_type") != "SaveImage":
            continue
        prefix = str(node["inputs"].get("filename_prefix", "ComfyUI"))
        subfolder, _, base = prefix.rpartition("/")
        # Walk back to the latent to learn width/height/batch.
        width, height, batch = 64, 64, 1
        stack = [node]
        seen = set()
        while stack:
            cur = stack.pop()
            for v in cur.get("inputs", {}).values():
                if isinstance(v, list) and len(v) == 2 and str(v[0]) in prompt and str(v[0]) not in seen:
                    seen.add(str(v[0]))
                    nxt = prompt[str(v[0])]
                    if nxt.get("class_type") == "EmptyLatentImage":
                        width = int(nxt["inputs"].get("width", 64))
                        height = int(nxt["inputs"].get("height", 64))
                        batch = int(nxt["inputs"].get("batch_size", 1))
                    stack.append(nxt)
        images = []
        for i in range(batch):
            counter += 1
            filename = f"{base}_{counter:05d}_.png"
            colour = ((37 * counter) % 256, (91 * counter) % 256, (173 * counter) % 256)
            # Cap the mock image at 128px so a 1024² request does not cost seconds.
            FILES[("output", subfolder, filename)] = png_solid(min(width, 128), min(height, 128), colour)
            images.append({"filename": filename, "subfolder": subfolder, "type": "output"})
        outputs[nid] = {"images": images}
    with LOCK:
        HISTORY[prompt_id] = {
            "prompt": [0, prompt_id, prompt, {}, list(outputs.keys())],
            "outputs": outputs,
            "status": {"status_str": "success", "completed": True, "messages": [["execution_start", {"prompt_id": prompt_id}], ["execution_success", {"prompt_id": prompt_id}]]},
            "meta": {},
        }
        QUEUE[:] = [q for q in QUEUE if q[0] != prompt_id]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("[mock-comfy] " + (fmt % args) + "\n")

    def _json(self, status, payload):
        raw = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _bytes(self, status, raw, ctype):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _body_raw(self):
        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length) if length else b""

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        log_request("GET " + self.path, None)
        if u.path == "/system_stats":
            return self._json(200, {"system": {"os": "mock", "ram_total": 34359738368, "ram_free": 20000000000, "comfyui_version": "0.11.86-mock", "python_version": "3.12", "pytorch_version": "2.7", "argv": ["main.py", "--listen", "127.0.0.1"]},
                                    "devices": [{"name": "cuda:0 NVIDIA GeForce RTX 5060 : cudaMallocAsync", "type": "cuda", "index": 0, "vram_total": 8589934592, "vram_free": 7000000000, "torch_vram_total": 0, "torch_vram_free": 0}]})
        if u.path == "/queue":
            with LOCK:
                return self._json(200, {"queue_running": [], "queue_pending": [[0, pid, prompt, {}, []] for pid, prompt, _ in QUEUE]})
        if u.path == "/object_info":
            return self._json(200, NODES)
        if u.path.startswith("/object_info/"):
            cls = u.path[len("/object_info/"):]
            return self._json(200, {cls: NODES[cls]} if cls in NODES else {})
        if u.path.startswith("/history/"):
            pid = u.path[len("/history/"):]
            with LOCK:
                return self._json(200, {pid: HISTORY[pid]} if pid in HISTORY else {})
        if u.path == "/history":
            with LOCK:
                return self._json(200, HISTORY)
        if u.path == "/view":
            key = (q.get("type", ["output"])[0], q.get("subfolder", [""])[0], q.get("filename", [""])[0])
            raw = FILES.get(key)
            if raw is None:
                return self._bytes(404, b"", "text/plain")
            return self._bytes(200, raw, "image/png")
        if u.path == "/api/ps":  # when the test points OLLAMA_URL here too
            return self._json(200, {"models": [{"name": "qwen2.5-coder:7b", "model": "qwen2.5-coder:7b", "size_vram": 5000000000}]})
        return self._json(404, {"error": "not found"})

    def do_POST(self):
        u = urlparse(self.path)
        raw = self._body_raw()
        if u.path == "/upload/image":
            FLAGS["uploads"] += 1
            m = re.search(rb'filename="([^"]+)"', raw)
            name = m.group(1).decode() if m else "upload.png"
            sub = re.search(rb'name="subfolder"\r\n\r\n([^\r]*)', raw)
            subfolder = sub.group(1).decode() if sub else ""
            FILES[("input", subfolder, name)] = raw
            log_request("POST /upload/image", {"name": name, "subfolder": subfolder, "bytes": len(raw)})
            return self._json(200, {"name": name, "subfolder": subfolder, "type": "input"})
        try:
            body = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            body = {"_raw": raw[:200].decode("utf-8", "replace")}
        log_request("POST " + u.path, body)
        if u.path == "/prompt":
            prompt = body.get("prompt")
            if prompt is None:
                return self._json(400, {"error": {"type": "no_prompt", "message": "No prompt provided", "details": "", "extra_info": {}}, "node_errors": {}})
            err, node_errors = validate(prompt)
            if err:
                return self._json(400, {"error": err, "node_errors": node_errors})
            pid = body.get("prompt_id") or str(uuid.uuid4())
            with LOCK:
                QUEUE.append((pid, prompt, time.time()))
            threading.Thread(target=render, args=(pid, prompt), daemon=True).start()
            return self._json(200, {"prompt_id": pid, "number": len(QUEUE), "node_errors": {}})
        if u.path == "/free":
            FLAGS["free_calls"] += 1
            FLAGS["last_free"] = body
            return self._bytes(200, b"", "text/plain")
        if u.path == "/interrupt":
            FLAGS["interrupts"] += 1
            return self._bytes(200, b"", "text/plain")
        if u.path == "/api/generate":  # Ollama unload stand-in
            log_request("POST /api/generate", body)
            return self._json(200, {"model": body.get("model"), "done": True, "done_reason": "unload"})
        if u.path == "/_mock/flags":
            return self._json(200, FLAGS)
        return self._json(404, {"error": "not found"})


def main():
    global ARGS
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8188)
    parser.add_argument("--log", default="")
    parser.add_argument("--render-ms", type=int, default=300)
    ARGS = parser.parse_args()
    server = ThreadingHTTPServer((ARGS.host, ARGS.port), Handler)
    server.daemon_threads = True
    print(f"mock comfyui on http://{ARGS.host}:{ARGS.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
