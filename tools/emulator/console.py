#!/usr/bin/env python3
"""Run commands inside the emulated MPC over its serial console.

The guest autologins root on `ttyAMA0`, so a QEMU `-serial pty` gives us a real
interactive shell without needing sshd, host keys or a working guest network --
useful because the vendor rootfs keeps /var/lib on tmpfs and therefore
regenerates ssh host keys on every boot.

    console.py --wait-prompt 120 -- "id" "ls /dev/dri"

Each argument is one shell line. Output is streamed to stdout as it arrives.
"""

from __future__ import annotations

import argparse
import os
import socket
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

class Console:
    """Bidirectional serial console over a unix socket chardev.

    A pty chardev cannot be used for input: QEMU holds the master side, so
    writing to the slave path emulates the *guest* writing, not the host typing.
    A unix socket has no such direction problem.
    """

    def __init__(self, proc: subprocess.Popen, sock_path: str) -> None:
        self.proc = proc
        self.sock_path = sock_path
        for _ in range(50):
            try:
                self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.sock.connect(sock_path)
                break
            except OSError:
                time.sleep(0.2)
        else:
            raise SystemExit(f"could not connect to serial socket {sock_path}")
        self.buf = bytearray()
        self.lock = threading.Lock()
        self.thread = threading.Thread(target=self._reader, daemon=True)
        self.thread.start()

    def _reader(self) -> None:
        while True:
            try:
                data = self.sock.recv(4096)
            except OSError:
                return
            if not data:
                return
            with self.lock:
                self.buf += data
            sys.stdout.buffer.write(data)
            sys.stdout.buffer.flush()

    def text(self) -> str:
        with self.lock:
            return self.buf.decode("utf-8", "replace")

    def clear(self) -> None:
        with self.lock:
            self.buf.clear()

    def wait(self, pattern: str, timeout: float) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if re.search(pattern, self.text()):
                return True
            if self.proc.poll() is not None:
                return False
            time.sleep(0.2)
        return False

    def send(self, line: str) -> None:
        self.sock.sendall((line + "\n").encode())

    def screenshot(self, path: Path) -> None:
        """Capture the virtio-gpu scanout through the QEMU monitor."""
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(50):
            try:
                s.connect(self.monitor)
                break
            except OSError:
                time.sleep(0.2)
        time.sleep(0.3)
        try:
            s.recv(65536)
        except OSError:
            pass
        s.sendall(f"screendump {path}\n".encode())
        time.sleep(1.5)
        s.close()

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--qemu", default="/opt/homebrew/bin/qemu-system-aarch64")
    ap.add_argument("--dir", type=Path, required=True, help="directory with kernel/initramfs/dtb/rootfs")
    ap.add_argument("--wait-prompt", type=float, default=120.0)
    ap.add_argument("--timeout", type=float, default=60.0, help="per-command timeout")
    ap.add_argument("--display", default="none")
    ap.add_argument("--xres", default="320")
    ap.add_argument("--yres", default="240")
    ap.add_argument("--mem", default="4096")
    ap.add_argument("--cpus", default="4")
    ap.add_argument("--audio", default="coreaudio")
    ap.add_argument("--no-net", action="store_true")
    ap.add_argument("--keep", action="store_true", help="leave the VM running afterwards")
    ap.add_argument("--screenshot", type=Path, help="capture the framebuffer to this PPM path")
    ap.add_argument("cmds", nargs="*", help="shell lines to run, in order")
    a = ap.parse_args()

    d = a.dir
    kernel = next(iter(sorted(d.glob("k/vmlinuz*"))))
    cmd = [
        a.qemu, "-M", "virt", "-accel", "hvf", "-cpu", "host",
        "-smp", a.cpus, "-m", a.mem,
        "-kernel", str(kernel), "-initrd", str(d / "initramfs.cpio.gz"),
        "-dtb", str(d / "ac50.dtb"), "-append", "console=ttyAMA0",
        "-drive", f"file={d/'rootfs.img'},format=raw,if=virtio,cache=unsafe",
        "-device", f"virtio-gpu-pci,xres={a.xres},yres={a.yres}",
        "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci",
    ]
    if a.audio != "none":
        cmd += ["-audiodev", f"{a.audio},id=aud0",
                "-device", "virtio-sound-pci,audiodev=aud0,streams=2"]
    if not a.no_net:
        cmd += ["-netdev", "user,id=net0,hostfwd=tcp::2222-:22,hostfwd=tcp::7100-:7000,hostfwd=tcp::8080-:80",
                "-device", "virtio-net-pci,netdev=net0"]
    sock = d / "serial.sock"
    mon = d / "monitor.sock"
    for f in (sock, mon):
        if f.exists():
            f.unlink()
    qmp = d / "qmp.sock"
    if qmp.exists():
        qmp.unlink()
    cmd += ["-display", a.display, "-serial", f"unix:{sock},server=on,wait=off",
            "-monitor", f"unix:{mon},server=on,wait=off",
            "-qmp", f"unix:{qmp},server=on,wait=off"]

    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    con = Console(proc, str(sock))
    con.monitor = str(mon)
    try:
        if not con.wait(r"login:|# ", a.wait_prompt):
            raise SystemExit("guest did not reach a shell prompt")
        # agetty keeps the line discipline busy for a moment after autologin;
        # anything sent too early is silently dropped.
        time.sleep(2.5)
        con.send("")
        time.sleep(0.5)
        con.clear()
        for i, c in enumerate(a.cmds):
            # A bare prompt match is racy: the previous prompt may still be in
            # the buffer. Append a unique marker and wait for that instead.
            marker = f"__EMU_DONE_{i}__"
            con.clear()
            con.send(f"{c} ; echo {marker}")
            if not con.wait(marker, a.timeout):
                print(f"\n[console] command {i} timed out: {c}", flush=True)
            time.sleep(0.2)
        if a.screenshot:
            con.screenshot(a.screenshot)
        if not a.keep:
            con.clear()
            con.send("poweroff -f")
            con.wait(r"Power down|reboot", 20)
    finally:
        if not a.keep:
            con.close()
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    main()
