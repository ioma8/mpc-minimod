"""Read the running MPC Sample process memory from the emulator host.

The lab VM has no debugger and BusyBox's ``od`` has no ``-j/-N/-A/-t``, so the
only portable way in is ``/proc/<pid>/mem`` through ``dd`` + ``od -x`` over the
lab's root ssh.  That is what this module wraps; it is how the Sample page was
mapped in docs/TRIM-PAGE.md.

    cd tools/emulator
    python3 guestmem.py read 0xffffe50bc620 0x40
    python3 guestmem.py dump 0xffffe50bc620 0x9000 > /tmp/screen.bin
    python3 guestmem.py f32  0xffffe50bc620+0x1f6c

Addresses are *runtime* addresses of the guest process (PIE: app base + file
offset; the base moves between runs — read it from ``/proc/<pid>/maps``).

Set ``MPCLAB_PORT_OFFSET=100`` (or whatever the lab uses) before running, so the
ssh port matches the VM.
"""
from __future__ import annotations

import os
import re
import struct
import subprocess
import sys
from pathlib import Path

LAB = Path(os.environ.get("MPCLAB_LAB", "~/projects/customs/virtual-mpc-sample/lab.py")).expanduser()
ENV = {**os.environ}
_PID: int | None = None


def _ssh(command: str) -> str:
    result = subprocess.run(
        [sys.executable, str(LAB), "ssh", command], capture_output=True, text=True, env=ENV
    )
    if result.returncode != 0 and not result.stdout:
        raise RuntimeError(result.stderr[-400:])
    return result.stdout


def pid() -> int:
    """Guest pid of the stock app."""
    global _PID
    if _PID is None:
        _PID = int(_ssh('pidof "MPC Sample"').split()[0])
    return _PID


def maps() -> list[tuple[int, int, str, str]]:
    """(start, end, perms, path) for every mapping of the app."""
    out = []
    for line in _ssh(f"cat /proc/{pid()}/maps").splitlines():
        hit = re.match(r"([0-9a-f]+)-([0-9a-f]+) (\S+) \S+ \S+ \S+\s*(.*)", line)
        if hit:
            out.append((int(hit.group(1), 16), int(hit.group(2), 16), hit.group(3), hit.group(4)))
    return out


def base() -> int:
    """Load base of the app (first executable mapping of the binary)."""
    for start, _end, perms, path in maps():
        if perms.startswith("r-x") and path.endswith("MPC Sample"):
            return start
    raise RuntimeError("no executable mapping of 'MPC Sample'")


def _decode_od(text: str) -> bytes:
    words = []
    for line in text.splitlines():
        parts = line.split()
        if not parts or not parts[0].isdigit():
            continue
        words += [int(w, 16) for w in parts[1:] if w and len(w) <= 4]
    return b"".join(w.to_bytes(2, "little") for w in words)


def read(addr: int, size: int) -> bytes:
    """Read *size* bytes at *addr*, page-block by page-block."""
    out = []
    while size > 0:
        page, offset = divmod(addr, 4096)
        take = min(size, 4096 - offset)
        raw = _ssh(f"dd if=/proc/{pid()}/mem bs=4096 skip={page} count=1 2>/dev/null | od -x")
        out.append(_decode_od(raw)[offset : offset + take])
        addr += take
        size -= take
    return b"".join(out)


def write(addr: int, data: bytes) -> None:
    """Write *data* at *addr* through /proc/<pid>/mem (the app's own process)."""
    esc = "".join(f"\\x{b:02x}" for b in data)
    # BusyBox dd has no conv=notrunc and reports "File too large" for /proc/mem,
    # but the bytes do land; verify by reading them back.
    try:
        _ssh(f"printf '{esc}' | dd of=/proc/{pid()}/mem bs=1 seek={addr} count={len(data)} 2>/dev/null")
    except RuntimeError:
        pass
    got = read(addr, len(data))
    if got != data:
        raise RuntimeError(f"write verification failed at {addr:#x}: {got.hex()} != {data.hex()}")


def u64(addr: int) -> int:
    return struct.unpack("<Q", read(addr, 8))[0]


def u32(addr: int) -> int:
    return struct.unpack("<I", read(addr, 4))[0]


def f32(addr: int) -> float:
    return struct.unpack("<f", read(addr, 4))[0]


def _target(arg: str) -> int:
    """``0xffffe50bc620`` or ``0xffffe50bc620+0x1f6c``."""
    if "+" in arg:
        head, tail = arg.split("+", 1)
        return int(head, 16) + int(tail, 16)
    return int(arg, 16)


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    cmd = argv[1]
    if cmd == "maps":
        for start, end, perms, path in maps():
            print(f"{start:#x}-{end:#x} {perms} {path}")
        return 0
    if cmd == "read":
        sys.stdout.buffer.write(read(_target(argv[2]), int(argv[3], 0)))
        return 0
    if cmd == "dump":
        sys.stdout.buffer.write(read(_target(argv[2]), int(argv[3], 0)))
        return 0
    if cmd in ("u64", "u32", "f32"):
        fn = {"u64": u64, "u32": u32, "f32": f32}[cmd]
        print(fn(_target(argv[2])))
        return 0
    print(f"unknown command {cmd!r}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
