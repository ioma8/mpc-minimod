# AutoTrim handoff

> **Historical.** Written while the mod was C. `mod/src/minimod.c` and the other C and C++ sources are
> gone (see git history); the equivalent code is in `rust/crates/`, mapped in the "Final state" section of
> [`RUST-PORT.md`](docs/RUST-PORT.md). File, line and function references below point at the removed C, and build
> commands that compile it no longer apply (`mod/build.sh` builds the Rust crate).

Current source of truth for the AutoTrim implementation in `main`. Firmware
support is limited to MPC Sample 1.3.0.12.

## Current status

AutoTrim is triggered by **SHIFT + B3** on the Sample page. It analyzes the
selected pad's WAV and sets the sample's trim through the app's own control
dispatcher. The app updates its trim model and waveform markers natively; the
old property scan, guessed model writes, and trim-edge overlay are no longer
used for AutoTrim.

While SHIFT remains held, successive B3 presses use thresholds of 3%, 6%, and
9% of peak, then clear the trim on the fourth press. Releasing SHIFT resets the
cycle. The threshold has a 90-count absolute floor; `MINIMOD_TRIM_DB` overrides
the relative threshold for the first step (default about −30.46 dB). The parser
supports PCM 16/24/32-bit and IEEE float32 WAV data, including extensible WAV
headers.

AutoTrim is disabled while the Sample page is in Chop mode, where SHIFT+B3 is
the stock Split action. The AutoTrim soft-key caption is also hidden in Chop
mode. The button repeats while held, so only its press edge acts. Each trim
shows a one-second confirmation toast.

## Implementation outline

1. The button hook tracks SHIFT, B3 press edges, the 16 Levels toggle, and the
   most recently triggered pad.
2. `do_autotrim()` resolves that pad's sample from the surface's pad-state
   vector (`+0x2808/+0x2810`), its layer array (`+0x22e8`), and the sample at
   layer `+0x4a8`.
3. It resolves the WAV path from the sample object first, then uses a pointer
   scan or matches frame count against a newly recorded file in TempProjects.
   The WAV parser validates the file's frame count against the loaded sample.
4. It computes the first and last frames above the selected threshold, then
   calls the app's native control handler (`0x1968020`) with the live dispatcher
   context and pad index. Start is opened to zero first, End is set to
   `last + 1`, and Start is set to `first`, all in frame positions.
5. The native command updates the model and renderer. No app property is
   pre-written and no trim marker is drawn by the mod.

Relevant source: [`mod/src/minimod.c`](mod/src/minimod.c), especially
`buttons()`, `wav_info()`, `wav_edges()`, and `do_autotrim()`.

## Evidence and remaining checks

The native command path, live waveform markers, Start/End controls, pad changes,
float WAV decoding, and the threshold cycle were verified in the emulator. The
automated emulator check is `python3 mod/tests/emulator_autotrim.py`; WAV parser
fixtures are checked by `python3 mod/tests/wav_edges.py`.

Remaining validation recorded in [`docs/AUTOTRIM-NATIVE.md`](docs/AUTOTRIM-NATIVE.md):
real hardware, project save/reload, non-first sample layers, sample selection
through routes other than pad presses, and selection while a sequence is
playing. The selected pad is currently inferred from the most recent note-on,
defaulting to A01 before a pad has been triggered.

## Build and emulator

```sh
cd mod
./build.sh
./pack.sh
```

The release build is the default; investigation hooks and logging require
`MINIMOD_DEBUG=1`. The emulator instructions and port configuration are in
[`docs/EMULATOR.md`](docs/EMULATOR.md).

## Older investigation notes

[`docs/TRIM-PAGE.md`](docs/TRIM-PAGE.md) and much of
[`docs/CODE-REVIEW.md`](docs/CODE-REVIEW.md) are chronological records from the
former heap/property/overlay implementation. Their old conclusions about
marker refresh, pad selection, and the −40 dB threshold were superseded by the
native-control implementation; use [`docs/AUTOTRIM-NATIVE.md`](docs/AUTOTRIM-NATIVE.md)
for the current findings.
