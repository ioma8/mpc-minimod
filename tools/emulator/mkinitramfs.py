#!/usr/bin/env python3
"""Assemble a minimal initramfs that boots the AC50 rootfs under QEMU.

The image contains a static busybox, the kernel modules needed before and after
mounting root, and an `/init` that

  1. brings up /proc, /sys and devtmpfs,
  2. loads the boot-critical modules (ext4 + virtio_blk) and mounts root,
  3. loads display/input/audio modules,
  4. overlays tmpfs on the directories the real device keeps on separate
     partitions (/data, /content, ...), which do not exist here,
  5. switch_roots into the vendor rootfs, either to a shell (`emu.shell`) or to
     the vendor `/sbin/init`.

Modules are copied decompressed because busybox `insmod` cannot read `.ko.xz`.
Load order comes from `modeps.py`, which derives it from the symbol graph.
"""

from __future__ import annotations

import argparse
import gzip
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

INIT = r"""#!/bin/busybox sh
/bin/busybox --install -s /bin
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
mount -t proc     proc     /proc
mount -t sysfs    sysfs    /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null
mkdir -p /dev/pts /run /tmp /newroot
mount -t devpts devpts /dev/pts 2>/dev/null
mount -t tmpfs  tmpfs     /tmp

say() { echo "[emu] $*"; }
CMDLINE=$(cat /proc/cmdline 2>/dev/null)

# --- boot-critical modules -------------------------------------------------
%(boot)s

say "mounting root from /dev/%(rootdev)s"
if ! mount -o rw /dev/%(rootdev)s /newroot; then
    say "root mount FAILED -- block devices:"
    ls /dev | tr '\n' ' '; echo
    dmesg | tail -5
    exec /bin/sh
fi
say "root mounted: $(ls /newroot | tr '\n' ' ')"

# --- everything else -------------------------------------------------------
%(late)s

# --- writable overlays for the device's separate partitions -----------------
for d in data content media factory secure-media var/lib var/volatile run tmp; do
    mkdir -p /newroot/$d
    mount -t tmpfs -o mode=0755 tmpfs /newroot/$d 2>/dev/null
done
mkdir -p /newroot/var/volatile/lib /newroot/var/volatile/log
mount --bind /newroot/var/volatile/lib /newroot/var/lib 2>/dev/null
mount --bind /newroot/var/volatile/log /newroot/var/log 2>/dev/null
mkdir -p /newroot/dev /newroot/proc /newroot/sys
mount --move /dev  /newroot/dev  2>/dev/null || mount -t devtmpfs devtmpfs /newroot/dev
mount --move /proc /newroot/proc 2>/dev/null || mount -t proc     proc     /newroot/proc
mount --move /sys  /newroot/sys  2>/dev/null || mount -t sysfs    sysfs    /newroot/sys

# --- optional probe script (development hook) ------------------------------
[ -f /emu/hook.sh ] && . /emu/hook.sh

case "$CMDLINE" in *emu.probe*)
    say "emu.probe -> halting"
    sync
    poweroff -f
    ;;
esac

case "$CMDLINE" in *emu.shell*)
    say "emu.shell requested -> root shell"
    exec switch_root /newroot /bin/sh
    ;;
esac
say "starting vendor init"
exec switch_root /newroot /sbin/init
"""


