# Spectral Chop — investigation history

The chronological record: what was tried, what was disproved, and what finally
worked. **The result, the model, the mechanisms and the recipe are in
[SPECTRAL-CHOP-SOLVED.md](SPECTRAL-CHOP-SOLVED.md)** — read that first. The
numbered evidence is in [`HYPOTHESES.csv`](../../HYPOTHESES.csv).

## Pass 1 — the wrong detector

The first implementation opened the stock Chop view and selected its
**Threshold** type, then reported the detected count as if it had chopped. It
looked right because markers appeared — they were the firmware's own detector's
positions and ours were discarded. It was reduced to a fail-safe: detect, then
report `Spectral: not applied` and change nothing.

An earlier variant that wrote slices through the **Extract** command restarted
the app and was removed.

## Pass 2 — chasing the command API

Static work found the real slice commands (`Insert`/`Split`/`Remove`/`Remove
All` at `0x1a60af0` / `0x1a60d50` / `0x1a5d1c0` / `0x1a5cf40`) and the UI
wrapper `0x21a3b10`, all of which need an "editor" object carrying the sample
at `+0x4c8`. Three heap scans failed to find that object, and calling Insert on
the best candidate faulted — which was read, wrongly, as "the command wants a
different sample object". No write path.

## Pass 3 — the address-space mistake

`HPSS-HANDOFF.md` and `docs/re/README.md` both state the rule: **Ghidra VA =
mod offset + `0x100000`**. The probe had been using Ghidra VAs directly, so
every hook sat 0x100000 past its target and every "the hook never fires" was an
artifact. After subtracting `0x100000`:

* hooks fired immediately;
* `isSampleValid` on `layer+0x4a8` returned **1**, so the earlier fault was the
  address, not the object;
* Split and Merge were seen to fire **no** command and no dispatcher — only the
  marker resolver, the slice-list accessor and a slice-cell publisher.

That last observation redirected the work from the command API to the model
itself.

## Pass 4 — the human in the loop

Two remarks from the user did more than any probe:

> "the knob 1 or 2 move boundaries of selected chop … you select given chop by
> pressing a pad"

> "split button works only when chop type is set to manual"

They became one experiment: snapshot **every word of every slice** on every
button press and knob turn, and have the user perform the gesture in the panel.
The diff was unambiguous:

```
cc10 (K1):  slice0 +0xd0  2465 -> 4906   (+ mirrors on slice1)
cc11 (K2):  slice1 +0xd0  4907 -> 5143 -> 5295 -> 6051
```

`+0xd0` is the end boundary, `+0x78`/`+0x128` the start and its mirror, and a
pad press chooses which slice the knobs edit.

## Pass 5 — the first working chop (direct writes, shipped in `9b299b9`)

Writing the boundaries, shrinking the container's vector and re-entering Chop
produced the first correct chop: 10 onsets became 10 slices at the detected
frames, verified by reading every end frame back.

The reason a rebuild was needed at all: the Chop view caches its layout, and a
raw field write broadcasts nothing, so neither a repaint nor a pad press
redraws the markers.

## Pass 6 — the native API replaces the writes

The container has its own API, and using it is both simpler and correct:

| purpose | mod offset | effect |
|---|---|---|
| reset | `0x2950aa0` | clears the list |
| insert at a frame | `0x2950d20` | allocates, links listeners, notifies, returns the index |
| reconcile ends | `0x2950bb0` | fixes adjacent ends and the tail |
| select a slice | `0x16680a0` | moves the selection |
| Manual chop type | binding `+0xd8` callback | preserves the pad's chop type |

Everything hand-rolled then disappeared: no vector shortening, no synthetic
Split presses, no field pokes, no CHOP toggle. The action also moved onto the
**UI thread** as a phase machine (the input hook only publishes a request),
gained control-surface suppression while it runs, and got an emulator
acceptance test that reads the model independently and asserts exact
boundaries across repeats, modes, playback and re-entry.

This is the shipped implementation.

## Disproved along the way

Each of these cost a detour:

* the rebuild lives in the function containing `0x1a601d0` — it is an unwind
  cleanup;
* the editor is findable by scanning for `*(p+0x4c8) == sample` — never
  observed at all;
* a pad press inserts a slice through `0x21a3b10` — it only moves the selection;
* slices are allocated per edit through libc `malloc` — the app has its own
  allocator, so the mod must never allocate a Slice;
* `layer+0xb0…+0xf8` hold `SlicePoints` — those words are inline `std::string`
  data;
* `+0xd0` is a start frame — it is the end;
* K1/K2 are zoom controls — a false conclusion from a test that had selected the
  start/end marker instead of a slice;
* direct writes plus a rebuild are the way to apply a chop — they work, but the
  native API is correct and needs no rebuild.

## Tooling left behind

`mod/src/minimod.c` keeps a `#if MINIMOD_DEBUG` probe (compiled out of release)
with `/run/minimod.chop` commands for snapshots, memory read/write, repaint,
runtime vtables, heap scans, value searches, an allocator probe and a page
watch. [SPECTRAL-CHOP-SOLVED.md §4 and §6](SPECTRAL-CHOP-SOLVED.md) catalogue
them and the verification harness.
