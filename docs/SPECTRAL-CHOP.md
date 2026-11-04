# Spectral Chop

Spectral Chop slices the selected sample at detected onsets and puts the slices
on the next free pads. It lives in the same **SAMPLE OPTIONS** menu as HPSS:

```
SHIFT + DATA   open SAMPLE OPTIONS
TURN DATA      HPSS -> Spectral Chop -> Close
DATA           confirm
SHIFT + DATA   close
```

The source sample is never modified. The slices are normal native samples,
created and assigned by the stock Extract command.

## Analysis

`mod/src/spectral.c` implements:

| step | value |
|---|---|
| input | mono sum of the source channels, analysis only |
| STFT | 1024 samples, hop 512, symmetric Hann |
| onset function | normalised spectral flux, `sum(max(0, |X[f]| - |X[f-1]|))` |
| threshold | median of a ±0.5 s flux window × 1.8, floor 0.35 |
| peak pick | local maximum, at least 60 ms between onsets |
| limit | 16 strongest onsets, then sorted by position |
| lead-in | adds position 0 when the first hit is later than 50 ms |
| tail | ignores the artificial transient from end padding |

`tools/hpss/onsets.py` is the readable numpy reference. The two implementations
produce the same counts and positions on the emulator test loops.

## Slicing path

The stock Chop screen already has a **Threshold** chop type, but it exposes only
a threshold, not a list of positions. Spectral Chop therefore uses the existing
native Extract mechanism from HPSS:

1. detect the onset positions from the source WAV;
2. preflight that many free pads through the native occupancy predicate;
3. for each segment `[onset[i], onset[i+1])`, call stock Extract with that
   source range and no writer substitution — the native reader copies the
   original audio into a new sample;
4. let the stock command choose the pad, write the file and assign it.

The execute hook from HPSS is reused to patch the extraction range
(`screen+0x9ba0`, `+0x78` start and `+0xd0` end) for the duration of the
command's copy. The constructor hook supplies `Slice <run>-<n>` names. The run
token prevents the app's duplicate-name handling from collapsing several
slices onto one file when an earlier run already used `Slice 1`, `Slice 2`, ....

Slicing runs one command per UI paint, not in a blocking loop. Blocking the UI
through sixteen back-to-back extracts crashed the app; the state machine in
`spectral_start` / `spectral_step` / `spectral_finish` keeps the app responsive
and lets each command finish.

## Validation

Emulator, release build, firmware 1.3.0.12, `NRestarts=0`:

- 2 s drum loop: 10 slices, all ten files written, first pad `Slice 91010-1`;
- 7 s drum mix: 16 slices in the debug run;
- guitar loop: 7 slices, not 16 — sustained chords do not fill the pads;
- very short slice (1 536 frames): `Spectral: no onsets`;
- waveform-leading silence: first hit later than 50 ms is folded into a slice
  starting at 0;
- HPSS still creates its two layers after the range/execute refactor;
- AutoTrim still works (`SHIFT+B3`), and the encoder menu cycles
  HPSS → Spectral Chop → Close;
- slices are native samples on pads, selected pads show images such as
  `Slice 91010-1`, and the project reloads without a missing-sample dialog.

The host tests are:

```sh
python3 mod/tests/spectral_onsets.py
python3 mod/tests/hpss_dsp.py
```

`spectral_onsets.py` covers a five-click track, digital silence, a steady sine,
and a click track with a long lead-in.

## Limits

- The menu is the only entry point; `SHIFT+CHOP` is not bound (the user asked
  for the shared SAMPLE OPTIONS menu).
- At most 16 onsets are taken. If fewer pads are free, the operation refuses
  before writing anything.
- Tonal material is reduced to the strongest attacks by the floor; it is not
  classified as “no onsets” unless fewer than two peaks survive.
- The app writes the slice files; the mod does not create or delete files.
- Real hardware has not been tested yet.
