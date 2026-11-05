# Add Reverb handoff

## Behavior

In Sample mode, open **SAMPLE OPTIONS** with `SHIFT+DATA`, select **Add Reverb**,
and press DATA. The action reads the selected pad's WAV, renders a dry-plus-room
copy followed by an RT60-length room tail, then uses the firmware's stock Chop
Extract command to save and assign it to the next empty pad. It does not replace
the source. If there is no free pad or the selected sample changes during
processing, it stops without submitting the output.

## Implementation

- `mod/src/reverb.c` renders a stereo-decorrelated Schroeder room: four feedback
  combs, two allpass diffusers, 14 ms pre-delay, 6 kHz damping, 0.88 dry gain
  and 0.30 wet gain. RT60 is `clamp(source_duration * 0.75, 0.25 s, 1.8 s)`;
  the appended tail is one RT60 long. Output is
  peak-limited to 0.98 only when the dry-plus-wet sum would exceed it.
- `sample_transform_start(1)` reads the exact sample WAV and runs DSP on a worker.
  The UI thread checks that the page and sample are still the ones processed.
- `hpss_extract_ctx()` reuses the established Extract creation and assignment
  mechanism. The writer hook substitutes the processed float buffer and names
  the result `Reverb` (the app adds a number on collision).
- Ghidra showed the native reader ends at the source frame count
  (`FUN_021c6e40` checks the source reader's length). The writer callback receives
  a mutable frame count, and the concrete WAV writer consumes that count.
  `hpss_writer_hook()` substitutes the final source block with the rest of the
  rendered buffer and temporarily extends that block's frame count, so native
  Extract writes the tail and finalizes a longer WAV. It restores the block
  before returning to the reader. The writer cursor marks the full buffer
  consumed only after this final write.
- The former approach requested a longer source range and was clipped by the
  native reader. Extending the last writer block avoids reading beyond the
  source and retains native file registration and pad assignment.

The frame-count getter is `FUN_02a41f80` (`sample+0x130+4`); the reader loop is
`FUN_021c6e40`; the block-to-writer callback is `FUN_01e8aff0`; the concrete
float WAV writer is `FUN_01e93570`. Their addresses and the writer-boundary
contract are recorded in [FIRMWARE-MECHANISMS.md](re/FIRMWARE-MECHANISMS.md).

Raw Ghidra Java-script output for the frame getter and reader bound is in
[`reverb-reader-boundary.txt`](re/hpss/reverb-reader-boundary.txt). The callback
and concrete writer decompilations are in
[`read-loop.txt`](re/hpss/read-loop.txt) and
[`wav-write-block.txt`](re/hpss/wav-write-block.txt).

## Emulator validation

Validated on 2026-11-05 through the real SAMPLE OPTIONS menu with
`mod/tests/emulator_reverb.py --lab /Users/jakubkolcar/projects/customs/virtual-mpc-sample`.
The test selected A01 (`RnB-Kik-Grove1 91V.wav`, 10,501 frames), invoked Add
Reverb, and observed:

- `Reverb2.wav` contains 21,526 frames, stereo, 44.1 kHz: the original 10,501
  frames plus 11,025 frames (250 ms) of rendered tail. The tail RMS was
  `0.002719`, and the output differs from the source.
- Mod status reached `Reverb: copy created`. This status is set only after
  native Extract reports successful output and pad assignment.
- The MPC Sample PID stayed `26817`; `NRestarts=0` before and after.

The harness checks for changed or newly created output WAVs. Quote the project
directory separately from the `Reverb*.wav` glob because the emulator's project
folder name contains square brackets. The same test verifies that the output is
longer than the input and that the appended float32 tail is non-silent.

## Ghidra evidence

The current Java-script bridge found firmware reverb classes and presets,
including `StereoReverb`, `SpringReverb`, `NonLinearReverb`, and `Studio
Reverb`. The entry point above instead uses local offline DSP so it can process
the exact sample selected on the pad without attaching an effect to the live
program. Stock Extract remains responsible for the new pad and file ownership.
