# mpc-mini

> **Firmware 1.3.0.12 only.** This mod is built against one exact build of
> `/usr/bin/MPC Sample` (`b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`);
> every hook is an absolute file offset into that binary, so it does not work on
> any other firmware. The autorun package refuses to load on a different build
> (it compares a hash of the app's first 4 KiB), but `lab.py preload` / manual
> `LD_PRELOAD` has no such guard and would crash the app.

A minimal mod for the Akai MPC Sample (firmware 1.3.0.12), delivered as an SD
`autorun` package. Nothing on disk is modified: the library is copied to `/run`
(tmpfs) and preloaded into the stock app, so a reboot returns to stock.

**Status: all four features verified on real hardware (2026-11-03).**

| # | feature | control | state |
| --- | --- | --- | --- |
| 1 | pitch shift in 16 Levels, ±8 semitones (half the pad grid) | `PLUS` / `MINUS` | verified on device |
| 2 | autotrim of the selected pad's sample to its silent edges | `SHIFT` + `B3` | analysis + layer write verified; **on-screen Start/End markers do not refresh** |
| 3 | chromatic control from an external MIDI controller | always while 16 LEVELS is lit | note path verified with simulated MIDI |
| 4 | shift overlay label | `SHIFT` held on the Sample page | verified on device |

Status history, evidence and open work: [`docs/MOD-MINI.md`](../docs/MOD-MINI.md).
The Sample/Trim page itself: [`docs/TRIM-PAGE.md`](../docs/TRIM-PAGE.md).
The am-mod study that supplied the overlay technique: [`docs/AM-MOD-NOTES.md`](../docs/AM-MOD-NOTES.md).
Ecosystem: [`docs/ECOSYSTEM.md`](../docs/ECOSYSTEM.md).

## Controls

```
SHIFT held : Loop Lock   Normalize   AutoTrim      <- AutoTrim is drawn by the mod
released   : Trim        Tune        Filter
```

* **Pitch shift** — `PLUS` / `MINUS` while 16 LEVELS is lit move the whole pad
  row by **8 semitones** per press (half of the 16-note grid), clamped to ±4
  presses. **Holding both keys at once resets the row to its original pitch.**
  The keys are *swallowed* so the stock "nudge the selected knob" action does
  not also run. The shift is applied in
  `0x27e4400(ctx, ?, ?, semitoneOffset)` — the function the 16-Levels renderer
  calls with `(pad & 0xf) - originalPad`.
* **Autotrim** — `SHIFT` + **B3**. Computes the first/last non-silent frame of
  the pad's sample from the app's own WAV copy (24-bit aware). The silence
  threshold is **relative to the file's peak** (default −40 dB, so low-level
  noise at the ends is trimmed as well, not only digital silence) with an
  absolute floor so a near-silent file is not trimmed away; `MPCMINI_TRIM_DB`
  overrides it (e.g. `-30` to trim more aggressively, `-60` to keep more).
  It then writes the trim core, the layer frames and the applied value. The audio state lands; the
  page's cached markers are not refreshed, because the page paints from a model
  object we have not identified yet (full account in `docs/TRIM-PAGE.md`).
* **Chromatic MIDI** — always on while 16 LEVELS is lit (no toggle). Off with
  `MPCMINI_MIDI=0`: the rawmidi reader takes the control-surface node from the
  app, which stops the stock app from starting.

## Install (device)

```sh
cd mod
./build.sh          # build/mpcmini.so
./pack.sh           # build/mpc-mini.zip

# card mounted at /Volumes/MPCSAMPLE
rm -rf /Volumes/MPCSAMPLE/autorun
unzip -q build/mpc-mini.zip -d /Volumes/MPCSAMPLE/
rm -f /Volumes/MPCSAMPLE/autorun/._* /Volumes/MPCSAMPLE/._autorun   # macOS AppleDouble
sync; diskutil unmount /Volumes/MPCSAMPLE
```

Insert the card and power on. The firmware runs `/autorun/autorun.sh`, which:

1. verifies the first 4 KiB of `/usr/bin/MPC Sample` against `44507c7a…` and
   exits silently on any other firmware;
2. copies `mpcmini.so` to `/run/mpcmini.so`;
3. writes `/run/systemd/system/ac50.service.d/mpcmini.conf` with `LD_PRELOAD`
   and restarts `ac50`.

To unload: reboot, or delete `autorun` from the card.

## Install (emulator)

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py run            # HVF, panel on :8865
MPCLAB_PORT_OFFSET=100 python3 lab.py preload /path/to/mod/build/mpcmini.so
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh 'tail -f /run/mpcmini.log'
```

Extra switches (off by default): `MPCMINI_HOOKS` (bitmask: 2 proc, 16 buttons,
32 note_on, 64 cs, 128 note_off), `MPCMINI_DEBUG=1`, `MPCMINI_PROBE=1` with
`MPCMINI_PROBES=<bits>`, `MPCMINI_MIDI=1`, `MPCMINI_TESTTRIM=0.09,0.45`.

## How it hooks

The library finds the app base with `dl_iterate_phdr`, maps a relay page and a
trampoline page within ±128 MB, and overwrites the first instruction of each
target with `B slot`. The slot jumps to the C hook; the hook calls a trampoline
that replays the function's prologue (copied only up to the first pc-relative
instruction) and branches into the body, so the original runs on its own frame
and returns to the hook. Both pages are `mprotect`ed executable — a missing
`mprotect` on the trampoline page makes every hook die with SIGSEGV at the
trampoline address.

The overlay is a second mechanism: the `SampleModeScreen` vtable slot 27
(`Component::paint`) is replaced, the stock paint is called first, and while
SHIFT is held the third soft-key cell is filled black and `AutoTrim` drawn with
the app's own `Graphics` calls. See [`docs/AM-MOD-NOTES.md`](../docs/AM-MOD-NOTES.md).

## Anchors (1.3.0.12)

| offset | meaning |
| --- | --- |
| `0x159f2f0` | button dispatch `(event)`, `event+8` code, `event+9` value |
| `0x27e4400` | 16-Levels pitch offset setter `(ctx, ?, ?, semitones)` |
| `0x21472f0` | 16-Levels render `(self, pad, ctx)` |
| `0x1149880` | component repaint `(p, 0, p[0x28])` |
| `0x102d5e0` / `0x102e000` | `Graphics::setColour` / `fillRect` |
| `0x1082810` / `0x102dca0` / `0x10817b0` | `Font` / `setFont` / `drawText` |
| `0xf535c0` | `juce::String::operator=(const char*)` |
| `0x6e212a0` | `SampleModeScreen` vtable (paint = slot 27) |
| `0x6e1b240` | `TrimParameter` vtable (`+0x19c` value, `+0x1e8` frames) |
| `0x15e70c0` | trim core `(float value, TrimParameter*)` |
| `0x13bb930` | value broadcaster `(property, float)` |
| `0x14a7580` | observable setter `(value, property)` |
| `0x27e4510` | 16-Levels pitch setter for the other mode branch |

## Build

```sh
./build.sh     # build/mpcmini.so  (zig cc -target aarch64-linux-gnu, -O2)
./pack.sh      # build/mpc-mini.zip (autorun package with a valid autorun.check)
```

`pack.sh` regenerates `autorun.check` = `sha256(autorun.json + autorun.sh + salt)`;
the firmware refuses a package whose check does not match.
