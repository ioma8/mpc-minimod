#!/usr/bin/env python3
"""Drive the emulated MPC through the QEMU monitor/QMP sockets.

Two jobs:

* `shot`  -- capture the virtio-gpu scanout (the app's framebuffer)
* `tap`   -- inject an absolute pointer press/release, so the UI can be driven
             without a physical touch panel

Absolute pointer coordinates are sent in QEMU's 0..0x7fff range but are given
here in framebuffer pixels, because that is what we measure from screenshots.

    qmp.py --dir . shot out.ppm
    qmp.py --dir . tap 160 25
"""

from __future__ import annotations

import argparse
import json
import socket
import time
from pathlib import Path

ABS_MAX = 0x7FFF


class QMP:
    def __init__(self, path: str, timeout: float = 5.0) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(timeout)
        for _ in range(int(timeout / 0.2)):
            try:
                self.sock.connect(path)
                break
            except OSError:
                time.sleep(0.2)
        else:
            raise SystemExit(f"cannot connect to {path}")
        greeting = self._read()  # {"QMP": {...}}
        if "QMP" not in greeting:
            raise SystemExit(f"not a QMP socket: {greeting!r}")
        self.cmd("qmp_capabilities")

    def _read(self) -> dict:
        buf = b""
        while b"\n" not in buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                break
            buf += chunk
        return json.loads(buf.decode().strip() or "{}")

    def cmd(self, name: str, **args) -> None:
        msg = {"execute": name}
        if args:
            msg["arguments"] = args
        self.sock.sendall((json.dumps(msg) + "\n").encode())
        try:
            self._read()
        except (json.JSONDecodeError, socket.timeout):
            pass

    def close(self) -> None:
        self.sock.close()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", type=Path, required=True)
    ap.add_argument("--width", type=int, default=320)
    ap.add_argument("--height", type=int, default=240)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("shot")
    s.add_argument("path", type=Path)
    t = sub.add_parser("tap")
    t.add_argument("x", type=int)
    t.add_argument("y", type=int)
    t.add_argument("--hold-ms", type=int, default=80)
    a = ap.parse_args()

    # Both screendump and input-send-event exist in QMP, so one socket does it all.
    mon = QMP(str(a.dir / "qmp.sock"))
    if a.cmd == "shot":
        mon.cmd("screendump", filename=str(a.path))
        time.sleep(1.2)
    else:
        ax = int(a.x / a.width * ABS_MAX)
        ay = int(a.y / a.height * ABS_MAX)
        for ev in (
            {"type": "abs", "data": {"axis": "x", "value": ax}},
            {"type": "abs", "data": {"axis": "y", "value": ay}},
        ):
            mon.cmd("input-send-event", events=[ev])
        mon.cmd("input-send-event", events=[
            {"type": "btn", "data": {"down": True, "button": "left"}}])
        time.sleep(a.hold_ms / 1000)
        mon.cmd("input-send-event", events=[
            {"type": "btn", "data": {"down": False, "button": "left"}}])
    mon.close()
    print("ok")


if __name__ == "__main__":
    main()
