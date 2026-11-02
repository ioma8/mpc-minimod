#!/usr/bin/env python3
"""Serve the emulated panel to a browser at a chosen zoom.

QEMU's cocoa display cannot upscale (`zoom-to-fit` only shrinks) and the app
always paints a fixed 320x240 canvas, so no layer in the guest or in QEMU will
magnify it. A browser will: it scales the framebuffer with
`image-rendering: pixelated`, which is a crisp nearest-neighbour integer zoom.

Frames come from QEMU's monitor (`screendump`), converted to PNG here. That is
fine for this panel: the app is a static UI, not video.

    panel.py --port 8090 --zoom 2 --width 320 --height 240
"""

from __future__ import annotations

import argparse
import io
import json
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from PIL import Image

PAGE = """<!doctype html>
<meta charset="utf-8"><title>MPC Sample</title>
<style>
  html,body {{ margin:0; background:#111; height:100%; }}
  body {{ display:grid; place-items:center; }}
  img {{ image-rendering: pixelated; width:{w}px; height:{h}px;
         -ms-interpolation-mode: nearest-neighbor; }}
</style>
<img id="p" width="{w}" height="{h}" src="/frame.png">
<script>
  const p = document.getElementById('p');
  setInterval(() => {{ p.src = '/frame.png?t=' + Date.now(); }}, {interval});
</script>
"""


class FrameSource:
    """Pulls the scanout through QMP and keeps the latest PNG in memory."""

    def __init__(self, qmp_path: Path) -> None:
        self.qmp_path = qmp_path
        self.png = b""
        self.lock = threading.Lock()
        self.tmp = Path(tempfile.mkdtemp(prefix="mpc-panel-")) / "frame.ppm"

    def _qmp(self, sock: socket.socket, name: str, **args) -> None:
        msg = {"execute": name}
        if args:
            msg["arguments"] = args
        sock.sendall((json.dumps(msg) + "\n").encode())
        sock.recv(65536)

    def grab(self) -> bytes:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(4)
        s.connect(str(self.qmp_path))
        s.recv(65536)                       # greeting
        try:
            self._qmp(s, "qmp_capabilities")
            self._qmp(s, "screendump", filename=str(self.tmp))
        finally:
            s.close()
        for _ in range(40):
            try:
                if self.tmp.exists() and self.tmp.stat().st_size:
                    break
            except OSError:
                pass
            time.sleep(0.02)
        img = Image.open(self.tmp)
        buf = io.BytesIO()
        img.convert("RGB").save(buf, format="PNG")
        return buf.getvalue()

    def run(self) -> None:
        while True:
            try:
                data = self.grab()
                with self.lock:
                    self.png = data
            except Exception:                # noqa: BLE001 - keep serving last frame
                pass
            time.sleep(0.12)

    def latest(self) -> bytes:
        with self.lock:
            return self.png


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", type=Path, required=True, help="dir holding qmp.sock")
    ap.add_argument("--port", type=int, default=8090)
    ap.add_argument("--zoom", type=int, default=2)
    ap.add_argument("--width", type=int, default=320)
    ap.add_argument("--height", type=int, default=240)
    ap.add_argument("--interval", type=int, default=150)
    a = ap.parse_args()

    src = FrameSource(a.dir / "qmp.sock")
    threading.Thread(target=src.run, daemon=True).start()

    page = PAGE.format(
        w=a.width * a.zoom, h=a.height * a.zoom, interval=a.interval
    ).encode()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args) -> None:   # quiet
            pass

        def do_GET(self) -> None:
            if self.path.startswith("/frame.png"):
                body, ctype = src.latest(), "image/png"
                if not body:
                    self.send_error(503)
                    return
            else:
                body, ctype = page, "text/html; charset=utf-8"
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

    srv = ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    print(f"panel: http://127.0.0.1:{a.port}/  ({a.width}x{a.height} @{a.zoom}x)", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
