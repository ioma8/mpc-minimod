# Add Reverb handoff

## Behavior

In Sample mode, open **SAMPLE OPTIONS** with `SHIFT+DATA`, select **Add Reverb**,
and press DATA. The action reads the selected pad's WAV, renders it through the
firmware's native `Air::H3M::StereoReverb`, appends the generated tail, then
uses the firmware's stock Chop Extract command to save and assign it to the next
empty pad. It does not replace the source. If there is no free pad or the
selected sample changes during processing, it stops without submitting output.

## Implementation

- `mod/src/h3m_effect.c` creates isolated native H3 effect instances. Reverb
  uses `StereoReverb` with the firmware factory at `base + 0x3c445b0`; Vinyl
  and Decimator use their own registered factories. The renderer passes a
  context containing
  sample rate and a 512-frame block size, calls vtable slot 9 with the context
  pointer, renders through slot 11 in stereo blocks, feeds four seconds of
  silence, and destroys through slot 1. The appended silent tail is trimmed
  using a peak-relative threshold; output peak is limited to 0.98 only if needed.
- `sample_transform_start(action)` reads the exact sample WAV and runs the
  selected transform on a worker. Action IDs are 0=HPSS, 1=Reverb, 2=Vinyl,
  and 3=Lo-Fi (Decimator).
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

- `Reverb4.wav` contains 154,886 frames, stereo, 44.1 kHz: the original 10,501
  frames plus 144,385 frames of rendered tail. The tail RMS was `0.035679`, and
  the output differs from the source.
- Mod status reached `Reverb: copy created`. This status is set only after
  native Extract reports successful output and pad assignment.
- The MPC Sample PID stayed `29682`; `NRestarts=0` before and after.

The harness checks for changed or newly created output WAVs. Quote the project
directory separately from the `Reverb*.wav` glob because the emulator's project
folder name contains square brackets. It also verifies that the output is
longer than the input and that the appended float32 tail is non-silent. An
earlier emulator run passed `1` as the setup callback argument and crashed.
Ghidra showed the slot forwards this value to a base initializer which
dereferences it as an audio-context pointer. Passing the context pointer fixes
the crash; the successful run had no restart.

## Built-in firmware effect

Ghidra identified the native `Air::H3M::StereoReverb` class, its factory,
constructor, process callback and parameter updater. Add Reverb, Vinyl, and
Lo-Fi (Decimator) instantiate their native H3 effects directly in the offline
worker. Vinyl uses Drive `0.18` and Noise Level `0.12`; Lo-Fi lowers bit depth
and sample rate. Stock Extract handles file and pad ownership. Both new actions
passed emulator validation on 2026-11-05, producing distinct same-length stereo
float WAVs and assigning each to a new pad without restarting the firmware. See
[the built-in effect trace](re/BUILTIN-REVERB.md) for factories, parameter
indices, context ABI, and detailed runtime measurements.
