#!/usr/bin/env python3
"""Lockstep RSP client: set Z2 write watchpoint, continue, report hits.

Usage: rsp_watch2.py <addr-hex> [seconds]
"""
import socket
import sys
import time


class Stub:
    def __init__(self, host="127.0.0.1", port=5658):
        self.s = socket.create_connection((host, port), timeout=10)
        self.s.settimeout(10)
        self.buf = b""

    def _fill(self, deadline):
        while time.time() < deadline:
            try:
                chunk = self.s.recv(65536)
            except socket.timeout:
                return False
            if not chunk:
                return False
            self.buf += chunk
            return True
        return False

    def _next(self, deadline):
        while time.time() < deadline:
            # strip acks
            while self.buf[:1] in (b"+", b"-"):
                self.buf = self.buf[1:]
            if self.buf[:1] == b"$":
                end = self.buf.find(b"#")
                if end > 0 and len(self.buf) >= end + 3:
                    pkt = self.buf[1:end].decode("latin-1")
                    self.buf = self.buf[end + 3:]
                    self.s.sendall(b"+")
                    return pkt
            if not self._fill(min(deadline, time.time() + 0.5)):
                pass
        return None

    def cmd(self, payload, timeout=5):
        data = payload.encode()
        self.s.sendall(b"$" + data + b"#" + ("%02x" % (sum(data) & 0xFF)).encode())
        return self._next(time.time() + timeout)

    def poke(self):
        self._next(time.time() + 0.3)


def regs(stub):
    g = stub.cmd("g", 5)
    if not g:
        return None
    try:
        raw = bytes.fromhex(g)
    except ValueError:
        return None
    if len(raw) < 34 * 8:
        return None
    v = [int.from_bytes(raw[i * 8:(i + 1) * 8], "little") for i in range(34)]
    return {"pc": v[32], "lr": v[30], "x0": v[0], "x1": v[1], "sp": v[31]}


def main():
    addr = int(sys.argv[1], 16)
    seconds = int(sys.argv[2]) if len(sys.argv) > 2 else 25
    st = Stub()
    st.poke()
    print("qSupported:", st.cmd("qSupported:multiprocess+;swbreak+;hwbreak+"))
    print("Z2:", st.cmd("Z2,%x,4" % addr))
    hits = 0
    deadline = time.time() + seconds
    while time.time() < deadline:
        st.s.sendall(b"$c#63")
        pkt = st._next(deadline)
        if pkt is None:
            print("no stop reply")
            break
        print("STOP", pkt)
        if pkt[:1] in ("T", "S"):
            r = regs(st)
            if r:
                print("   pc=0x%x lr=0x%x x0=0x%x x1=0x%x" %
                      (r["pc"], r["lr"], r["x0"], r["x1"]))
                hits += 1
    print("hits:", hits)
    st.cmd("D", 2)


if __name__ == "__main__":
    main()
