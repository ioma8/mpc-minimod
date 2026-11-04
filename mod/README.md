# minimod

> **Firmware 1.3.0.12 only.** This mod is built against one exact build of
> `/usr/bin/MPC Sample` (`b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`);
> every hook is an absolute file offset into that binary, so it does not work on
> any other firmware. The autorun package refuses to load on a different build
> (it compares a hash of the app's first 4 KiB), but `lab.py preload` / manual
> `LD_PRELOAD` has no such guard and would crash the app.

A minimal mod for the Akai MPC Sample (firmware 1.3.0.12), delivered as an SD
`autorun` package. Nothing on disk is modified: the library is copied to `/run`
(tmpfs) and preloaded into the stock app, so a reboot returns to stock.

**Status: the four original features are verified on real hardware
(2026-11-03); SAMPLE OPTIONS, HPSS and Spectral Chop are emulator-verified
(2026-11-04).**

| # | feature | control | state |
| --- | --- | --- | --- |
| 1 | pitch shift in 16 Levels, ±8 semitones (half the pad grid) | `PLUS` / `MINUS` | verified on device |
| 2 | autotrim of the selected pad's sample to its silent edges | `SHIFT` + `B3` | analysis + layer write verified; **on-screen Start/End markers do not refresh** |
| 3 | chromatic control from an external MIDI controller | always while 16 LEVELS is lit | note path verified with simulated MIDI |
| 4 | shift overlay label | `SHIFT` held on the Sample page | verified on device |
| 5 | SAMPLE OPTIONS → HPSS split onto two pads | `SHIFT` + `DATA`, then `DATA` | emulator-verified; see [HPSS-HANDOFF](../HPSS-HANDOFF.md) |
| 6 | SAMPLE OPTIONS → Spectral Chop chops the selected pad at its detected onsets | `SHIFT` + `DATA`, choose Spectral Chop, `DATA` | emulator-verified, boundaries read back; see [SPECTRAL-CHOP](../docs/SPECTRAL-CHOP.md) |

Status history, evidence and open work: [`docs/MINIMOD.md`](../docs/MINIMOD.md).
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
* **Autotrim** — `SHIFT` + **B3**. The two resulting edges are drawn on the
  waveform (the page itself only re-reads its trim when the pad is re-selected,
  so without this the result would be invisible); any knob move or leaving the
  page removes them. Computes the first/last non-silent frame of
  the pad's sample from the app's own WAV copy (24-bit aware). The silence
  threshold is **relative to the file's peak** (default −40 dB, so low-level
  noise at the ends is trimmed as well, not only digital silence) with an
  absolute floor so a near-silent file is not trimmed away; `MINIMOD_TRIM_DB`
  overrides it (e.g. `-30` to trim more aggressively, `-60` to keep more).
  It then writes the trim core, the layer frames and the applied value. The audio state lands; the
  page's cached markers are not refreshed, because the page paints from a model
  object we have not identified yet (full account in `docs/TRIM-PAGE.md`).
* **SAMPLE OPTIONS / HPSS** — `SHIFT` + `DATA` opens the menu on the Sample
  page. Turn `DATA` to choose **HPSS**, **Spectral Chop** or **Close**, press
  `DATA` to confirm, or press `SHIFT` + `DATA` to close. HPSS reads the selected
  sample, runs the split on a worker thread, and creates `HPSS Harmonic` then
  `HPSS Percussive` on the next two empty pads through the stock Extract
  command. The source sample and the live trim/chop range are left unchanged.
  Needs two empty pads; outputs are float32 WAVs on this firmware. Full details
  and validation: [`HPSS-HANDOFF.md`](../HPSS-HANDOFF.md).
* **Spectral Chop** — from the same menu. Detects up to 16 spectral-flux
  onsets, then rewrites the selected pad's chop so each slice boundary is one
  of those frames: it grows the slice list to the onset count, writes each
  slice's start/end, shrinks the list, re-enters Chop so the view rebuilds
  from the model, and verifies every boundary before reporting. No samples,
  files or other pads are created. Algorithm and the model layout:
  [`docs/SPECTRAL-CHOP.md`](../docs/SPECTRAL-CHOP.md) and
  [`docs/re/SPECTRAL-CHOP-SOLVED.md`](../docs/re/SPECTRAL-CHOP-SOLVED.md).
* **Chromatic MIDI** — always on while 16 LEVELS is lit (no toggle). Off with
  `MINIMOD_MIDI=0`: the rawmidi reader takes the control-surface node from the
  app, which stops the stock app from starting.

## Install (device)

```sh
cd mod
./build.sh          # build/minimod.so
./pack.sh           # build/minimod.zip

# card mounted at /Volumes/MPCSAMPLE
rm -rf /Volumes/MPCSAMPLE/autorun
unzip -q build/minimod.zip -d /Volumes/MPCSAMPLE/
rm -f /Volumes/MPCSAMPLE/autorun/._* /Volumes/MPCSAMPLE/._autorun   # macOS AppleDouble
sync; diskutil unmount /Volumes/MPCSAMPLE
```

Insert the card and power on. The firmware runs `/autorun/autorun.sh`, which:

1. verifies the first 4 KiB of `/usr/bin/MPC Sample` against `44507c7a…` and
   exits silently on any other firmware;
2. copies `minimod.so` to `/run/minimod.so`;
3. writes `/run/systemd/system/ac50.service.d/minimod.conf` with `LD_PRELOAD`
   and restarts `ac50`.

To unload: reboot, or delete `autorun` from the card.

## Install (emulator)

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py run            # HVF, panel on :8865
MPCLAB_PORT_OFFSET=100 python3 lab.py preload /path/to/mod/build/minimod.so
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh 'tail -f /run/minimod.log'
```

Extra switches (off by default): `MINIMOD_HOOKS` (bitmask: 2 proc, 16 buttons,
32 note_on, 64 cs, 128 note_off), `MINIMOD_DEBUG=1`, `MINIMOD_PROBE=1` with
`MINIMOD_PROBES=<bits>`, `MINIMOD_MIDI=1`, `MINIMOD_TESTTRIM=0.09,0.45`.

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
| `0x16580d0` | stock Sample/Chop Extract handler |
| `0x143ed40` / `0x143f0b0` | `ExtractAndAssignSampleCommand` constructor / execute |
| `0x1d8aff0` | WAV writer callback (block `+0` frames, `+8` float32 pointer) |
| `0x286dd00` / `0x28047b0` | native free-pad finder / pad-occupancy predicate |
| `0xf535c0` | `juce::String::operator=(const char*)` |
| `0x6e212a0` | `SampleModeScreen` vtable (paint = slot 27) |
| `0x6e1b240` | `TrimParameter` vtable (`+0x19c` value, `+0x1e8` frames) |
| `0x15e70c0` | trim core `(float value, TrimParameter*)` |
| `0x13bb930` | value broadcaster `(property, float)` |
| `0x14a7580` | observable setter `(value, property)` |
| `0x27e4510` | 16-Levels pitch setter for the other mode branch |

## Build

```sh
./build.sh     # build/minimod.so  (zig cc -target aarch64-linux-gnu, -O2)
./pack.sh      # build/minimod.zip (autorun package with a valid autorun.check)
```

`pack.sh` regenerates `autorun.check` = `sha256(autorun.json + autorun.sh + salt)`;
the firmware refuses a package whose check does not match.