def insmod_lines(modules: list[str], ver: str, tag: str) -> str:
    out = []
    for rel in modules:
        name = rel.split("/")[-1].removesuffix(".ko")
        out.append(f'insmod /lib/modules/{ver}/{rel} || say "insmod {name} failed"')
    return "\n".join(out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--index", type=Path, required=True, help="output of `modeps.py scan`")
    ap.add_argument("--deb", type=Path, required=True, help="kernel .deb holding the modules")
    ap.add_argument("--busybox", type=Path, required=True)
    ap.add_argument("--boot-roots", default="ext4,virtio_blk")
    ap.add_argument("--late-roots", default="virtio-gpu,virtio_input,evdev,snd-hda-intel,snd-pcm,soundcore,drm")
    ap.add_argument("--rootdev", default="vda")
    ap.add_argument("--hook", type=Path, help="script sourced after root is mounted")
    ap.add_argument("--extra", type=Path, action="append", default=[],
                    help="directory copied into /emu/extra inside the initramfs")
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    tool = Path(__file__).with_name("modeps.py")
    idx = json.loads(args.index.read_text())
    ver = next(iter(idx["modules"].values()))["ver"]

    def order(roots: str) -> list[str]:
        r = subprocess.run(
            [sys.executable, str(tool), "loadorder", str(args.index), "--roots", roots],
            capture_output=True, text=True, check=True,
        )
        prefix = f"/lib/modules/{ver}/"
        return [ln[len(prefix):] for ln in r.stdout.split() if ln.startswith(prefix)]

    boot, late = order(args.boot_roots), order(args.late_roots)

    # modules live in the deb as ./usr/lib/modules/<ver>/<rel>.xz
    import io, lzma, tarfile

    raw = args.deb.read_bytes()
    pos, blob = 8, None
    while pos + 60 <= len(raw):
        hdr = raw[pos:pos + 60]
        size = int(hdr[48:58].decode().strip())
        if hdr[0:16].decode().strip().startswith("data.tar"):
            blob = raw[pos + 60 : pos + 60 + size]
            break
        pos += 60 + size + (size & 1)
    tf = tarfile.open(fileobj=io.BytesIO(blob))
    wanted = {f"./usr/lib/modules/{ver}/{rel}.xz": rel for rel in boot + late}
    wanted |= {f"./usr/lib/modules/{ver}/{rel}": rel for rel in boot + late}

    with tempfile.TemporaryDirectory() as td:
        root = Path(td) / "root"
        for d in ("bin", "sbin", "usr/bin", "usr/sbin", "proc", "sys", "dev", "dev/pts",
                  "run", "tmp", "newroot", "lib/modules"):
            (root / d).mkdir(parents=True, exist_ok=True)
        shutil.copy2(args.busybox, root / "bin/busybox")
        (root / "bin/busybox").chmod(0o755)

        got = 0
        for m in tf.getmembers():
            rel = wanted.get(m.name)
            if rel is None or not m.isfile():
                continue
            dst = root / "lib/modules" / ver / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            data = tf.extractfile(m).read()
            if m.name.endswith(".xz"):
                data = lzma.decompress(data)
            dst.write_bytes(data)
            got += 1
        missing = [r for r in boot + late if not (root / "lib/modules" / ver / r).exists()]
        if missing:
            raise SystemExit(f"modules missing from deb: {missing}")

        for ex in args.extra:
            dst = root / "emu/extra"
            dst.mkdir(parents=True, exist_ok=True)
            for f in sorted(ex.iterdir()):
                if f.is_file():
                    shutil.copy2(f, dst / f.name)
                    (dst / f.name).chmod(0o755)

        if args.hook:
            (root / "emu").mkdir(exist_ok=True)
            shutil.copy2(args.hook, root / "emu/hook.sh")
        (root / "init").write_text(
            INIT % {
                "boot": insmod_lines(boot, ver, "boot"),
                "late": insmod_lines(late, ver, "late"),
                "rootdev": args.rootdev,
            }
        )
        (root / "init").chmod(0o755)

        listing = subprocess.run(
            ["find", ".", "-print"], cwd=root, capture_output=True, text=True, check=True
        ).stdout
        cpio = subprocess.run(
            ["/usr/bin/cpio", "-o", "-H", "newc"],
            input=listing.encode(), cwd=root, capture_output=True, check=True,
        ).stdout
        args.out.write_bytes(gzip.compress(cpio, 6))
        print(f"wrote {args.out} ({args.out.stat().st_size/1e6:.1f} MB, {got} modules, kernel {ver})")


if __name__ == "__main__":
    main()
