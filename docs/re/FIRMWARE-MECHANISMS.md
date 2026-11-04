# MPC Sample 1.3.0.12 — firmware mechanisms we found

A catalogue of the internal mechanisms this project reverse-engineered, how each
was established, and where the deeper write-ups are. It is a reference, not a
walkthrough.

## Conventions

* **mod offset** — what `FN()`, `install_hook()` and `mk` take.
  **Ghidra VA** = mod offset + `0x100000`. Runtime address = `base + offset`,
  where `base` is the module's load address read from `/proc/<pid>/maps`
  (`0xaaaaaaaa0000` in the lab; the binary is a PIE).
* Evidence markers used below:
  **[rt]** measured at runtime on the emulator, **[hook]** observed by a live
  hook, **[static]** decompiled only, **[doc]** established in a sibling
  investigation and recorded there.

## 1. Process and loader

| mechanism | detail | evidence |
|---|---|---|
| the firmware is one big PIE | `/usr/bin/MPC Sample`, ~120 MB, statically linked C++ runtime | [rt] |
| two address spaces | Ghidra VAs are `+0x100000` from mod offsets; mixing them silently patches the wrong function | [rt] |
| our code runs inside it | `LD_PRELOAD` an aarch64 shared object; `constructor` resolves `base` | [rt] |
| its own allocator | an `LD_PRELOAD` `malloc` hook never sees slice or sample allocation, so memory handed to the app by the mod could not be freed by it | [hook] |
| symbols are stripped | no `_Znwm`; runtime identity must come from offsets, vtables or an unstripped build linked separately | [rt] |

## 2. Screen, page and paint

| mechanism | detail | evidence |
|---|---|---|
| the main screen object | `SampleModeScreen`; vtable file offset `0x6e212a0`, checked on every access | [rt] |
| page visibility | `FN(0x112fd50)(object) != 0` — on the screen it answers "Sample page visible", on `screen+0x1ee0` it answers "Chop group visible" (= chop mode) | [rt] |
| paint hook | the paint method is hooked; it fires per repaint and is the only safe place to touch the model | [hook] |
| repaint | `FN(0x1149880)(screen, 0, 0)` forces one; it does **not** rebuild cached sub-views | [rt] |
| page switch is asynchronous | a CHOP press only *schedules* the switch; polling in the same callback cannot observe it | [rt] |
| view caching | the Chop view keeps its own slice layout; raw model writes repaint nothing, while the native API broadcasts and does | [rt] |

Detailed: [SPECTRAL-CHOP-SOLVED.md](SPECTRAL-CHOP-SOLVED.md).

## 3. Pad state and layers

```
controls + 0x60            pad state
state    + 0x22e8          std::vector<Layer>, stride 0xb30, 8 pads (0x5980 bytes)
layer    + 0x1a8 / +0x220  trim Start / End (float, knob stride 0x78)
layer    + 0x418           selected marker: 0x80 start, 0x81 end, else slice index
layer    + 0x4a8           the Sample
layer    + 0x4b0           refcounted companion of the sample
layer    + 0x6e0 bit 0     chop flag
state    + 0x60            command manager for the slice commands
```

The pad-index → layer path is `FUN_02968450` (integer argument is the pad index)
resolving through the program's pointer vector at `+0x2808` / `+0x2810`;
`FUN_02902e60` copies pad blocks and documents the `0xb30` stride; `FUN_0290dca0`
is the per-pad dispatch. Decompiled copies are in
[`docs/re/`](README.md). Evidence: [rt] for the layer offsets and stride,
[static] for the sync loop, [doc] for the pad-index path
([AUTOTRIM-NATIVE.md](../AUTOTRIM-NATIVE.md)).

## 4. The Sample

```
sample + 0x130   -> meta
meta   + 0     u16 channels
meta   + 4     u32 frames
meta   + 8     u32 sample rate
meta   + 0x10  the slice container
meta   + 0x70 / 0x78 / 0x80   vector<Slice*> begin / end / capacity
```

`FN(0x2943640)(sample, 1)` is the sample-validity predicate (returns 1 for a
live sample). Evidence: [rt].

## 5. The slice container and a Slice

```
Slice + 0x78    start frame   (mirror at +0x128)
Slice + 0xd0    end frame
adjacent slices share a boundary:  slice[i].+0xd0 == slice[i+1].+0x78
~0x340 bytes each, 16 slices maximum
```

| mechanism | call (mod offset) |
|---|---|
| reset the list | `0x2950aa0(cont)` |
| insert a slice at a frame | `0x2950d20(cont, frame, 1, frames)` → new index |
| reconcile adjacent ends and the tail | `0x2950bb0(cont, frames)` |
| select a slice | `0x16680a0(selection + 0x618, &index)` |

