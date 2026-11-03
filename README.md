# mpc-minimod

A minimal **SD-card mod** for the Akai MPC Sample — firmware **1.3.0.12 only**.

Three features, nothing else:

| feature | control |
| --- | --- |
| **AutoTrim** — trims the selected pad's sample to its silent / low-noise edges | `SHIFT` + `B3` |
| **Octave shift in 16 Levels** — 8 semitones per press; both keys resets | `PLUS` / `MINUS` |
| **Chromatic MIDI** — an external controller plays the pads chromatically (opt-in) | `MPCMINI_MIDI=1` |

Nothing on disk is modified: the library is copied to `/run` (tmpfs) and
`LD_PRELOAD`ed into the stock app, so a reboot returns to stock.

## Install

```sh
cd mod
./build.sh && ./pack.sh          # -> build/mpc-mini.zip

# SD card mounted at /Volumes/MPCSAMPLE
rm -rf /Volumes/MPCSAMPLE/autorun
unzip -q build/mpc-mini.zip -d /Volumes/MPCSAMPLE/
rm -f /Volumes/MPCSAMPLE/autorun/._* /Volumes/MPCSAMPLE/._autorun   # macOS AppleDouble
sync; diskutil unmount /Volumes/MPCSAMPLE
```

Insert the card and power on, then confirm the prompt. The autorun script checks
the first 4 KiB of `/usr/bin/MPC Sample` and exits silently on any other build.

To unload: reboot, or delete `autorun` from the card.

## Requirements

Firmware **1.3.0.12** exactly — app sha256 `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`.
Every hook is an absolute offset into that build, so no other firmware works.

## Docs and other projects in this repo

**The mod**
- [`mod/README.md`](mod/README.md) — controls, install, how it hooks, anchor table
- [`AUTOTRIM-HANDOFF.md`](AUTOTRIM-HANDOFF.md) — AutoTrim end to end, and what is left
- [`docs/AUTOTRIM-NATIVE.md`](docs/AUTOTRIM-NATIVE.md) — the native trim dispatch
- [`docs/MOD-MINI.md`](docs/MOD-MINI.md) — design, verified behaviour, evidence trail
- [`docs/CODE-REVIEW.md`](docs/CODE-REVIEW.md) — review findings and fix status
- [`docs/TRIM-PAGE.md`](docs/TRIM-PAGE.md) — the Sample/Trim page internals

**The device**
- [`docs/BLUETOOTH.md`](docs/BLUETOOTH.md) · [`docs/WIFI.md`](docs/WIFI.md) — what the hardware actually has
- [`docs/PREFERENCES-TABS.md`](docs/PREFERENCES-TABS.md) — the Preferences tabs nobody documents
- [`docs/EMULATOR.md`](docs/EMULATOR.md) · [`docs/MODDING.md`](docs/MODDING.md) · [`docs/ECOSYSTEM.md`](docs/ECOSYSTEM.md) — emulator, autorun mechanism, ecosystem

**The converter — a separate tool that also lives here**
- [`docs/MPC2STEMS.md`](docs/MPC2STEMS.md) — `mpc2stems`: `.xpj` project → per-pad DAW stems
  that sum back to the device's Song export, plus [`CONVERTER-HANDOFF.md`](CONVERTER-HANDOFF.md)
  and the format notes in [`docs/FORMAT.md`](docs/FORMAT.md)
