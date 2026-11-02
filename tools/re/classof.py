"""Name the classes behind live vtable pointers of the MPC Sample process.

The binary keeps C++ RTTI, so a vtable address point V resolves offline:

    u64(V - 8)  -> type_info object
    u64(ti + 8) -> NUL-terminated mangled name

This is the reverse direction of :mod:`rtti` (which goes name -> vtable) and is
what you need while staring at a heap dump or a live ``vptr``.

Usage:
    python3 tools/re/classof.py BINARY 0x706e5d0 0x6e212a0 ...
    python3 tools/re/classof.py BINARY --scan DUMP.bin --base 0xaaaaaaaa0000

``--scan`` walks a raw memory dump for qwords that resolve to a vtable and
prints ``offset -> class``; that is how the Sample page's embedded widgets were
enumerated (see docs/TRIM-PAGE.md).
"""
from __future__ import annotations

import re
import struct
import sys
from pathlib import Path

from fwxref import Segment, segments, to_offset


_NAME = re.compile(r"[A-Za-z0-9_$.]{3,200}\Z")


def class_name(path: Path, segs: list[Segment], vtable: int) -> str | None:
    """Return the RTTI name for a vtable address point, or None."""
    off = to_offset(segs, vtable - 8)
    if off is None:
        return None
    with path.open("rb") as fh:
        fh.seek(off)
        (ti,) = struct.unpack("<Q", fh.read(8))
    off = to_offset(segs, ti + 8)
    if off is None:
        return None
    with path.open("rb") as fh:
        fh.seek(off)
        (name_va,) = struct.unpack("<Q", fh.read(8))
    off = to_offset(segs, name_va)
    if off is None:
        return None
    with path.open("rb") as fh:
        fh.seek(off)
        raw = fh.read(256)
    name = raw.split(b"\0", 1)[0].decode("latin-1")
    # A mangled name is ASCII; anything else means this was not a vtable slot.
    return name if _NAME.match(name) else None


def scan_dump(path: Path, segs: list[Segment], dump: bytes, base: int) -> list[tuple[int, str]]:
    """Every 8-byte slot of *dump* whose value names a class at *base*."""
    out = []
    for off in range(0, len(dump) - 8, 8):
        (v,) = struct.unpack_from("<Q", dump, off)
        if v < base:
            continue
        name = class_name(path, segs, v - base)
        if name:
            out.append((off, name))
    return out


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    path = Path(argv[1])
    segs = segments(path)
    if argv[2] == "--scan":
        dump = Path(argv[3]).read_bytes()
        base = int(argv[5], 16) if len(argv) > 5 and argv[4] == "--base" else 0xAAAAAAAA0000
        seen: dict[str, int] = {}
        for off, name in scan_dump(path, segs, dump, base):
            seen.setdefault(name, 0)
            seen[name] += 1
            print(f"+0x{off:04x} {name}")
        print(f"-- {len(seen)} distinct classes, {sum(seen.values())} slots")
        return 0
    for arg in argv[2:]:
        vtable = int(arg, 16)
        print(f"{vtable:#x} -> {class_name(path, segs, vtable)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