`FUN_02a50d20` constructs the Slice, registers its listeners
(`FUN_02a4b170`, `FUN_02a4a080`), notifies observers (`FUN_0102d0d0`), links it
into the container (`FUN_0246b140`) and calls the reconciler. The K1/K2 knobs
edit the same `+0x78`/`+0xd0` words for the slice a pad press selects.
Evidence: [rt] + [static]. Details: [SPECTRAL-CHOP-SOLVED.md](SPECTRAL-CHOP-SOLVED.md).

## 6. The chop-type binding

```
screen  + 0x168   chop parameter binding
binding + 0x9c    chop type, int   (Manual = 0; the list is at file 0x727f870)
binding + 0xc0    parameter / target block
binding + 0xd0    manager
binding + 0xd8    model callback (invoke)
binding + 0xe0    owner
binding + 0xe8    flags; bit 0 selects the fallback path
```

Set the type the way the stock UI does: `invoke(binding+0xc0, owner, &value)`
when bit 0 is clear and an owner exists, otherwise
`FN(0x15d2840)(binding+0x80, value)`. This is the path the stock Split handler
takes (Ghidra `0x1756728`); assigning the view's cached enum does not survive.
Evidence: [rt].

## 7. The observable property system

The firmware's generic "parameter changed" plumbing:

```
knob -> property object P -> 0x13bb930(P, value)
                              -> walks an intrusive list, calls
                                 listener->vtable[0x28] with (listener, value)
                                 -> parameter object -> page widget / model
```

* `0x13bb930` layout: `[P+0x10]` first listener node, `[P+0x08]` sentinel,
  node `+0x18` target (low 2 bits tagged), node `+8` next, `[P+0x50]` cached
  value.
* `0x14d0890` is a *different*, simpler property class:
  `if (value@0x20 == v) return; ++count@0x18; value@0x20 = v; for (node in
  list@0x08/@0x10) node->changed(v);`
* Broadcasting through `0x13bb930` really drives the app — a mis-identified
  broadcast once ran the stock **Normalize** command, which proves the mechanism
  while showing how easy it is to hit the wrong property.
* The Trim knobs do **not** go through `0x14d0890`; their property was
  identified but the exact owner was never pinned, which is why AutoTrim ended
  up dispatching the native command instead.

Evidence: [hook] (`lr` inside `0x13bb930`, listener call through vtable `0x28`),
[static] for both setters. Full record:
[TRIM-PAGE.md](../TRIM-PAGE.md) §4–§6.

## 8. The slice command / undo system

| command | ctor (Ghidra) | arguments |
|---|---|---|
| Insert Slice | `0x1a60af0` | `(controller, {sample, refcounted}, frame, editor)` |
| Split Slice | `0x1a60d50` | `(controller, {sample, refcounted}, index, editor)` |
| Remove Slice | `0x1a5d1c0` | `(controller, {sample, refcounted}, index, editor)` |
| Remove All | `0x1a5cf40` | `(controller, {sample, refcounted}, editor)` |
| UI wrapper | `0x21a3b10` | `(editor, {sample, refcounted}, frame)` |
| add at playhead | `0x21a9fa0` | `(editor)` — uses `editor+0x848` |
| dispatcher | `0x21a7140` | `(editor, msg, kind)` — reads the marker at `editor+0x528` |

The "editor" object they expect carries `+0x4c8` sample, `+0x4d0` selection,
`+0x528` marker, `+0x848` playhead, `+0x10c8` command controller. **It was never
observed at runtime**, and the Chop UI does not use these commands at all: with
every ctor hooked, Split and Merge fire none of them and edit the container
directly. The commands are an undoable API for paths outside this view.

Undo labels and telemetry keys are useful anchors for finding them:
`"Insert Slice"` at file `0x46a80e0`, the key `"slices_added"`, and
`"chopMinSliceTime"` at `0x469ecf0`. Evidence: [rt] (hooks) + [static].

## 9. The control surface and input path

| mechanism | detail | evidence |
|---|---|---|
| raw stream | the controller's events are 32-byte records on `/dev/snd/midiC1D0` with the MIDI byte at `+16` | [doc] |
| `read()` interception | the mod's `read()` wrapper sees every event and can drop specific messages (used to suppress input while a menu or an operation is active) | [rt] |
| button dispatcher | `buttons(event)`; `event+8` = code, `event+9` = `0x7f` down / `0x00` up | [rt] |
| control-change path | `0x159f230(event, raw, lo, range)`; `event+8` = CC number | [rt] |
| encoders | relative where noted: `01` = +1 step, `7F` = −1; an unchanged absolute value may issue no event at all | [doc] |
| button codes | `CHOP 0x4b`, `SHIFT 0x31`, screen buttons `0x0c/0x0d/0x0e` (= B1/B2/B3), `B3 0x0e` repeats while held | [rt] |
| panel feedback | lab HTTP `/state?since=0`: LED 75 = CHOP bright, LED 82 = PLAY | [rt] |
| input suppression | dropping the three `B0` reads stops the page from seeing a knob while the mod owns the interaction | [rt] |

