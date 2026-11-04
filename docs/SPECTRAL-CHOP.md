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
| onset function | spectral novelty: the spectrum is split at 250 Hz and 2 kHz, each band's positive log-magnitude flux is divided by that band's mean, and the three are summed |
| threshold | `delta + 2.5 × median(±0.5 s)`, with `delta = 0.1 × mean novelty` so a quiet passage cannot fire on noise |
| peak pick | local maximum, at least 40 ms between onsets, and at least 0.8 × mean novelty above the valley since the previous onset (so a loud hit's tail is not a second hit) |
| limit | the **first 16** onsets in time order; everything later stays inside the last slice |
| position | the boundary goes where the 1 ms envelope passes 15% of the local transient peak — on the attack, not in the silence before it |
| tail | ignores the last analysis frames, whose zero padding looks like a transient |

`tools/hpss/spectral_chop.py` is the readable reference and the tuning bed;
`tools/hpss/onsets.py` still describes the previous (shipped-before) detector.
Positions are sample frames in the source WAV — exactly the unit the chop model
uses. On a 4 s break this is 16 slices with a 180 ms median, most of them
within 1.4 ms of the visible start of a hit.

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
