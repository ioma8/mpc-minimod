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
| External MIDI chromatic playback | Select a sample, enable Pad MIDI In, turn on 16 LEVELS | Transposes external MIDI note-ons in the native event path from C3; firmware routing and note-offs remain native. |
| SAMPLE OPTIONS → HPSS | SHIFT + DATA, choose HPSS, DATA | Splits the selected sample into `HPSS Harmonic` and `HPSS Percussive` on the next two free pads through stock Extract; the source is untouched. |
| SAMPLE OPTIONS → Spectral Chop | SHIFT + DATA, choose Spectral Chop, DATA | Chops the selected pad in place at up to 16 detected frames, via the native slice API; every boundary is read back before it reports. |
| SAMPLE OPTIONS → Add Reverb / Vinyl / Lo-Fi | SHIFT + DATA, choose, DATA | Renders the selected sample through one of the firmware's own H3 effects and assigns the copy to the next free pad; Reverb also appends its tail. |
| SAMPLE OPTIONS → Bass Synth | SHIFT + DATA, choose, DATA | Auditions C2–D#3 from pads 1–16 and saves a DaisySP C2 pluck to the next free pad; K1–K3 shape it live. |

SAMPLE OPTIONS mechanics and validation: [`SPECTRAL-CHOP.md`](SPECTRAL-CHOP.md),
[`REVERB-HANDOFF.md`](REVERB-HANDOFF.md), [`re/BASS-SAMPLE.md`](re/BASS-SAMPLE.md),
and [`re/FIRMWARE-MECHANISMS.md`](re/FIRMWARE-MECHANISMS.md) §9–10.
Implementation evidence, setup and limits for the MIDI path are in
[`MIDI-CHROMATIC.md`](MIDI-CHROMATIC.md).

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

# host-side checks (no emulator, no device) — all four must pass
python3 tests/wav_edges.py      # WAV parser: PCM 16/24/32 + IEEE float32 edges
python3 tests/hpss_dsp.py       # HPSS C port against the Python reference
python3 tests/spectral_onsets.py # onset detector fixtures
python3 tests/menu_input.py     # native DATA events, menu ownership, knob filter
```

The emulator scenarios live in `mod/tests/`, need the sibling
`virtual-mpc-sample` checkout, and expect the built library to be preloaded
(`cd ../virtual-mpc-sample && python3 lab.py preload <repo>/mod/build/minimod.so`):

```sh
python3 mod/tests/emulator_menu.py --panel http://127.0.0.1:8765
python3 mod/tests/emulator_autotrim.py --lab ../virtual-mpc-sample --pad 1
python3 mod/tests/emulator_spectral.py --lab ../virtual-mpc-sample --boots 1
python3 mod/tests/emulator_reverb.py --lab ../virtual-mpc-sample
python3 mod/tests/emulator_bass_sample.py --lab ../virtual-mpc-sample
python3 mod/tests/emulator_midi_chromatic.py --lab ../virtual-mpc-sample   # needs Pad MIDI In = On, MIDI Port = External
```

`emulator_autotrim` reads the pad's trim bounds straight out of the app's
memory, so it works with the release build (no logging needed) and only
requires a pad that has a sample. `emulator_spectral` and `emulator_reverb`
pin a demo fixture and say so when it is missing.

Run each scenario after a fresh `systemctl restart ac50` in the guest: while
SAMPLE OPTIONS is open the mod owns the panel, so a menu left open by a failed
run swallows the next test's keys (the symptom is an empty status
`Reverb: `).

Release builds are the default; a `MINIMOD_DEBUG=1` build enables the
investigation hooks (and is the only build that reads the debug environment
switches — see [`../mod/README.md`](../mod/README.md)). See
[`EMULATOR.md`](EMULATOR.md) for emulator setup.

## Repository notes

- [`MOD-REFERENCE.md`](MOD-REFERENCE.md): firmware anchors and reverse-engineering reference.
- [`AUTOTRIM-NATIVE.md`](AUTOTRIM-NATIVE.md): current AutoTrim path and measured evidence.
- [`CODE-REVIEW.md`](CODE-REVIEW.md): review of the earlier implementation; findings about property writes and the overlay are historical.
- [`PREFERENCES-TABS.md`](PREFERENCES-TABS.md) and [`TOUCH-PREFERENCES-WIRING.md`](TOUCH-PREFERENCES-WIRING.md): static TouchPreferences research. SHIFT+DATA activation has not been implemented.
