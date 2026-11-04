# Spectral Chop

Spectral Chop detects the onsets in the sample already loaded on the selected
pad, then opens the stock **Chop** view for that same pad.  It does **not**
create samples, does not use free pads, and never writes a file.  The pad keeps
its original sample; the stock Chop page owns the slicing from then on.

It lives in the same **SAMPLE OPTIONS** menu as HPSS:

```
SHIFT + DATA   open SAMPLE OPTIONS
TURN DATA      HPSS -> Spectral Chop -> Close
DATA           confirm
SHIFT + DATA   close
```

## What happens

1. The mod reads the selected pad's WAV and runs the onset detector.
2. If fewer than two onsets are found, it reports `no onsets` and changes
   nothing.
3. Otherwise it closes the menu, puts the same pad into the stock Chop view
   (the `CHOP` button), selects the stock **Threshold** chop type, and reports
   `<n> onsets - chopped on this pad`.

The sample, its name, its pad and every other pad stay as they were.  The
stock Chop page draws the slice markers on the existing waveform.

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

`tools/hpss/onsets.py` is the readable numpy reference.  The two
implementations produce the same counts and positions on the emulator test
loops.  The detected positions are logged for debugging; the stock Chop page
performs the actual cut.

## Native path

The stock Chop page is the only thing that touches the pad.  The mod uses the
app's own button dispatcher to press `CHOP` if the Chop view is not already
visible.  If the K3 knob has been seen, its last control-surface event is
replayed with value 0, which is the **Threshold** chop type.  No `Extract`
command, no free-pad search, no sample writer and no blocking loop are used.

This also removes the crash/restart that the old Extract-based path caused:
the app no longer receives sixteen back-to-back extract commands while the
UI is waiting.

## Validation

Emulator, firmware 1.3.0.12, `NRestarts=0`:

- pad `A03` with a 2 s drum loop: `10 onsets`, stock Chop view opens on A03,
  slice markers are drawn, no new pads, no new files;
- the same pad after forcing the Chop Type to a non-Threshold value: running
  Spectral Chop from the menu returns it to Threshold and the markers;
- very short slice / silence: `no onsets`, nothing is changed;
- no files named `Slice ...` are created;
- `SHIFT + DATA` still opens the same menu, and the encoder cycles
  HPSS → Spectral Chop → Close;
- HPSS and AutoTrim are unchanged.

Host tests:

```sh
python3 mod/tests/spectral_onsets.py
python3 mod/tests/hpss_dsp.py
```

`spectral_onsets.py` covers a five-click track, digital silence, a steady
sine, and a click track with a long lead-in.

## Limits

- The menu is the only entry point; `SHIFT+CHOP` is not bound.
- At most 16 onsets are reported.
- The stock Chop page decides the exact slice positions; the detector selects
  whether the material has usable onsets and reports the count.
- Real hardware has not been tested yet.
