# Spectral Chop

> **Not finished.** The detector works and is verified, but the detected
> positions are **not** written to the pad. Picking **Spectral Chop** reports
> `Spectral: not applied` and changes nothing. An earlier attempt drove the
> stock **Threshold** chop type instead, which sliced at the firmware's own
> positions — not ours — so it was removed.
> [`re/spectral-chop-handoff.md`](re/spectral-chop-handoff.md) has the native
> slice API, the verified anchors and the exact remaining work.

Spectral Chop reads the sample already loaded on the selected pad and runs an
onset detector on it. It does **not** create samples, does not use free pads
and never writes a file. It lives in the same **SAMPLE OPTIONS** menu as HPSS:

```
SHIFT + DATA   open SAMPLE OPTIONS
TURN DATA      HPSS -> Spectral Chop -> Close
DATA           confirm
SHIFT + DATA   close
```

## What happens today

1. The mod resolves the selected pad's sample (the same chain HPSS uses) and
   reads its WAV.
2. `spectral_onsets()` returns up to 16 frame positions.
3. Fewer than two onsets: `no onsets`, nothing changes.
4. Otherwise the menu reports `Spectral: not applied` and the pad is left
   exactly as it was. `MINIMOD_CHOP_DBG=1` additionally logs every position.

The detection half is real and testable; the writing half is what is missing.

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

`tools/hpss/onsets.py` is the readable numpy reference; the two produce the
same counts and positions on the test loops. Positions are sample frames in
the source WAV, i.e. exactly the unit the native slice API takes.

## Why the markers are not written yet

The stock model keeps live slices as objects in the sample, not as a list of
numbers: `*(sample+0x130)+0x10` holds a vector of slice pointers at
`+0x60/+0x68`, and each slice carries its start frame at `+0xd0`. The app only
mutates that list through undoable commands:

| command | function | arguments |
|---|---|---|
| Insert Slice | `0x1a60af0` | `(controller, {sample, refcounted}, frame, editor)` |
| Split Slice | `0x1a60d50` | `(controller, {sample, refcounted}, index, editor)` |
| Remove Slice | `0x1a5d1c0` | `(controller, {sample, refcounted}, index, editor)` |
| Remove All Slices | `0x1a5cf40` | `(controller, {sample, refcounted}, editor)` |

The one-stop wrapper the UI itself uses is `0x21a3b10`
`(editor, {sample, refcounted}, frame)`; it also emits the `slices_added`
telemetry. Every one of them needs the **editor object** — a long-lived
object that owns the selected sample at `+0x4c8`, the refcounted selection at
`+0x4d0`, the selected marker at `+0x528` (`0x80` = start, `0x81` = end) and
the command controller at `+0x10c8`.

That editor is the blocker. It is not reachable from the screen, the pad
binding or the layer, and a heap scan for `*(p+0x4c8) == sample` yields
objects that are not it: calling Insert Slice on the best candidate faults
inside `0x2a43640` while validating the sample, so the object the command
wants is not the object `layer+0x4a8` gives us. Switching the pad to the
**Manual** chop type works (K3 control-surface value `127`), and the stock
Manual workflow is "tap pads along with the music", but neither could be made
to emit an Insert Slice command from our side.

## Validation

Host tests (all passing):

```sh
python3 mod/tests/spectral_onsets.py
python3 mod/tests/hpss_dsp.py
python3 mod/tests/wav_edges.py
```

`spectral_onsets.py` covers a five-click track, digital silence, a steady sine
and a click track with a long lead-in.

Emulator, firmware 1.3.0.12: the menu entry detects and reports, the pad is
unchanged (`NRestarts=0`), HPSS and AutoTrim are unaffected. No claim about
markers is made because none are written.

## Limits

- The menu is the only entry point; `SHIFT+CHOP` is not bound.
- At most 16 onsets are reported.
- Real hardware has not been tested.
