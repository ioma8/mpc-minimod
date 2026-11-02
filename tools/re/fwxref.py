"""Fast AArch64 adrp/add xref scanner for the stripped MPC Sample binary.

Full auto-analysis of a 119 MB PIE is slow and mostly wasted; almost every
reference to a rodata string or a global is an ``adrp`` + ``add`` (or ``ldr``)
pair, which can be found with a linear scan.  This is the workhorse for reading
the firmware's DSP constants.
"""

from __future__ import annotations

import struct
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Segment:
    offset: int
    vaddr: int
    filesz: int
    flags: int

    @property
    def executable(self) -> bool:
        return bool(self.flags & 1)


def segments(path: str | Path) -> list[Segment]:
    data = Path(path).read_bytes()[:0x10000]
    e_phoff = struct.unpack_from("<Q", data, 0x20)[0]
    e_phentsize = struct.unpack_from("<H", data, 0x36)[0]
    e_phnum = struct.unpack_from("<H", data, 0x38)[0]
    out = []
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = struct.unpack_from("<I", data, off)[0]
        if p_type != 1:  # PT_LOAD
            continue
        p_flags = struct.unpack_from("<I", data, off + 4)[0]
        p_offset, p_vaddr = struct.unpack_from("<QQ", data, off + 8)
        p_filesz = struct.unpack_from("<Q", data, off + 32)[0]
        out.append(Segment(p_offset, p_vaddr, p_filesz, p_flags))
    return out


def to_vaddr(segs: list[Segment], offset: int) -> int | None:
    for s in segs:
        if s.offset <= offset < s.offset + s.filesz:
            return s.vaddr + (offset - s.offset)
    return None


def to_offset(segs: list[Segment], vaddr: int) -> int | None:
    for s in segs:
        if s.vaddr <= vaddr < s.vaddr + s.filesz:
            return s.offset + (vaddr - s.vaddr)
    return None


def _sext(value: int, bits: int) -> int:
    sign = 1 << (bits - 1)
    return (value & (sign - 1)) - (value & sign)


def find_string(segs, blob: bytes, needle: bytes) -> list[int]:
    """Virtual addresses of every occurrence of ``needle`` in a loaded segment."""
    hits = []
    start = 0
    while True:
        i = blob.find(needle, start)
        if i < 0:
            return hits
        va = to_vaddr(segs, i)
        if va is not None:
            hits.append(va)
        start = i + 1


def scan_xrefs(path: str | Path, targets: set[int], window: int = 4) -> dict[int, list[int]]:
    """Map each target address to the file offsets of instructions referencing it."""
    blob = Path(path).read_bytes()
    segs = segments(path)
    found: dict[int, list[int]] = {t: [] for t in targets}
    for seg in segs:
        if not seg.executable:
            continue
        body = blob[seg.offset : seg.offset + seg.filesz]
        words = struct.unpack_from(f"<{len(body) // 4}I", body)
        pending: list[tuple[int, int, int]] = []  # (index, reg, target_page)
        for i, w in enumerate(words):
            # ADRP: 1 immlo(2) 10000 immhi(19) Rd(5)
            if (w & 0x9F000000) == 0x90000000:
                immlo = (w >> 29) & 3
                immhi = (w >> 5) & 0x7FFFF
                rd = w & 0x1F
                pc = seg.vaddr + i * 4
                target = (pc & ~0xFFF) + (_sext((immhi << 2) | immlo, 21) << 12)
                pending = [(j, r, t) for (j, r, t) in pending if j > i - window] if False else pending
                pending = [p for p in pending if i - p[0] <= window]
                pending.append((i, rd, target))
                continue
            # ADD (imm, 64-bit): sf=1 0 0 100010 sh imm12 Rn Rd
            if (w & 0xFF800000) == 0x91000000:
                imm12 = (w >> 10) & 0xFFF
                rn = (w >> 5) & 0x1F
                if (w >> 22) & 1:
                    imm12 <<= 12
                for (j, r, page) in pending:
                    if r == rn:
                        abs_target = page + imm12
                        if abs_target in found:
                            found[abs_target].append(seg.offset + i * 4)
                continue
            # LDR (unsigned imm, 64-bit): 1111100101 imm12 Rn Rt
            if (w & 0xFFC00000) == 0xF9400000:
                imm12 = ((w >> 10) & 0xFFF) * 8
                rn = (w >> 5) & 0x1F
                for (j, r, page) in pending:
                    if r == rn:
                        abs_target = page + imm12
                        if abs_target in found:
                            found[abs_target].append(seg.offset + i * 4)
                continue
            if i - (pending[0][0] if pending else i) > window:
                pending = []
    return found


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print("usage: fwxref.py BINARY STRING [STRING...]", file=sys.stderr)
        return 2
    path = argv[1]
    blob = Path(path).read_bytes()
    segs = segments(path)
    targets: set[int] = set()
    names: dict[int, str] = {}
    for needle in argv[2:]:
        for va in find_string(segs, blob, needle.encode()):
            targets.add(va)
            names[va] = needle
    hits = scan_xrefs(path, targets)
    for va in sorted(hits):
        refs = hits[va]
        print(f"{names[va]!r} @ {va:#x}: {len(refs)} xref(s)")
        for off in refs[:24]:
            print(f"    file {off:#x}  vaddr {to_vaddr(segs, off):#x}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
