"""Locate Itanium-ABI RTTI type_info objects and their vtables in a PIE ELF.

The MPC Sample binary is stripped, but its C++ RTTI names survive in .data, so a
class name string leads to its type_info object and from there to every vtable
that names it.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

from fwxref import Segment, segments, to_offset, to_vaddr


def read_at(path: Path, offset: int, size: int) -> bytes:
    with path.open("rb") as fh:
        fh.seek(offset)
        return fh.read(size)


def find_name(segs: list[Segment], path: Path, mangled: str) -> list[int]:
    needle = mangled.encode() + b"\x00"
    blob = path.read_bytes()
    hits, start = [], 0
    while True:
        i = blob.find(needle, start)
        if i < 0:
            return hits
        va = to_vaddr(segs, i)
        if va is not None:
            hits.append(va)
        start = i + 1


def find_typeinfo(path: Path, segs: list[Segment], name_vas: list[int]) -> list[int]:
    """A type_info is {vptr, name_ptr}; return the addresses of those objects."""
    out = []
    blob = path.read_bytes()
    for seg in segs:
        body = blob[seg.offset : seg.offset + seg.filesz]
        words = struct.unpack_from(f"<{len(body) // 8}Q", body)
        for i, w in enumerate(words):
            if w in name_vas and i > 0:
                # name pointer at object+8 -> object starts 8 bytes earlier
                va = seg.vaddr + (i - 1) * 8
                out.append(va)
    return out


def find_vtables(path: Path, segs: list[Segment], ti_vas: list[int]) -> dict[int, list[int]]:
    out: dict[int, list[int]] = {}
    blob = path.read_bytes()
    for seg in segs:
        body = blob[seg.offset : seg.offset + seg.filesz]
        words = struct.unpack_from(f"<{len(body) // 8}Q", body)
        for i, w in enumerate(words):
            if w in ti_vas:
                vtable = seg.vaddr + (i - 1) * 8
                out.setdefault(w, []).append(vtable)
    return out


def dump_vtable(path: Path, segs: list[Segment], vtable: int, count: int = 16) -> list[int]:
    off = to_offset(segs, vtable)
    if off is None:
        return []
    body = read_at(path, off, 8 * count)
    return list(struct.unpack(f"<{count}Q", body))


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print("usage: rtti.py BINARY MANGLED_NAME", file=sys.stderr)
        return 2
    path = Path(argv[1])
    segs = segments(path)
    names = find_name(segs, path, argv[2])
    print(f"name {argv[2]!r}: {len(names)} occurrence(s) {[hex(v) for v in names[:6]]}")
    tis = find_typeinfo(path, segs, set(names))
    print(f"type_info candidates: {[hex(v) for v in tis[:8]]}")
    vts = find_vtables(path, segs, set(tis))
    for ti, tables in vts.items():
        for vt in tables:
            entries = dump_vtable(path, segs, vt)
            fns = [e for e in entries if e]
            print(f"  type_info {ti:#x} -> vtable {vt:#x}")
            for k, e in enumerate(fns[:12]):
                print(f"      [{k}] {e:#x}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
