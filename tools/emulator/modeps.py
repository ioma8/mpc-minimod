#!/usr/bin/env python3
"""Compute kernel-module dependency graphs without running depmod.

Debian ships kernel modules as `*.ko.xz` but generates `modules.dep` only in the
package postinst, i.e. on the target. When we want to boot a vendor rootfs under
QEMU we cannot run that postinst, so we recompute the graph here:

  * exported symbols  <- the module's `__ksymtab_strings` section
  * required symbols  <- `SHN_UNDEF` entries of its `.symtab`

A module depends on another when it requires a symbol the other exports. That is
exactly (a conservative approximation of) what `depmod` uses.

Usage:
    modeps.py scan <deb-or-dir> -o modules.json     # one-off, slow
    modeps.py closure modules.json --roots ext4,virtio_blk
    modeps.py depfile modules.json -o modules.dep
    modeps.py loadorder modules.json --roots ext4,virtio_blk
"""

from __future__ import annotations

import argparse
import io
import json
import lzma
import os
import sys
import tarfile
from pathlib import Path

SHN_UNDEF = 0


def _sections(d: bytes) -> dict[str, tuple[int, int]]:
    """Return {section name: (offset, size)} for an ELF64 little-endian object."""
    if d[:4] != b"\x7fELF" or d[4] != 2 or d[5] != 1:
        raise ValueError("not a 64-bit LE ELF")
    e_shoff = int.from_bytes(d[0x28:0x30], "little")
    e_shentsize = int.from_bytes(d[0x3A:0x3C], "little")
    e_shnum = int.from_bytes(d[0x3C:0x3E], "little")
    e_shstrndx = int.from_bytes(d[0x3E:0x40], "little")
    if e_shoff == 0 or e_shnum == 0:
        return {}

    def shdr(i: int) -> tuple[int, int, int, int, int]:
        o = e_shoff + i * e_shentsize
        name = int.from_bytes(d[o : o + 4], "little")
        typ = int.from_bytes(d[o + 4 : o + 8], "little")
        off = int.from_bytes(d[o + 0x18 : o + 0x20], "little")
        size = int.from_bytes(d[o + 0x20 : o + 0x28], "little")
        link = int.from_bytes(d[o + 0x28 : o + 0x2C], "little")
        return name, typ, off, size, link

    str_off, str_size = shdr(e_shstrndx)[2], shdr(e_shstrndx)[3]
    names = d[str_off : str_off + str_size]
    out: dict[str, tuple[int, int]] = {}
    for i in range(e_shnum):
        name_off, _typ, off, size, _link = shdr(i)
        end = names.find(b"\x00", name_off)
        nm = names[name_off:end].decode("utf-8", "replace")
        out.setdefault(nm, (off, size))
    return out


def analyse(blob: bytes) -> tuple[set[str], set[str]]:
    """Return (exported symbols, undefined symbols) for one module image."""
    secs = _sections(blob)
    exported: set[str] = set()
    # __ksymtab_strings holds exactly the EXPORT_SYMBOL names, NUL separated.
    for nm in ("__ksymtab_strings", "___ksymtab_strings"):
        if nm in secs:
            off, size = secs[nm]
            exported |= {s.decode("utf-8", "replace") for s in blob[off : off + size].split(b"\x00") if s}
    undefined: set[str] = set()
    if ".symtab" in secs:
        sym_off, sym_size = secs[".symtab"]
        strtab_idx = None
        # find .strtab by re-reading the section header linking of .symtab
        for i in range(int.from_bytes(blob[0x3C:0x3E], "little")):
            e_shentsize = int.from_bytes(blob[0x3A:0x3C], "little")
            o = int.from_bytes(blob[0x28:0x30], "little") + i * e_shentsize
            off = int.from_bytes(blob[o + 0x18 : o + 0x20], "little")
            if off == sym_off:
                strtab_idx = int.from_bytes(blob[o + 0x28 : o + 0x2C], "little")
                break
        if strtab_idx is not None:
            o = int.from_bytes(blob[0x28:0x30], "little") + strtab_idx * int.from_bytes(
                blob[0x3A:0x3C], "little"
            )
            st_off = int.from_bytes(blob[o + 0x18 : o + 0x20], "little")
            for e in range(sym_off, sym_off + sym_size, 24):
                st_name = int.from_bytes(blob[e : e + 4], "little")
                st_shndx = int.from_bytes(blob[e + 6 : e + 8], "little")
                if st_shndx != SHN_UNDEF or st_name == 0:
                    continue
                end = blob.find(b"\x00", st_off + st_name)
                undefined.add(blob[st_off + st_name : end].decode("utf-8", "replace"))
    return exported, undefined


