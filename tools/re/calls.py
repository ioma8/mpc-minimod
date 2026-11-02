"""Find BL call sites to a target address in the stripped AArch64 PIE."""

from __future__ import annotations

import struct
import sys
from pathlib import Path

from fwxref import segments, to_vaddr


def callers(path: str | Path, target: int) -> list[int]:
    blob = Path(path).read_bytes()
    segs = segments(path)
    hits: list[int] = []
    for seg in segs:
        if not seg.executable:
            continue
        body = blob[seg.offset : seg.offset + seg.filesz]
        words = struct.unpack_from(f"<{len(body) // 4}I", body)
        for i, w in enumerate(words):
            if (w & 0xFC000000) != 0x94000000:
                continue
            imm = w & 0x3FFFFFF
            if imm & 0x2000000:
                imm -= 0x4000000
            pc = seg.vaddr + i * 4
            if pc + imm * 4 == target:
                hits.append(seg.offset + i * 4)
    return hits


def nearest_literals(path: str | Path, offsets: list[int], window: int = 64) -> dict[int, list[str]]:
    """Float constants loaded with fmov from an immediate near each call site."""
    blob = Path(path).read_bytes()
    segs = segments(path)
    out: dict[int, list[str]] = {}
    for off in offsets:
        chunk = blob[max(0, off - window * 4) : off + window * 4]
        words = struct.unpack_from(f"<{len(chunk) // 4}I", chunk)
        found = []
        for w in words:
            # fmov Sd, #imm8  (VFP immediate): 0x1E2x00xx family
            if (w & 0xFFE00000) == 0x1E200000 and ((w >> 5) & 0x1F) == 0:
                imm8 = (w >> 13) & 0xFF
                found.append(f"fmov_imm8={imm8:#04x}")
            # movz/movk building a float pattern
            if (w & 0x7F800000) == 0x52800000:
                imm16 = (w >> 5) & 0xFFFF
                hw = (w >> 21) & 3
                found.append(f"movz[{hw}]={imm16:#06x}")
        out[off] = found
    return out


def main(argv: list[str]) -> int:
    path, target = argv[1], int(argv[2], 0)
    offs = callers(path, target)
    print(f"{len(offs)} call site(s) to {target:#x}")
    for off in offs[:40]:
        print(f"  file {off:#x}  vaddr {to_vaddr(segments(path), off):#x}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
