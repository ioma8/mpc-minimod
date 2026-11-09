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

**Current status:** the Live Looper is emulator-verified (audio, state machine, sync quantization, close); pitch shift and full-keyboard chromatic external MIDI are verified on hardware. AutoTrim's native command and UI updates are verified in the emulator. SAMPLE OPTIONS, HPSS, Spectral Chop, Reverb, Vinyl, Lo-Fi, and Bass Synth are emulator-verified. Bass Synth saves a DaisySP physical-model C2 pluck and auditions a chromatic C2–D#3 range from pads 1–16 while its parameter page is open. It does not instantiate the firmware's FMBass class because that class requires an H3 host context. Details: [`docs/FEATURES.md`](../docs/FEATURES.md#bass-synth). Add Reverb appends a rendered tail through the native Extract writer; see [`docs/FEATURES.md`](../docs/FEATURES.md#add-reverb-vinyl-and-lo-fi). Per-feature behavior and validation: [`docs/FEATURES.md`](../docs/FEATURES.md); firmware mechanisms: [`docs/MECHANISMS.md`](../docs/MECHANISMS.md).

| # | feature | control | state |
| --- | --- | --- | --- |
| 1 | pitch shift in 16 Levels, ±8 semitones (half the pad grid) | `PLUS` / `MINUS` | verified on device |
| 2 | AutoTrim of the selected pad's sample | `SHIFT` + `B3` | native command path and marker refresh verified in emulator |
| 3 | AutoTrim soft-key caption | `SHIFT` held on the Sample page | verified; hidden in Chop mode |
| 4 | SAMPLE OPTIONS → HPSS split onto two pads | `SHIFT` + `DATA`, then `DATA` | emulator-verified; see [HPSS](../docs/FEATURES.md#hpss) |
| 5 | SAMPLE OPTIONS → Spectral Chop slices the selected pad at detected onsets | `SHIFT` + `DATA`, choose Spectral Chop, `DATA` | emulator-verified by `mod/tests/emulator_spectral.py`; see [Spectral Chop](../docs/FEATURES.md#spectral-chop) |
| 6 | SAMPLE OPTIONS → Add Reverb creates a reverbed copy with a tail on a free pad | `SHIFT` + `DATA`, choose Add Reverb, `DATA` | longer WAV and non-silent tail checked in emulator |
| 7 | SAMPLE OPTIONS → Bass Synth auditions C2–D#3 on pads 1–16 and saves a plucked C2 sample to the next free pad; K1/K2/K3 shape brightness, excitation and filter live | `SHIFT` + `DATA`, choose Bass Synth, play pads or press `DATA` to save | live audio mix, output WAV, C2 pitch, source preservation and no restart checked in emulator |
| 8 | external MIDI plays the selected pad chromatically over the whole keyboard (no wrapping, exact semitones) | MIDI keyboard, Pad MIDI In **On**, then 16 Levels | verified on device (emulator: 48→110.4, 60→220.7, 65→293.4 Hz on a 220 Hz sample) |

Chromatic external MIDI playback is available while 16 Levels is active; see
[`docs/FEATURES.md`](../docs/FEATURES.md#external-midi-chromatic-playback) for
setup and mechanism.

Docs: [`docs/README.md`](../docs/README.md) is the index. Feature behavior:
[`docs/FEATURES.md`](../docs/FEATURES.md). Firmware mechanisms and anchors:
[`docs/MECHANISMS.md`](../docs/MECHANISMS.md). Boot chain, autorun and hook
technique: [`docs/MODDING.md`](../docs/MODDING.md). Investigations and
disproved leads: [`docs/HISTORY.md`](../docs/HISTORY.md). Other addins and the
shared installer rules: [`docs/ADDIN-ECOSYSTEM.md`](../docs/ADDIN-ECOSYSTEM.md).

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
* **AutoTrim** — `SHIFT` + **B3**. The app's native control dispatcher updates
  Start, End, the model, and waveform markers. While SHIFT stays held, presses
  use 3%, 6%, 9%, then clear trim; release SHIFT to reset the cycle. The default
  threshold is 3% of peak (about −30.46 dB), with a 90-count floor.
  `MINIMOD_TRIM_DB` overrides the relative threshold for the first step. PCM
  16/24/32-bit and float32 WAV files are supported. AutoTrim is suppressed in
  Chop mode, where SHIFT+B3 is the stock Split command.
* **SAMPLE OPTIONS** — `SHIFT` + `DATA` opens the menu on the Sample
  page. Turn `DATA` to choose **HPSS**, **Spectral Chop**, **Add Reverb**, **Vinyl**, **Lo-Fi**, **Bass Synth** or **Close**, press
  `DATA` to confirm, or press `SHIFT` + `DATA` to close. The wheel uses the
  firmware's native signed DATA event (`0x140dbc0`), filtered to the control
  surface signal at `+0x2da8`. Repeated identical deltas still move. The native
  CC hook blocks knobs behind the menu; there are no raw MIDI, evdev or
  float-property navigation fallbacks. See
  [input path and regression checks](../docs/MECHANISMS.md#9-the-control-surface-and-input-path).
  HPSS runs on a worker thread and creates `HPSS Harmonic` then `HPSS
  Percussive` on the next two empty pads through the stock Extract command. The
  source sample and live trim/chop range are left unchanged. Needs two empty
  pads; outputs are float32 WAVs on this firmware. Details:
  [`docs/FEATURES.md`](../docs/FEATURES.md#hpss).
* **Spectral Chop** — from the same menu. Detects up to 16 spectral-flux onsets,
  then slices the selected pad at those frame positions. It drives the
  firmware's slice API and binding callback, which owns allocation and observer
  updates. Each boundary is read back before success. It creates no samples,
  files or other pads. Detector and validation:
  [`docs/FEATURES.md`](../docs/FEATURES.md#spectral-chop); slice API and chop
  binding: [`docs/MECHANISMS.md`](../docs/MECHANISMS.md#5-the-slice-container-and-a-slice).
  Acceptance test: `mod/tests/emulator_spectral.py`.
* **Add Reverb** — from the same menu. Reads the selected sample, renders a
  dry-plus-room copy with an appended RT60-length tail, then submits the float
  audio through stock Extract so the firmware assigns it to the next empty pad.
  The source is unchanged. DSP details and the writer hook are in
  [`docs/FEATURES.md`](../docs/FEATURES.md#add-reverb-vinyl-and-lo-fi) and
  [`docs/MECHANISMS.md`](../docs/MECHANISMS.md#the-writer-boundary).
* **Bass Synth** — from the same menu, opens a small parameter page. K1 changes
  string brightness, K2 excitation/structure and K3 low-pass cutoff. Press `DATA` to render
  one plucked C2 note (MIDI 36), held for 400 ms in a 1.5 second sample, then
  assign it through native Extract to the next empty pad. Pads 1–16 also play
  the live chromatic range C2–D#3; turning the knobs reshapes ringing and new
  notes. The current pad's sample is preserved. The DaisySP renderer avoids the firmware FMBass class,
  whose constructor requires the H3 host interface. Implementation and Ghidra
  evidence: [`docs/FEATURES.md`](../docs/FEATURES.md#bass-synth); emulator
  check: `mod/tests/emulator_bass_sample.py`.
* **Chromatic MIDI** — select a pad, enable **Pad MIDI In** in MIDI Config (it
  resets to Off on every app start), then enter 16 Levels. Every external note
  plays the selected pad from the **sample's own root note** (C3 when it has
  none), by exact semitones, with no wrapping and no out-of-range fallback to
  other pads. The mod rebuilds the note event for the selected pad, reads the
  root from `sample+0x130 -> data+0x3e4`, and calls the app's own pitch setter
  (`w3 = note - root`) before dispatching with the physical-pad source;
  note-offs are tracked and replayed the same way. With 16
  Levels off the dispatcher is left byte-identical to stock. See
  [`docs/FEATURES.md`](../docs/FEATURES.md#external-midi-chromatic-playback);
  emulator check: `tests/emulator_midi_chromatic.py`.

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

Release builds run the fixed hook set and honour **no** environment switches
except `MINIMOD_TRIM_DB` (the AutoTrim threshold, described under Controls). A
`-DMINIMOD_DEBUG=1` build additionally reads `MINIMOD_HOOKS` (bitmask: 2 proc,
16 buttons, 32 note_on, 64 cs, 128 note_off), `MINIMOD_PROBE=1`, and the
investigation switches `MINIMOD_VIS`, `MINIMOD_BTNLOG`,
`MINIMOD_DUMP`, `MINIMOD_SNAPDIFF`, `MINIMOD_ENV` and `MINIMOD_SAFE`. The
release build ignores all of those.
Chromatic MIDI is handled by the native hook in the default release build; it
has no separate thread or environment switch.

## How it hooks

The library finds the app base with `dl_iterate_phdr`, maps a relay page and a
trampoline page within ±128 MB, and overwrites the first instruction of each
target with `B slot`. The slot jumps to the C hook; the hook calls a trampoline
that replays the function's prologue (copied only up to the first pc-relative
instruction) and branches into the body, so the original runs on its own frame
and returns to the hook. Both pages are `mprotect`ed executable — a missing
`mprotect` on the trampoline page makes every hook die with SIGSEGV at the
trampoline address.

The paint hook draws the temporary AutoTrim soft-key caption and confirmation
toast. It no longer draws trim edges: those are updated by the native command.
The overlay technique (paint-slot swap, graphics calls) is described in [`docs/MODDING.md`](../docs/MODDING.md#63-am-mod--the-overlay-technique).

## Anchors (mod offsets, 1.3.0.12)

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
./build.sh     # build/minimod.so  (the Rust cdylib, cross-linked with zig cc; no C is compiled)
./pack.sh      # build/minimod.zip (autorun package with a valid autorun.check)
```

`pack.sh` regenerates `autorun.check` = `sha256(autorun.json + autorun.sh + salt)`;
the firmware refuses a package whose check does not match.
