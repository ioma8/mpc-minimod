#!/usr/bin/env python3
"""Extract files out of a .deb without dpkg.

A .deb is an `ar` archive holding control.tar.* and data.tar.*. macOS has no
`ar`-for-xz pipeline worth relying on, so unpack it in Python.

    unpack-deb.py pkg.deb --kernel-dir out/k --modules-dir out/modules
    unpack-deb.py pkg.deb --extract bin/busybox:out/busybox --chmod 755
"""

from __future__ import annotations

import argparse
import io
import os
import sys
import tarfile
from pathlib import Path


def data_tar(deb: Path) -> tarfile.TarFile:
    raw = deb.read_bytes()
    if raw[:8] != b"!<arch>\n":
        raise SystemExit(f"{deb}: not a .deb")
    pos = 8
    while pos + 60 <= len(raw):
        hdr = raw[pos : pos + 60]
        size = int(hdr[48:58].decode().strip())
        name = hdr[0:16].decode().strip()
        body = raw[pos + 60 : pos + 60 + size]
        if name.startswith("data.tar"):
            return tarfile.open(fileobj=io.BytesIO(body))
        pos += 60 + size + (size & 1)
    raise SystemExit(f"{deb}: no data.tar member")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("deb", type=Path)
    ap.add_argument("--kernel-dir", type=Path, help="where to put vmlinuz/config")
    ap.add_argument("--modules-dir", type=Path, help="where to unpack the module tree")
    ap.add_argument("--extract", action="append", default=[], metavar="SRC:DST")
    ap.add_argument("--chmod", default=None)
    a = ap.parse_args()

    tf = data_tar(a.deb)
    wanted: dict[str, Path] = {}
    for spec in a.extract:
        src, _, dst = spec.partition(":")
        wanted[src.lstrip("./")] = Path(dst)

    for m in tf.getmembers():
        name = m.name.lstrip("./")
        if not m.isfile():
            continue
        if a.kernel_dir and name.startswith("boot/"):
            base = os.path.basename(name)
            if base.startswith(("vmlinuz", "config-", "System.map")):
                a.kernel_dir.mkdir(parents=True, exist_ok=True)
                (a.kernel_dir / base).write_bytes(tf.extractfile(m).read())
                continue
        if a.modules_dir and "/lib/modules/" in name and ".ko" in name:
            ver, _, rel = name.split("/lib/modules/", 1)[1].partition("/")
            dst = a.modules_dir / "lib" / "modules" / ver / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_bytes(tf.extractfile(m).read())
            continue
        key = name if name in wanted else next(
            (k for k in wanted if name.endswith("/" + k) or name == k), None)
        if key is not None:
            dst = wanted.pop(key)
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_bytes(tf.extractfile(m).read())
            if a.chmod:
                dst.chmod(int(a.chmod, 8))
    if wanted:
        raise SystemExit("not found in archive: " + ", ".join(wanted))
    print(f"  ok {a.deb.name}")


if __name__ == "__main__":
    main()
