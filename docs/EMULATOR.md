# Emulator

The MPC Sample runs as a virtual device: the **stock firmware 1.3.0.12** boots in
QEMU and the whole front panel — screen, pads, knobs, buttons, data wheel — is a
web page you can click.

This replaces the earlier hand-built QEMU lab, which is retired. Everything below
runs from `~/projects/customs/virtual-mpc-sample` (upstream:
<https://github.com/orrpunk-art/virtual-mpc-sample>).

## Start

```sh
cd ~/projects/customs/virtual-mpc-sample
python3 lab.py run
```

Then open **<http://127.0.0.1:8765/>** and click once for sound.

First boot takes ~30 s to reach the app.

## Stop

```sh
python3 lab.py stop
```

## What is already set up

The build is done and its inputs are staged, so `lab.py run` needs no Docker.

| | |
|---|---|
| `firmware/extracted/part1` | boot partition, 28,577,792 B |
| `firmware/extracted/part2` | rootfs, 427,117,568 B |
| `firmware/extracted/parr` | recovery, 28,303,360 B |
| `firmware/Firmware/MPCSAMPLE-1.3.0.12-update.img` | the update image |

All four are SHA-256 checked against `inputs.sha256`. The update image came from
the official updater bundle, `MPC Sample 1.3.0 Updater.app/Contents/Resources/Update.img`;
it is an `AZ0x` container — `BOOT` raw, the other partitions xz-compressed.

Built artefacts live in `out/`: `kernel/Image`, `initramfs.cpio.gz`,
`emmc.base.qcow2`, `lab-layer.ext4`, `bridge/mpclab-audio-bridge`.

## Rebuilding

Only needed if the source or firmware changes. Docker (colima) is required.

```sh
colima start --cpu 4 --memory 8
python3 lab.py build          # kernel ~15 min, then bridge and images
```

## Checking it is alive

```sh
python3 lab.py status          # cs / console / qmp / audio sockets
curl -s http://127.0.0.1:8765/state | head -c 300
```

`/state` shows live control-surface traffic from the guest — `rx_bytes`, the
LED `log` (`b0 <id> <state>`), the app's SysEx, and pot values.

## Driving a control from the command line

```sh
curl -X POST -H 'Content-Type: application/json' \
     -d '{"name":"SEQ","action":"press"}' http://127.0.0.1:8765/control
curl -X POST -H 'Content-Type: application/json' \
     -d '{"name":"SEQ","action":"release"}' http://127.0.0.1:8765/control
```

Control names come from `panel/controls.json`. The protocol is plain MIDI over
the control-surface UART: `90 <id> 7F` press, `80 <id> 00` release, LEDs back as
CC `b0 <id> <state>`.

## Notes

- QEMU 9+ and Python 3.9+ to run; Docker only to build.
- Screen is 320×240 (`out/run/mpc-virt.dtb`, patched at build time).
- The guest is reachable over `ssh -p 2022 root@127.0.0.1` (lab keys in
  `out/keys/`).
- Project-file conversion is a separate tool; see
  [`../converter/docs/FORMAT.md`](../converter/docs/FORMAT.md) and
  [`../converter/docs/FINDINGS.md`](../converter/docs/FINDINGS.md).
- Preloading the mod and driving it from tests:
  [`TOOLING.md`](TOOLING.md#2-the-emulator-lab) and
  [`FEATURES.md`](FEATURES.md#build-and-verification).
