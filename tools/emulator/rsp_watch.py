#!/usr/bin/env python3
"""Set a write watchpoint on a guest virtual address through QEMU's gdbstub.

Needs the stub running in the guest VM:

    printf '{"execute":"qmp_capabilities"}\\n{"execute":"human-monitor-command",
            "arguments":{"command-line":"gdbserver tcp::5658"}}\\n' | nc 127.0.0.1 5654

Usage: rsp_watch.py <addr-hex> [seconds]
Prints the PC (and LR) of every write to that address.
"""
import socket
import sys
import time


class RSP:
    def __init__(self, host="127.0.0.1", port=5658, timeout=30):
        self.s = socket.create_connection((host, port), timeout=timeout)
        self.s.settimeout(timeout)
        self.buf = b""

    def _recv_packet(self):
        while True:
            if self.buf.startswith(b"+"):
                self.buf = self.buf[1:]
                continue
            if self.buf.startswith(b"$"):
                end = self.buf.find(b"#")
                if end < 0 or len(self.buf) < end + 3:
                    self.buf += self.s.recv(4096)
                    continue
                payload = self.buf[1:end]
                self.buf = self.buf[end + 3:]
                self.s.sendall(b"+")
                return payload.decode("latin-1")
            try:
                chunk = self.s.recv(4096)
            except socket.timeout:
                return None
            if not chunk:
                return None
            self.buf += chunk

    def cmd(self, payload):
        data = payload.encode()
        csum = sum(data) & 0xFF
        self.s.sendall(b"$" + data + b"#" + ("%02x" % csum).encode())
        return self._recv_packet()

    def close(self):
        try:
            self.cmd("D")
        except Exception:
            pass
        self.s.close()


def regs(rsp):
    g = rsp.cmd("g")
    if not g:
        return None
    raw = bytes.fromhex(g)
    if len(raw) < 34 * 8:
        return None
    vals = [int.from_bytes(raw[i * 8:(i + 1) * 8], "little") for i in range(34)]
    return {"x0": vals[0], "x1": vals[1], "x29": vals[29], "x30": vals[30],
            "sp": vals[31], "pc": vals[32]}


def main():
    if len(sys.argv) < 2:
        print("usage: rsp_watch.py <addr-hex> [seconds]")
        return 2
    addr = int(sys.argv[1], 16)
    seconds = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    rsp = RSP()
    first = rsp._recv_packet()  # the stub reports an initial stop on connect
    print("initial:", first)
    print("stub:", rsp.cmd("qSupported:multiprocess+;swbreak+;hwbreak+"))
    reply = rsp.cmd("Z2,%x,4" % addr)
    print("Z2 =>", reply)
    if reply != "OK":
        rsp.close()
        return 1
    deadline = time.time() + seconds
    hits = 0
    while time.time() < deadline:
        r = rsp.cmd("c")
        if r is None:
            print("no stop reply")
            break
        if not r.startswith("T"):
            print("reply:", r)
            break
        reg = regs(rsp)
        if reg:
            print("watch hit pc=0x%x lr=0x%x x0=0x%x" %
                  (reg["pc"], reg["x30"], reg["x0"]))
            hits += 1
        # keep watching
    print("hits:", hits)
    rsp.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