## 10. Sample pool, pad occupancy and Extract

| mechanism | call (Ghidra VA) | evidence |
|---|---|---|
| first empty pad (0–127, `-1` if full) | `0x296dd00` | [doc] |
| pad occupancy predicate | `0x29047b0` | [doc] |
| stock Sample/Chop Extract handler | `0x17580d0`, argument is the Sample screen | [doc] |
| `ExtractAndAssignSampleCommand` ctor / execute | `0x153ed40` / `0x153f0b0` | [doc] |
| extraction writer callback / concrete writer | `0x1e8aff0` / `0x1e93570` | [doc] |
| extraction settings object | `screen + 0x9ba0`; range start `+0x78`, end `+0xd0` (exclusive) | [doc] |

The pattern that works there, and the reason HPSS needs no guessed model writes:
**capture the native command, patch only the values it is about to copy, let the
firmware keep ownership of files, history and pad assignment.** Details:
[HPSS-HANDOFF.md](../../HPSS-HANDOFF.md).

## 11. Persistence and serialization

* Pad/sample serialization is a separate class from the runtime layer. The
  deserializer (`0x1e02230`) stores `SlicePoints` as `{double timeInSeconds,
  float, double energy}` (0x18 bytes) in `vector`s — Threshold / Regions / Beat /
  Manual variants — but **those are not at `layer+0xb0…`**: on the live layer
  those words are inline `std::string` data. Reading them as SlicePoints is
  wrong. [static] + [rt]
* A manual chop therefore has a persisted form we have not exercised. The pad
  path `FUN_02968450` resolves pads through `+0x2808` / `+0x2810`. [static]
* Project state lives in `/data/TempProjects/...`; the emulator restores it on
  restart, so transient model edits do not survive a restart unless the app
  saves them. [doc]

## 12. Threads and ownership

| thread | what runs there | rule |
|---|---|---|
| control-surface / input | `buttons()`, `cc_capture()`, `read()` wrapper | must not touch the model; publish a request only |
| UI / paint | `screen_paint()`, after the stock paint | the only place model calls are safe |
| worker | HPSS DSP and file I/O | never touches the model; commits through the native command on the UI thread |
| audio | playback; calls the marker resolver for the slice it plays | observe only |

Cross-thread hand-off is an atomic flag plus a phase machine, one phase per
paint pass. Evidence: [rt] — blocking the input thread and polling for a page
switch in the same callback both failed.

## 13. Constants and limits

| limit | value | source |
|---|---|---|
| slices per sample | 16 | [rt] (Split refused at 16) |
| onsets we apply | 16, truncated by strength | feature choice |
| chop types | Threshold, Regions 4, Regions 8, Regions 16, Manual — list at file `0x727f870` | [rt] |
| minimum slice length | `chopMinSliceTime` at file `0x469ecf0` (value not measured) | [static] |
| marker sentinels | `0x80` start, `0x81` end, else slice index | [rt] |
| undo | slice edits are not undoable in this view | [rt] |

## 14. How these were found

The instrumentation that produced all of the above is catalogued in
[SPECTRAL-CHOP-SOLVED.md §4](SPECTRAL-CHOP-SOLVED.md#4-mechanisms-catalogue):
hooking by entry patch with a trampoline, `FN()` calls by offset,
`/proc/self/mem` reads and writes, structured logging to `/run/minimod.log`, a
runtime command channel (`/run/minimod.chop`) executed on the UI thread,
`mprotect` page watches, accessor-caller logging, whole-object snapshot diffing
driven by a human at the panel, an allocator probe, `findval`/`findq` value and
double searches, runtime vtable dumps, and a host-side harness that reads the
model out of `/proc/<pid>/mem` and asserts exact values.

## 15. Still unknown

* where Threshold mode takes its slice positions from (the algorithm is native
  and was not needed once the insert API was found);
* the property object that owns a given `TrimParameter` (§7) — identified as a
  concept, never pinned to an instance;
* whether a manual chop is persisted through the deserializer's `SlicePoints`
  and how it reloads (§11);
* the value and units of `chopMinSliceTime`;
* behaviour on real hardware — everything here is emulator-verified.