def iter_modules(src: Path):
    """Yield (relpath-without-.xz, module bytes) for a .deb or an unpacked tree."""
    if src.is_dir():
        for p in sorted(src.rglob("*.ko*")):
            name = str(p.relative_to(src))
            name = name.removeprefix("lib/modules/").removeprefix("usr/lib/modules/")
            ver, _, rel = name.partition("/")
            compressed = p.suffix == ".xz"
            yield ver, rel.removesuffix(".xz"), p.read_bytes(), compressed
        return
    import subprocess

    raw = src.read_bytes()
    if raw[:8] != b"!<arch>\n":
        raise SystemExit(f"{src}: not a .deb (ar archive)")
    pos, blob = 8, None
    while pos + 60 <= len(raw):
        hdr = raw[pos : pos + 60]
        size = int(hdr[48:58].decode().strip())
        name = hdr[0:16].decode().strip()
        data = raw[pos + 60 : pos + 60 + size]
        if name.startswith("data.tar"):
            blob = data
            break
        pos += 60 + size + (size & 1)
    if blob is None:
        raise SystemExit("no data.tar in deb")
    tf = tarfile.open(fileobj=io.BytesIO(blob))
    for m in tf.getmembers():
        if not m.isfile() or ".ko" not in m.name:
            continue
        rel = m.name.removeprefix("./")
        rel = rel.removeprefix("usr/lib/modules/").removeprefix("lib/modules/")
        ver, _, sub = rel.partition("/")
        yield ver, sub.removesuffix(".xz"), tf.extractfile(m).read(), True


def scan(src: Path) -> dict:
    mods: dict[str, dict] = {}
    for ver, rel, raw, compressed in iter_modules(src):
        try:
            blob = lzma.decompress(raw) if compressed else raw
            exp, und = analyse(blob)
        except Exception as exc:  # noqa: BLE001 - a broken module must not abort the scan
            print(f"  ! {rel}: {exc}", file=sys.stderr)
            continue
        mods[rel] = {"ver": ver, "exports": sorted(exp), "needs": sorted(und)}
    return {"modules": mods}


def build_index(index: dict) -> tuple[dict[str, str], dict[str, list[str]]]:
    exporter: dict[str, str] = {}
    for rel, m in index["modules"].items():
        for s in m["exports"]:
            exporter.setdefault(s, rel)
    deps: dict[str, list[str]] = {}
    for rel, m in index["modules"].items():
        d = set()
        for s in m["needs"]:
            owner = exporter.get(s)
            if owner and owner != rel:
                d.add(owner)
        deps[rel] = sorted(d)
    return exporter, deps


def closure(deps: dict[str, list[str]], roots: list[str]) -> list[str]:
    order: list[str] = []
    seen: set[str] = set()

    def visit(n: str) -> None:
        if n in seen:
            return
        seen.add(n)
        for d in deps.get(n, []):
            visit(d)
        order.append(n)

    for r in roots:
        if r not in deps:
            raise SystemExit(f"unknown module {r!r}; known e.g. {sorted(deps)[:3]}")
        visit(r)
    return order


def resolve(deps: dict[str, list[str]], name: str) -> str:
    if name in deps:
        return name
    hits = [k for k in deps if k.endswith("/" + name + ".ko") or k == name + ".ko"]
    if len(hits) == 1:
        return hits[0]
    raise SystemExit(f"ambiguous or unknown module {name!r} ({len(hits)} hits)")


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("scan")
    s.add_argument("src", type=Path)
    s.add_argument("-o", "--out", type=Path, required=True)
    for name in ("closure", "loadorder", "depfile"):
        p = sub.add_parser(name)
        p.add_argument("index", type=Path)
        if name != "depfile":
            p.add_argument("--roots", required=True)
        if name == "depfile":
            p.add_argument("-o", "--out", type=Path, required=True)
        if name == "depfile":
            p.add_argument("--ver", required=True)
    a = ap.parse_args()

    if a.cmd == "scan":
        idx = scan(a.src)
        a.out.write_text(json.dumps(idx, indent=1))
        print(f"scanned {len(idx['modules'])} modules -> {a.out}")
        return

    idx = json.loads(a.index.read_text())
    _exp, deps = build_index(idx)
    if a.cmd == "depfile":
        lines = []
        for rel in sorted(deps):
            if not rel.startswith(f"{a.ver}/"):
                continue
            target = rel.split("/", 1)[1]
            ds = " ".join(d.split("/", 1)[1] for d in deps[rel])
            lines.append(f"{target}:" + (f" {ds}" if ds else ""))
        a.out.write_text("\n".join(lines) + "\n")
        print(f"wrote {a.out} ({len(lines)} entries)")
        return

    roots = [resolve(deps, r.strip()) for r in a.roots.split(",") if r.strip()]
    order = closure(deps, roots)
    if a.cmd == "closure":
        for m in order:
            print(m)
    else:
        for m in order:
            print(f"/lib/modules/{idx['modules'][m]['ver']}/{m.removesuffix('.ko')}.ko")


if __name__ == "__main__":
    main()
