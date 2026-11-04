# Spectral Chop — investigation history

This is the chronological record: what was tried, what was disproved, and why
the answer took as long as it did. **The result, the model and the recipe are in
[SPECTRAL-CHOP-SOLVED.md](SPECTRAL-CHOP-SOLVED.md)** — read that first. The
per-step evidence lives in [`HYPOTHESES.csv`](../../HYPOTHESES.csv).

## Pass 1 — the wrong detector (commits up to `1c7c364`)

The first implementation opened the stock Chop view and selected its
**Threshold** type, then reported the detected count as if it had chopped. It
looked right because markers appeared — but they were the firmware's own
detector's positions, and the spectral positions were discarded. Once that was
noticed the action was reduced to a fail-safe: detect, then report
`Spectral: not applied` and change nothing.

An earlier variant that wrote slices through the **Extract** command was
removed: it restarted the app.

## Pass 2 — chasing the command API

Static work found the real slice commands (`Insert`/`Split`/`Remove`/`Remove
All` at `0x1a60af0` / `0x1a60d50` / `0x1a5d1c0` / `0x1a5cf40`) and the UI
wrapper `0x21a3b10`. All of them need an "editor" object carrying the sample at
`+0x4c8`. Three heap scans failed to find it, and calling Insert on the best
candidate faulted — which was read, wrongly, as "the command wants a different
sample object".

That pass produced a fail-safe and a set of anchors, but no write path.

## Pass 3 — the address-space mistake, found by reading the other docs

`HPSS-HANDOFF.md` and `docs/re/README.md` both state the rule: **Ghidra VA =
mod offset + `0x100000`**. The probe had been using Ghidra VAs directly, so
every hook was installed 0x100000 past its target and every "the hook never
fires" was an artifact. After subtracting `0x100000`:

* hooks fired immediately;
* `isSampleValid` on `layer+0x4a8` returned **1**, so the earlier fault was the
  address and not the object;
* Split and Merge were seen to fire **no** command and no dispatcher — only the
  marker resolver, the slice-list accessor and a slice-cell publisher.

That last observation is what redirected the work from the command API to
direct model edits.

## Pass 4 — hypothesis loop, and the two hints that cracked it

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

So `+0xd0` is the end boundary, `+0x78`/`+0x128` are the start and its mirror,
and the pads choose which slice the knobs edit.

The final unknown was why a direct write did not show up. Writing and
repainting produced a byte-identical screen; a pad press changed pixels but not
the markers. **Toggling CHOP changed the markers to exactly the written
frames** — the view had cached its layout, and re-entering is the rebuild
trigger. From there the recipe (grow → write → shrink → re-enter → verify) was
mechanical.

## Disproved along the way

These are worth keeping because each one cost a detour:

* the rebuild lives in the function containing `0x1a601d0` — it is an unwind
  cleanup;
* the editor is findable by scanning for `*(p+0x4c8) == sample` — zero shaped
  candidates, and it was never observed at all;
* a pad press inserts a slice through `0x21a3b10` — it only changes the
  selection;
* slices are allocated per edit through libc `malloc` — the app has its own
  allocator, so the mod must never create a Slice;
* `layer+0xb0…+0xf8` hold `SlicePoints` — those words are inline `std::string`
  data; the deserializer's layout belongs to a different class;
* `+0xd0` is a start frame — it is the end;
* K1/K2 are zoom controls — my own first test selected the wrong marker and
  produced that false conclusion; the user's correction fixed it.

## Tooling left behind

`mod/src/minimod.c` keeps a `#if MINIMOD_DEBUG` probe (compiled out of release
builds) with `/run/minimod.chop` commands for snapshots, memory read/write,
repaint, runtime vtables, heap scans, value searches and guarded command calls.
[SPECTRAL-CHOP-SOLVED.md §5](SPECTRAL-CHOP-SOLVED.md#5-reproducing-it) lists
them and the method for reading marker positions off the screen.
