# Task — Spectral Chop

> **Done and emulator-verified.** Entry point is the shared SAMPLE OPTIONS
> menu (not `SHIFT+CHOP`). The action chops the selected pad **in place** at the
> detected frames through the firmware's own slice API, verifies every boundary
> by reading it back, and does not create samples or use free pads.
> [`docs/SPECTRAL-CHOP.md`](../SPECTRAL-CHOP.md) covers the detector;
> [`docs/re/SPECTRAL-CHOP-SOLVED.md`](../re/SPECTRAL-CHOP-SOLVED.md) records the
> model, the mechanisms, the recipe and how it was established.
> The acceptance test is `mod/tests/emulator_spectral.py`, which reads the model
> out of `/proc/<pid>/mem` and asserts exact boundaries.

**One action: slice the selected sample at its detected onsets instead of at fixed divisions.**

> Outcome: **path A** (§Two implementation paths) worked. The stock chop does
> take positions — the slice container exposes `Reset`/`Insert`/`Reconcile` —
> so no slices are built by the mod and nothing outside the pad's own chop is
> touched.

Nothing else. No classification, no LED colours, no generated sequence. The stock
Chop still owns everything after the slicing: the slices land on the pads exactly
as they do today.

---

## The button

Proposed: **`SHIFT` + `CHOP`**. *(Today the entry point is
`SHIFT`+`DATA` → Spectral Chop; this section is the original proposal.)*

Before building, verify it is free — press it in stock Chop and see whether the app
already reacts (the `CHOP` key is `0x4b`; `SHIFT` is `0x31`). If it is taken, the
fallbacks in order of preference: `SHIFT` + `16 LEVELS` (`0x28`), a double-tap of
`CHOP`, or `CHOP` held for 400 ms while in Chop mode.

## Behaviour

1. User is on the Sample page, a loop is selected (the Chop source).
2. `SHIFT`+`CHOP` → the mod reads that sample's WAV, detects onsets, and asks for
   slices at those positions.
3. The pads fill with slices, exactly as a normal chop does.
4. A one-second toast reports the result: `17 slices` (or `no onsets` if the
   sample is silent / too short).
5. Nothing is persisted by the mod: if the app writes chop files, they are the
   app's files, written by the app's own code.

## Analysis

Operates on the same WAV the app has for that pad:

| step | detail |
|---|---|
| read | RIFF walker, mono sum of the channels for analysis only (slicing stays stereo) |
| frames | 1024 samples, hop 512, Hann |
| onset function | spectral flux: `Σ max(0, |X[f]| − |X[f−1]|)`, normalised |
| threshold | adaptive: median of a ±0.5 s window × 1.8, with a floor |
| peak pick | local maximum, minimum 60 ms between onsets |
| limit | first 16 onsets by strength; always include 0 if the first onset is later than 50 ms |
| output | sorted sample positions (integers) |

60 ms is the number that matters: below ~50 ms a single snare's two transients
split into two slices, above ~100 ms fast hats merge.

## Two implementation paths

**A. Drive the stock chop (preferred).** If the app can be told *where* to slice —
a slice list, or a "threshold" chop mode that keys off transients — the mod only
feeds it positions. No new audio is created by the mod, so no new write path is
needed.

Needs investigation, in this order:
1. how the Chop screen performs the cut (the code behind `CHOP`, `0x4b`);
2. whether the cut positions live in a list the mod could write, or are always
   computed from a division count;
3. whether a threshold-based chop type already exists in firmware 1.3.0.12 — if it
   does, the mod may only need to set its threshold from the detected onsets.

**B. Build the slices ourselves.** Only if A is impossible: cut the PCM at the
onset positions and add the slices to the sample pool. This is blocked on the
sample-pool write path — the same unknown that blocks every offline transform.

The task is done when path A works; path B is the fallback and is out of scope
since the write path exists.

**Outcome.** Path A. The stock chop takes positions through the slice
container's own API (`0x2950aa0` reset, `0x2950d20` insert-at-frame returning
the index, `0x2950bb0` reconcile ends), and the chop type is set to Manual
through the binding's model callback. Path B was never needed.

## Acceptance criteria

- [x] The menu entry on a drum loop fills the pads with slices that start on the hits.
- [x] The status reports the slice count, and `no onsets` for a silent sample.
- [x] A loop with a lead-in silence does not waste a slice on the silence (the
      detector adds position 0 only when the first hit is later than 50 ms).
- [x] Pressing it twice in a row gives the same slices (deterministic, no RNG);
      covered by the test's three repeat cases.
- [x] A tonal loop yields few slices or `no onsets` rather than nonsense.
- [x] The app does not crash, the pid does not change and `NRestarts` stays put;
      the mod writes no files of its own.
- [x] `SHIFT`+`B3` (AutoTrim) and the pitch keys still behave as before.
- [x] Every boundary is read back and compared with the detected frame before
      the action reports success.
- [x] The chop type ends as **Manual** on the same pad.
- [x] Re-entering the Chop view keeps the markers (the write goes through the
      native API, which broadcasts to the observers).

## Test plan

**Automated:** `python3 mod/tests/emulator_spectral.py --lab <lab> --boots 1` —
restarts the app, verifies the deployed library digest, reads the model from
`/proc/<pid>/mem` and asserts exact boundaries across first run, repeats,
Regions, Threshold, pad playback and Chop re-entry. It is the acceptance test of
record; the manual steps below remain useful for feel.

Emulator, release build, factory demo project:

1. Load the drum loop used for the HPSS tests into a pad.
2. Count the onsets with the Python reference (`tools/hpss` sibling script) and write
   the expected number down.
3. Press `SHIFT`+`CHOP`, screenshot the pad screen, compare the pad count and the
   toast against the expected number.
4. Play a few slices — each must start on a transient, no clipped attacks.
5. Repeat on a tonal loop and on a 0.3 s one-shot (edge cases: too short, no onsets).

## Risks

| risk | mitigation |
|---|---|
| Stock chop cannot take positions (path A fails) | decide before writing C: 1 session of RE on `0x4b` |
| Onset detector tuned on one loop fails on others | fixed thresholds, plus a `MINIMOD_CHOP_DBG=1` build that logs every onset position and its strength |
| 16-slice limit silently drops hits | toast shows `16 of 24` when the limit truncates |
| Slice at a non-zero crossing clicks | optional ±3 ms snap of each cut to the nearest zero crossing (cheap, we already scan PCM) |

## Effort

| piece | estimate |
|---|---|
| RE: how the stock chop cuts, can it take positions | 1 session |
| onset detector in C (with the Python reference to compare against) | 1 session |
| button binding, toast, edge cases | half a session |
| emulator test pass | half a session |

## Out of scope

Classification (kick/snare/hat), LED colours, renaming slices, generating a
sequence from the slices, and reordering the pads. If this lands, those become
separate tasks on top of working automatic slices.
