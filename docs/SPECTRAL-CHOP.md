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

1. The request is published from the control-surface thread; all model work
   happens on the UI thread, one phase per paint pass.
2. The mod resolves the selected pad's sample (the same chain HPSS uses) and
   reads its WAV.
3. `spectral_onsets()` returns up to 16 frame positions.
4. Fewer than two onsets: `no onsets`, nothing changes.
5. Otherwise the pad is chopped in place:
   * enter the stock Chop view if it is not already active;
   * select **Manual** through the chop binding's own model callback;
   * **reset** the slice list, **insert** one slice per detected frame (the
     native insert allocates, links listeners and broadcasts), **reconcile**
     the adjacent ends and the tail, and put the selection on slice 0;
   * read every boundary back and only then report `n slices`.
6. The menu reports `Spectral: n slices on this pad`. A refusal says why
   (`no sample on this pad`, `cannot read sample WAV`, `no onsets`,
   `cannot select Manual chop`, `native marker write failed`, …) and never
   claims success.

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

The live chop is the sample's slice container, not the pad's serialized
`SlicePoints`:

```
sample + 0x130  ->  meta { u16 channels, u32 frames, u32 rate }
meta   + 0x70 / 0x78 / 0x80   vector<Slice*> begin / end / capacity
meta   + 0x10                 the slice container the native API takes

Slice + 0x78    start frame   (mirror at +0x128)
Slice + 0xd0    end frame
```

The firmware's own API does the editing — `0x2950aa0` reset, `0x2950d20`
insert-at-frame (returns the new index), `0x2950bb0` reconcile ends, `0x16680a0`
select a slice. Because those own allocation and the observer broadcast, the
Chop view updates itself: no field pokes and no forced rebuild.

Moving a boundary by hand with K1/K2 edits the same words for the slice a pad
press selects, which is how the layout was confirmed.

The full model, the mechanisms and the evidence:
[`docs/re/SPECTRAL-CHOP-SOLVED.md`](re/SPECTRAL-CHOP-SOLVED.md).

## Validation

`mod/tests/emulator_spectral.py` is the acceptance test. It restarts the app,
checks the deployed library's digest, locates the module base and the
`SampleScreen` through its vtable, reads the chop model straight out of
`/proc/<pid>/mem`, then runs the real menu action and compares **exact**
boundaries:

```
PASS boot 1: first from Trim; exact 10 boundaries, Manual, pid=13416
PASS boot 1: repeat / repeat again
PASS boot 1: regions / threshold / after pad playback
PASS: menu, model, repeat, modes, playback, re-entry and restart checks
```

The fixture (`A03`, `Drums 2s`, 88200 frames) must produce
`[0, 1536, 12288, 24064, 36352, 48128, 60416, 67584, 72192, 84480]` with the
last slice ending at 88200.

```sh
./mod/build.sh
./mod/pack.sh
python3 mod/tests/emulator_spectral.py --lab ~/projects/customs/virtual-mpc-sample --boots 1
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
