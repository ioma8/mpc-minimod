# MiniMod

Current behavior of the release build for MPC Sample firmware 1.3.0.12.
Historical implementation details are kept in [`TRIM-PAGE.md`](TRIM-PAGE.md);
for current AutoTrim reverse-engineering findings see
[`AUTOTRIM-NATIVE.md`](AUTOTRIM-NATIVE.md).

## Shipped features

| Feature | Control | Behavior |
|---|---|---|
| Pitch shift in 16 Levels | PLUS / MINUS | Moves the 16-pad pitch row by 8 semitones per press, up to ±32 semitones. Pressing both resets to the original pitch. Keys are swallowed only while 16 Levels is active. |
| AutoTrim | SHIFT + B3 | Trims the selected pad's sample using native app control commands. Three presses while holding SHIFT use 3%, 6%, and 9% peak thresholds; the fourth clears trim. Releasing SHIFT resets the cycle. |
| AutoTrim caption | Hold SHIFT on the Sample page | Replaces the third soft-key caption with AutoTrim; suppressed in Chop mode. |

Chromatic MIDI from an external controller is not included in the release
build. Its investigation is in [`MIDI-CHROMATIC.md`](MIDI-CHROMATIC.md).

## AutoTrim behavior

The mod resolves the most recently triggered pad's sample from the app's pad
state, reads its WAV, and sets Start and End through the app's native control
dispatcher. It opens Start to zero, sets End to the new end frame, then sets
Start to the new beginning frame. This lets bounds expand when clearing an
existing trim. The app updates its own model, controls, and waveform markers;
the mod does not write guessed property fields or draw trim edges.

The WAV reader parses RIFF chunks and supports PCM 16/24/32-bit and IEEE
float32, including WAVE_FORMAT_EXTENSIBLE. The default first-step threshold is
3% of the file peak (about −30.46 dB), with a 90-count absolute floor. The
`MINIMOD_TRIM_DB` environment variable overrides that first-step relative
threshold. The 6% and 9% cycle steps use their fixed ratios.

AutoTrim is disabled in Chop mode so SHIFT+B3 keeps its stock Split behavior.
The B3 handler acts on the down edge because the device repeats button events
while held. A one-second toast confirms each trim step.

The selected pad index is currently the most recent note-on pad, defaulting to
A01 before a pad has been triggered. Emulator verification covers pad changes,
native marker refresh, the trim cycle, float WAVs A07/A09, and stable page
navigation. Hardware, save/reload, non-first sample layers, non-pad selection
routes, and selection during sequence playback remain to be checked.

## Pitch shift details

16 Levels mode is tracked from its panel toggle because no reliable mode field
was found. The flag is cleared when the Sample page is no longer visible. The
pitch adjustment is added at the app's 16-Levels pitch setter, not by modifying
the renderer's context object. The supported branch is the common 16-Levels
path; a separate firmware branch in the renderer remains unhooked.

## Build and verify

```sh
cd mod
./build.sh
./pack.sh
python3 tests/wav_edges.py
```

The emulator AutoTrim scenario is `python3 mod/tests/emulator_autotrim.py`
from the repository root. Release builds are the default; investigation hooks
are enabled only with `MINIMOD_DEBUG=1`. See [`EMULATOR.md`](EMULATOR.md) for
emulator setup.

## Repository notes

- [`MOD-REFERENCE.md`](MOD-REFERENCE.md): firmware anchors and reverse-engineering reference.
- [`AUTOTRIM-NATIVE.md`](AUTOTRIM-NATIVE.md): current AutoTrim path and measured evidence.
- [`CODE-REVIEW.md`](CODE-REVIEW.md): review of the earlier implementation; findings about property writes and the overlay are historical.
- [`PREFERENCES-TABS.md`](PREFERENCES-TABS.md) and [`TOUCH-PREFERENCES-WIRING.md`](TOUCH-PREFERENCES-WIRING.md): static TouchPreferences research. SHIFT+DATA activation has not been implemented.
