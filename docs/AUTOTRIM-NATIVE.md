# Native AutoTrim branch findings

The previous native replay supplied a null command context and normalized fractions.
Real knob calls supply a live dispatcher context and frame positions. The integer
argument formerly called mode is the pad index. Ghidra's FUN_02968450 resolves it
through the program's pointer vector at +0x2808 (end +0x2810). Each pad state holds
its layer array at +0x22e8; the first layer holds its sample at +0x4a8.

The new AutoTrim resolves the sample and dispatches commands using the same pad
index. It opens Start to zero before setting End and then Start, allowing bounds
to expand from a previously narrow trim. It does not call the old property scan,
write guessed model fields, or set overlay markers. The native command handles the
model and renderer. The command capture hook is enabled in release.

Fresh-emulator pad selection works. The old claim that channel-10 pad messages
cannot select the displayed sample is retracted for this emulator build.

Validation: release build, factory demo project, no prior knob learning required.
PAD 1: BoomBap-Kik-BTB MD.wav, 15129 frames, bounds 1..12487.
PAD 3: HipHop-Clp-Hrt OS.wav, 52775 frames, bounds 0..9652.
PAD 7: Trap-Vox-Zen1 91V.wav, 90197 frames, bounds 65..90197.
Native waveform markers and lower controls redraw; two-pad automated test retains
PID 687 and verifies page navigation responds. A03 screenshot shows End at ~18%.

Run from the worktree root after loading its release library:

    python3 mod/tests/emulator_autotrim.py

The test moves knobs through intermediate values before resetting them: sending
an unchanged MIDI value does not necessarily issue a new control change.

Remaining validation: hardware, project save/reload, non-first sample layers,
selection changed through routes other than pad presses, and selection while a
sequence is playing. The current index is the last note-on pad (default A01), so
sequencer playback may require a dedicated UI-selection source before release.
The original low-severity review cleanup is not completed by this branch.

## Float WAV correction

A07 and A09 use IEEE float32 WAV data, not integer PCM32. The earlier decoder
ignored the WAV format tag and interpreted float bit patterns as integer audio,
which made quiet tails appear loud. The decoder now distinguishes PCM and float,
rejects unsupported encodings, and handles full-scale negative PCM without a
16-bit absolute-value overflow. The default threshold remains -40 dB.

Verified in the emulator after this fix:
- A07: 81..60816 of 90197 frames (0.666 seconds removed from the tail).
- A09: 76..83103 of 98280 frames (0.344 seconds removed from the tail).

`python3 mod/tests/wav_edges.py` compiles the production helpers and checks
matching boundaries for PCM16, PCM24, PCM32, and IEEE float32 fixtures with
low-level noise, including a negative full-scale sample.

## Threshold adjustment requested after listening

The default relative amplitude threshold is now 3% of peak (about -30.46 dB),
three times the former 1%. `MPCMINI_TRIM_DB` still overrides it. Verified in the
emulator: ratio 0.0300, threshold 876 for peak 29204; A07 bounds 84..52786,
A09 bounds 89..73431. The PCM/float regression checks still pass.

## Four-step trim cycle (SHIFT held)

`SHIFT` + `B3` now cycles while SHIFT stays held, and the counter resets on
SHIFT release:

| press | threshold | effect on A01 `BoomBap-Kik-BTB MD` (15 129 frames) |
|---|---|---|
| 1 | 3 % of peak (876) | bounds `23..9794` |
| 2 | 6 % of peak (1752) | bounds `62..7731` (tighter) |
| 3 | 9 % of peak (2628) | bounds `71..5358` (tighter still) |
| 4 | none | bounds `0..15129` (trim removed) |

`B3` is button code `0x0e`, the same code the control surface repeats while the
button is held, so the handler acts on the press edge only (`g_b3_down`); without
that, one physical press fired the cycle up to 16 times. `MPCMINI_TRIM_DB` still
overrides the ratio for the first step only.
