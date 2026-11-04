# Spectral Chop

Spectral Chop reads the sample already loaded on the selected pad, detects its
onsets, and **rewrites that pad's chop so every slice boundary sits on a
detected frame**. It does not create samples, does not use free pads and never
writes a file. It lives in the same **SAMPLE OPTIONS** menu as HPSS:

```
SHIFT + DATA   open SAMPLE OPTIONS
TURN DATA      HPSS -> Spectral Chop -> Close
DATA           confirm
SHIFT + DATA   close
```

## What happens

1. The mod resolves the selected pad's sample (the same chain HPSS uses) and
   reads its WAV.
2. `spectral_onsets()` returns up to 16 frame positions.
3. Fewer than two onsets: `no onsets`, nothing changes.
4. Otherwise the pad is chopped in place:
   * enter the stock Chop view;
   * grow the slice list to the onset count by splitting (the stock limit is
     16 slices);
   * write each slice's start and end frame from the detector;
   * shrink the list to exactly that many slices;
   * leave and re-enter Chop so the view rebuilds from the model;
   * read every boundary back and only then report `n slices`.
5. The menu reports `Spectral: n slices on this pad`. On a refusal (for
   example a pad whose slices could not be split to the needed count) it says
   so and does not claim success.

## Analysis

`mod/src/spectral.c` implements:

| step | value |
|---|---|
| input | mono sum of the source channels, analysis only |
| STFT | 1024 samples, hop 512, symmetric Hann |
| onset function | normalised spectral flux, `sum(max(0, \|X[f]\| - \|X[f-1]\|))` |
| threshold | median of a ±0.5 s flux window × 1.8, floor 0.35 |
| peak pick | local maximum, at least 60 ms between onsets |
| limit | 16 strongest onsets, then sorted by position |
| lead-in | adds position 0 when the first hit is later than 50 ms |
| tail | ignores the artificial transient from end padding |

`tools/hpss/onsets.py` is the readable numpy reference. Positions are sample
frames in the source WAV — exactly the unit the chop model uses.

## The chop model

The live chop is the sample's slice list, not the pad's serialized
`SlicePoints`:

```
slice list = *(*(sample+0x130)+0x10)   vector<Slice*> at +0x60 / +0x68
Slice + 0x78   start frame  (mirrored at +0x128)
Slice + 0xd0   end frame
```

Moving a slice boundary with K1/K2 edits exactly those words, and the pads
select which slice the knobs act on. Writing them directly works, but the Chop
view keeps its own cached layout: a repaint or a pad press does not rebuild it.
Leaving and re-entering Chop does, and in Manual mode the boundaries survive
that rebuild — which is what the feature relies on.

## Validation

Emulator, firmware 1.3.0.12, release build, `NRestarts=0`:

- pad `A03`, 2 s drum loop, 10 onsets: 10 slices written, every end frame read
  back and matched, markers at x = 18, 54, 93, 133, 172, 213, 237, 252 and 293
  px of a 320 px waveform (one per detected onset plus the sample end);
- a synthetic run with frames 500 / 3000 / 7000 / 11000 produced markers at
  x = 23, 71, 148, 225, which is those frames on the same scale;
- a pad with fewer slices than onsets is refused with a message instead of a
  half-written chop;
- HPSS and AutoTrim are unchanged.

Host tests:

```sh
python3 mod/tests/spectral_onsets.py
python3 mod/tests/hpss_dsp.py
python3 mod/tests/wav_edges.py
```

## Limits

- The menu is the only entry point; `SHIFT+CHOP` is not bound.
- At most 16 onsets — the stock chop limit.
- The pad's previous chop is replaced. Stock slice edits are not undoable
  either, so this matches the firmware's own behaviour.
- Real hardware has not been tested.
