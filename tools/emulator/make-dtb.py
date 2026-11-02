#!/usr/bin/env python3
"""Give the QEMU `virt` machine the identity of the AC50 board.

The MPC application decides which product it is running on from the *device
tree*, not from a config file:

    /sys/firmware/devicetree/base/inmusic,product-code
    /sys/firmware/devicetree/base/compatible
    /sys/firmware/devicetree/base/inmusic,az07-pcb-rev
    /sys/firmware/devicetree/base/chosen/u-boot,version

Without those it prints `Unhandled board platform: <unknown>` and stops. QEMU
generates its own DT for `-M virt`, so we dump it, graft the vendor properties
onto the root node (keeping `linux,dummy-virt` in `compatible` so the rest of
the platform still probes), and boot with `-dtb`.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import tempfile
from pathlib import Path

IDENTITY = {
    "model": '"MPC Sample"',
    "inmusic,product-code": '"AC50"',
    "inmusic,az07-pcb-rev": '"A"',
    "inmusic,panel-rotation": "<0x00000000>",
}

COMPATIBLE = 'compatible = "inmusic,ac50", "inmusic,az07", "rockchip,rk3566", "linux,dummy-virt";'


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, check=True, **kw)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--qemu", default="/opt/homebrew/bin/qemu-system-aarch64")
    ap.add_argument("--dtc", default="/opt/homebrew/bin/dtc")
    ap.add_argument("--cpus", default="4")
    ap.add_argument("--mem", default="4096")
    ap.add_argument("--uboot-version", default="U-Boot 2017.09-az07 (Mar 16 2026)")
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    with tempfile.TemporaryDirectory() as td:
        raw = Path(td) / "virt.dtb"
        run([
            args.qemu, "-M", f"virt,dumpdtb={raw}", "-accel", "hvf", "-cpu", "max",
            "-smp", args.cpus, "-m", args.mem, "-nographic",
        ])
        dts = run([args.dtc, "-I", "dtb", "-O", "dts", str(raw)]).stdout

    # Root node: rewrite the machine identity. `linux,dummy-virt` stays last in
    # compatible so every QEMU virt driver keeps matching.
    root_props = "\n".join(f'\t{k} = {v};' for k, v in IDENTITY.items())
    dts, n = re.subn(
        r'(\tmodel = )"linux,dummy-virt";',
        lambda m: m.group(1) + IDENTITY["model"] + ";",
        dts, count=1,
    )
    if n != 1:
        raise SystemExit("could not find the root model property")
    dts, n = re.subn(
        r'\tcompatible = "linux,dummy-virt";', "\t" + COMPATIBLE, dts, count=1
    )
    if n != 1:
        raise SystemExit("could not find the root compatible property")
    extra = "\n".join(
        f'\t{k} = {v};' for k, v in IDENTITY.items() if k not in ("model",)
    )
    dts = dts.replace("\t" + COMPATIBLE, "\t" + COMPATIBLE + "\n" + extra, 1)

    # U-Boot version, which the app also reads back out of /chosen.
    dts, n = re.subn(
        r"(\tchosen \{\n)",
        lambda m: m.group(1) + f'\t\tu-boot,version = "{args.uboot_version}";\n',
        dts, count=1,
    )
    if n != 1:
        raise SystemExit("could not find the /chosen node")

    with tempfile.TemporaryDirectory() as td:
        src = Path(td) / "patched.dts"
        src.write_text(dts)
        run([args.dtc, "-I", "dts", "-O", "dtb", "-o", str(args.out), str(src)])

    print(f"wrote {args.out} ({args.out.stat().st_size} bytes)")
    print(f"  model      = {IDENTITY['model']}")
    print(f"  compatible = {COMPATIBLE.split('=', 1)[1].strip()}")
    print(f"  product    = {IDENTITY['inmusic,product-code']}")


if __name__ == "__main__":
    main()
