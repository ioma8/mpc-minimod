# Spectral Chop: investigation handoff

## Goal

From **SAMPLE OPTIONS → Spectral Chop**, detect onsets in the selected pad's
sample and apply those exact frame positions as stock **Manual** chop markers
on that same pad. Do not extract files or assign other pads. Report the number
of slices, or `no onsets` without changing the pad. Keep HPSS, AutoTrim and
pitch behavior intact.

**State: finished and emulator-verified.** Detection works and the detected
frames are written into the pad's own chop; every boundary is read back before
the action reports success. Everything below that is marked *verified* was
measured on the emulator, not inferred.

## The solve (read this before the history)

The live chop is the sample's slice list, and the boundaries are plain words:

```
slice list = *(*(sample+0x130)+0x10)      vector<Slice*> at +0x60 / +0x68
Slice + 0x78    start frame  (mirror at +0x128)
Slice + 0xd0    end frame
```

K1/K2 move exactly those words for the slice selected by a pad press. Writing
them works, but the Chop view caches its layout: repaint and pad presses do not
rebuild it, while **leaving and re-entering Chop does**, and in Manual mode the
boundaries survive that rebuild. So the shipped recipe is:

1. enter the Chop view;
2. grow the slice list to the onset count by synthesising SHIFT+SCREEN 2
   splits (stock limit 16);
3. write `+0x78`/`+0x128` (start) and `+0xd0` (end) for each slice;
4. shrink the vector: `*(list+0x68) = begin + 8*n`;
5. toggle CHOP off and on so the view rebuilds from the model;
6. read every `+0xd0` back and only then report.

`mod/src/minimod.c` implements this in `spectral_write_markers()`. Verified:
`Drums 2s`, 10 detected onsets -> 10 slices, all end frames matched, markers at
x = 18/54/93/133/172/213/237/252/293 px; a synthetic run with 500/3000/7000/
11000 landed at x = 23/71/148/225. Release build, `NRestarts=0`.

Everything below is the history of how that was found. The first two passes
contain conclusions that were later disproved; they are kept because the
mistakes are instructive, and each is marked.

## What is verified now

### The live slice model (verified at runtime)

Slices are objects in the sample, not a list of numbers:

```
sample            = *(layer + 0x4a8)              // the object the mod already resolves
sample + 0x130    -> buffer info {u16 channels; u32 frames; u32 rate}
*(sample+0x130) + 0x10            -> "SliceList" object   (vtable file off 0x6dbd9e8)
SliceList + 0x60 / +0x68 / +0x70  -> std::vector<Slice*> begin / end / cap
Slice + 0xd0                      -> slice start frame (u64)
```

A dump of a pad with the stock **Regions 16** chop showed 16 slices with
`+0xd0 = 0, 945, 1890 … 15128` on a 15129-frame kick — i.e. `+0xd0` is the
start frame. `0x2a41ff0(sample)` returns the SliceList; `0x2a41f80(sample)`
returns the frame count and matches `*(u32*)(*(u64*)(sample+0x130)+4)`, so the
object at `layer+0x4a8` really is the `Sample` those helpers take.

### The layer (verified at runtime)

`state + 0x22e8` is a `std::vector<Layer>` with **stride 0xb30**
(`0x2904ae0` walks `+0x22e8 .. +0x22f0` by 0xb30). A Layer carries

| offset | meaning |
|---|---|
| `+0x418` | selected marker index (`0x80` = start marker, `0x81` = end marker) |
| `+0x4a8` | the `Sample` |
| `+0x4b0` | refcounted helper (refcount at +8) |
| `+0x740` | the "end marker" pseudo-slice |
| `+0xa8` | `SlicingStyle` (int) |
| `+0xb0/+0xc8/+0xe0/+0xf8` | `SlicePoints`, `ManualSlicePoints`, `RegionSlicePoints`, `BeatSlicePoints` as `std::vector<SlicePoint>` |

`SlicePoint` is 0x18 bytes: `{double timeInSeconds; float energy; double
normalizedEnergy;}` (parsed by `0x1e01e20`, printed by `0x1e02dc0`, both
siblings of the `0x1e02230` deserializer). **Those four vectors are empty in a
live session** even when the sample has 16 slices, so they are the persisted
form, not the live model. Do not treat them as the thing to write.

### The native slice commands (verified statically, argument shapes confirmed)

All are controller methods that build an undoable command and push it to the
command manager at `controller+0x60`:

| command | entry | arguments |
|---|---|---|
| Insert Slice | `0x1a60af0` | `(controller, {sample, refcounted}, frame, editor)` |
| Split Slice | `0x1a60d50` | `(controller, {sample, refcounted}, index, editor)` |
| Remove Slice | `0x1a5d1c0` | `(controller, {sample, refcounted}, index, editor)` |
| Remove All Slices | `0x1a5cf40` | `(controller, {sample, refcounted}, editor)` |

`{sample, refcounted}` is a two-qword struct: `sample` is validated with
`0x2a43640(sample, 1)`; the second word is a `ReferenceCountedObject` pointer
(refcount at +8) that the command copies with `0x14b35c0`.

The UI's own one-stop wrapper is **`0x21a3b10(editor, {sample, refcounted},
frame)`**: it calls Insert Slice, then emits the `slices_added` /
`chop_mode=Manual` / `num_slices` telemetry and notifies the editor. The 4th
argument of Insert Slice is the editor itself (stored at command `+0x78`).

Two other entry points on the editor:

- `0x21a9fa0(editor)` — insert a slice at the playhead (`editor+0x848`, u32).
- `0x21a6d20(editor, frame)` — insert a slice at a given frame (drag end).

`0x21a7140(editor, msg, 1)` is the dispatch the UI uses: it reads the selected
marker at `editor+0x528`; `0x80`/`0x81` mean "insert a slice", anything else
means "split that slice index".

### The editor object (the blocker)

Layout measured from the code above: `+0x58` int, `+0x3b0/+0x3b8` lock and
counter, `+0x4c8` selected `Sample*`, `+0x4d0` refcounted selection,
`+0x528` selected marker index, `+0x5c8` pointer, `+0x848` playhead frame,
`+0xa50` int, `+0x10a8` pointer, `+0x10c8` the command controller,
`+0x11e8` pointer, `+0x1484` counter. It is at least 0x1490 bytes.

It is **not** reachable from the objects the mod already holds:

- scanning the screen object, the pad binding and the state (0x12000 bytes
  each) for a pointer `p` with `*(p+0x4c8) == sample` finds nothing;
- a full heap scan for `*(p+0x4c8) == sample` finds 8 objects. Adding the
  editor's own shape to the predicate (`+0x4d0` a heap pointer, `+0x10c8` and
  its command manager non-null) leaves **zero** candidates. The one loose
  candidate that looked right (`+0x528 == 0x81`) has a string where the
  selection should be (`0x4c6c696154222020` = `"  Tail l"`) and faults inside
  `0x2a43640` when Insert Slice is called on it. So no live object outside the
  editor holds this sample — which also means the sample pointer the commands
  want is not simply `layer+0x4a8`;
- `0x2a43640(sample, 1)` on the sample from `layer+0x4a8` itself faults
  (`+0x170`/`+0x178` do not look like the expected shared_ptr pair), so the
  command path wants a sample object that is bound to the audio engine, which
  is not the same handle `layer+0x4a8` gives us. **This is the crux: find how
  the editor's `+0x4c8` sample is obtained, then the same route yields the
  editor.**

### Chop types (verified at runtime)

The Chop Type knob is `K3` (control-surface CC 18). Absolute values select the
list at `0x727f870`: `?`, `?`, `Threshold`, `Regions 4`, `Regions 8`,
`Regions 16`, `Manual`. Sending `K3 = 127` switches the pad to **Manual** (the
screen then shows `Start`, `End`, `Manual` and drops to a single slice);
`K3 = 32/64/96` produce `Regions 4/8/16`. This answers the old open question
"how to switch to Manual without a captured K3 event": send CC 18 = 127
through the same control-surface path the mod already uses (`0x159f230`).

The stock map in the lab (`virtual-mpc-sample/stock-map/index.html`) documents
the rest: Manual is "tap pads along with the music, up to 16 slices";
`SHIFT`+`SCREEN 1/2/3` are Extract / Split / Merge, `ERASE`+pad removes a
slice, and slice edits are not undoable in the stock UI.

## What was tried and rejected

- **Threshold chop type** (commits up to `1c7c364`): the firmware's own
  detector picks the positions, so the detected ones are discarded. Removed.
- **Calling Insert Slice on a heap-scanned editor candidate**: faults. The
  probe now wraps every such call in `sigsetjmp` (`g_cc_recover`) so a wrong
  candidate cannot take the app down — `NRestarts` stayed 0 through the
  experiments.
- **Pad taps in Manual mode**: pads select pads outside the Chop view, and
  inside it a tap plays a slice; no Insert Slice command was observed either
  from a tap or from `ERASE`+pad.
- **mprotect watchpoint on the slice vector**: page-granular, and the first
  write after arming is on an unrelated object on the same page, so it does
  not isolate the vector writer.

## Probe tooling in the tree

`mod/src/minimod.c` has a `#if MINIMOD_DEBUG` block ("chop probe") that is
compiled out of release builds (release size is unchanged). It hooks
`0x21a3b10`, `0x21a9fa0`, `0x1a60af0`, `0x1a5cf40`, `0x1a5d1c0` and
`0x21aa9e0`, and runs commands written to `/run/minimod.chop` **on the UI
thread** (from the button hook, never from the probe thread):

| command | effect |
|---|---|
| `dump` | slice count and slice list of the current sample, layer style and vectors, candidate offsets |
| `count` | slice count only |
| `vt [n]` | runtime vtable of the current sample, as file offsets |
| `vl [n]` | runtime vtable of the slice list |
| `scan` | full-heap scan for a `+0x4c8`-owns-the-sample candidate |
| `find` | same predicate inside screen / state / binding |
| `samp` | validity probe of the current sample object |
| `wvec` | arm a write watch on the slice vector |
| `addx <ed> <frame>` | guarded Insert Slice with an explicit editor |
| `rmallx <ed>` | guarded Remove All Slices with an explicit editor |

Build and use:

```sh
zig cc -target aarch64-linux-gnu -shared -fPIC -O2 -fno-omit-frame-pointer \
  -mbranch-protection=standard -DMINIMOD_LOG=1 -DMINIMOD_DEBUG=1 \
  -o mod/build/minimod-dbg.so mod/src/minimod.c mod/src/hpss.c mod/src/spectral.c
python3 ../virtual-mpc-sample/lab.py preload $PWD/mod/build/minimod-dbg.so
python3 ../virtual-mpc-sample/lab.py ssh 'echo dump > /run/minimod.chop'
# then press any button (SHIFT is harmless) to run it on the UI thread
python3 ../virtual-mpc-sample/lab.py ssh 'grep "^chop:" /run/minimod.log'
```

Panel control (drives the UI from the host):

```sh
curl -s -X POST http://127.0.0.1:8765/control -d '{"name":"CHOP","action":"press"}'
curl -s -X POST http://127.0.0.1:8765/control -d '{"name":"K3","action":"set","value":127}'
curl -s http://127.0.0.1:8765/screen.png -o /tmp/screen.png
```

Pad presses need a velocity: `{"name":"PAD 3","action":"press","value":100}`.
`GET /state?since=0` gives LED and pad state; LED 75 is CHOP.

### Ghidra bridge

```sh
GHIDRA_MCP_ALLOW_SCRIPTS=1 ~/Applications/ghidra_12.2_PUBLIC/ghidraRun ~/mcpsamplebin.gpr
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x21a3b10
```

Scripts used this session: `ResearchDecompile`, `HpssRefs`, `FindStr`,
`DumpRange`, `VtInfo`, `DumpVtable`, `FindConst` (all in `~/ghidra_scripts`).
`FindStr` searches defined strings and falls back to a raw byte scan.

**Warning about data reads in Ghidra:** code and strings resolve correctly at
their nominal addresses, but several `.data.rel.ro` vtable regions read as
garbage (`0x6dbd868`, the sample's own vtable). Read vtables at runtime with
the `vt`/`vl` probe commands instead; do not conclude a class has no methods
because its vtable looks empty in Ghidra.


## 2026-11-04, second pass: the address-space bug (read this first)

Everything in the first pass that said "the hook never fired" or "calling the
command faults the app" was an artifact of one mistake: **Ghidra VAs are the
mod's own offsets plus `0x100000`.** `HPSS-HANDOFF.md` states the rule and its
table proves it (the free-pad finder is `0x296dd00` in Ghidra and
`A_HPSS_FREE_PAD 0x286dd00` in `minimod.c`). The probe had been installing
hooks and reading data at `base + <Ghidra VA>`, i.e. 0x100000 past the real
target, so it patched unrelated code and jumped into garbage.

| Ghidra VA | mod offset | what it is |
|---|---|---|
| `0x21a3b10` | `0x20a3b10` | add a Manual slice: `(editor, {sample, refcounted}, frame)` |
| `0x21a9fa0` | `0x20a9fa0` | add a slice at the playhead (`editor+0x848`) |
| `0x21a6d20` | `0x20a6d20` | add a slice at a given frame (drag end) |
| `0x21a7140` | `0x20a7140` | slice-action dispatch: `(editor, msg, kind)` |
| `0x1a60af0` | `0x1960af0` | Insert Slice command ctor |
| `0x1a60d50` | `0x1960d50` | Split Slice command ctor |
| `0x1a5d1c0` | `0x195d1c0` | Remove Slice command ctor |
| `0x1a5cf40` | `0x195cf40` | Remove All Slices command ctor |
| `0x2a43640` | `0x2943640` | sample validity predicate |
| `0x2a41ff0` | `0x2941ff0` | sample -> slice list accessor |
| `0x2a41f80` | `0x2941f80` | sample -> frame count |
| `0x1a2d040` | `0x192d040` | slice-cell publisher (runs on every edit) |

With the corrected offsets the hooks fire, and the results below replace the
guesses of the first pass.

### What actually fires on a slice edit

`Merge` (SHIFT+SCREEN 3) and `Split` (SHIFT+SCREEN 2) both mutate the slice
list. With every candidate hook installed, only these fired:

| hook | what it sees |
|---|---|
| `0x2814ec0` marker resolver | `(layer, index)`; pads and the playback voice call it |
| `0x2941ff0` slice-list accessor | caller `0x152a91c` during redraws, caller `0x1655024` from the chop-button state refresh |
| `0x192d040` slice-cell publisher | runs on the edit itself |

`0x1960d50` (Split Slice), `0x1960af0` (Insert Slice), `0x195d1c0` (Remove
Slice), `0x20a7140` (slice-action dispatch) and `0x20a3b10` (add a slice) did
**not** fire. So the Chop UI does not go through the command objects at all —
it edits the `SliceList` directly. The command objects exist and are correct,
but the UI is not their client.

### The Split button rules (verified)

* Split is greyed when the sample already has **16 slices** — that is the
  maximum. Merge one away (16 -> 15) and Split turns white; press it and the
  count goes 15 -> 16.
* Split is only offered when Chop Type is **Manual**. K3 (control surface
  CC 18) with an absolute value of `127` selects Manual; the screen then
  labels K3 `Manual` and Split becomes enabled. `0/32/64/96` give
  Threshold / Regions 4 / Regions 8 / Regions 16.
* The screen's chop-button state refresh is `FUN_01754b30` (Ghidra
  `0x1754b30`): it computes `count = (*(list+0x68) - *(list+0x60)) >> 3` and
  calls each button's `setEnabled` through vtable slot `0x60`. That is the
  authoritative enable logic, and it confirms the vector layout.

### The model, re-confirmed

* `slice list = 0x2a41ff0(sample) = *(sample+0x130)+0x10`, vector of
  `Slice*` at `+0x60/+0x68/+0x70`.
* `layer + 0x418` = selected marker: `0x80` start marker, `0x81` end marker,
  otherwise a slice index. Pads move it; entering Chop selects slice 0.
* `layer + 0x6e0` bit 0 = chop flag; `layer + 0x738` reads 0 here.
* `state + 0x22e8` is a `vector<Layer>` with stride `0xb30`
  (`0x2904ae0` walks `+0x22e8..+0x22f0` by 0xb30).
* A Slice object is ~0x340 bytes. **A diff of slices 0 and 1 of a chopped pad
  shows `+0xd0 = 15127` (the sample end) in both**, so `+0xd0` is an
  end/length-like field, not the start. The earlier `0, 945, 1890 ...`
  reading came from a different pad state and is not proof of a start field.
  The start field is still unidentified.

### Scans and what they rule out

| scan | predicate | result |
|---|---|---|
| `find` | a pointer inside screen / state / binding whose `+0x4c8` is our sample | 0 |
| `scan` | any heap qword equal to our sample, candidate base = hit-0x4c8 | 8 loose hits, 0 with the editor's own shape (`+0x4d0` heap pointer, `+0x10c8` controller with a command manager) |
| `scan2` | `u32 at +0x528 == selected marker` plus a controller at `+0x10c8` | 0 |
| malloc probe | allocations 0x100..0x8000 with LR while changing the chop type | 0 — slices are preallocated/pooled |

So the `editor` object described below has not been observed at all, and the
`FUN_02a43640(sample,1)` fault that the first pass took as evidence
("the command wants a different sample") was produced by the wrong address.
That question is open again and can be settled by re-running the `samp` probe,
which now calls `0x2943640`.

### Where the next session should start

1. Re-run `samp` (validity predicate at the corrected address) to learn whether
   `layer+0x4a8` is an acceptable sample for the command path.
2. Hook `0x192d040`'s **caller** during a Merge to get the screen method that
   edits the list; its neighbourhood contains the insert/split/merge logic the
   UI actually uses. That function takes the screen, not an editor, so it is
   reachable from `g_screen` — a much better lead than the editor.
3. Failing that, identify the Slice start field by dumping slices 0..3 of a
   Regions-4 pad (values 0, N/4, N/2, 3N/4 are unmistakable) and then test a
   single field write plus a repaint.


## 2026-11-04, third pass: measured facts with working instrumentation

With the address space fixed, these are measurements, not inferences. The
probe commands named here exist in `mod/src/minimod.c` under `MINIMOD_DEBUG`.

### The Slice object

A Slice is a ~0x340-byte object; `+0xd0` holds one frame position per slice.
On a 4-slice pad they read `2465, 4907, 7349, 9794` — evenly spaced, so `+0xd0`
is the slice boundary the chop produced.

**Writing `+0xd0` does not move the drawn marker.** Verified: slice 1 was
`4907`, written to `5376`, the write persisted across a repaint issued
directly through `FN_01149880(screen,0,0)`, and the screen stayed byte
identical. Toggling CHOP away and back **rebuilt** the list and restored
`4907`. So the Slice objects are a projection; something else is the source of
truth for a rebuild.

### K1/K2 do not move boundaries

A snapshot of every slice field was taken on every button press and every knob
turn while a 4-slice Manual pad was driven from the panel (K1 turned through
its whole range, then Split, then Merge). Result: `+0xd0` never changed, but
the Slice objects were **recreated** (new pointers, same positions). K1/K2 are
range/zoom controls that trigger a full rebuild.

### The chop buttons

`FUN_01754b30` (Ghidra `0x1754b30`) is the button-state refresh. It computes
`count = (*(list+0x68) - *(list+0x60)) >> 3` and calls each button's
`setEnabled` through vtable slot `0x60`. Split is enabled only when the chop
type is Manual **and** `count < 16`; Merge when the selected marker is above
the first slice. Verified from the panel: at 16 slices Split is refused and
greyed; merging one away enables it and the next Split takes the count back to
16.

### The layer is not the chop model

`state+0x22e8` (stride 0xb30) is the runtime Layer: `+0x418` selected marker
(`0x80` start, `0x81` end, else a slice index), `+0x6e0` bit 0 chop flag,
`+0x4a8` the Sample. But the `SlicePoints` vectors of the deserializer
(`0x1e02230`) are **not** at `layer+0xb0/+0xc8/+0xe0/+0xf8`; those words are
inline `std::string` data (`"fier"`, `"rie"`). That table was wrong; the
deserializer's class is a different one.

### Allocation

The app does not allocate slices through libc `malloc`: an `LD_PRELOAD`
`malloc` hook sees nothing in 0x100..0x8000 while the slice list is rebuilt and
the Slice pointers change. The app has its own allocator, so a slice object
allocated by the mod could not be freed by the app. **Do not create Slice
objects from the mod.**

### Transient positions

Scanning the heap for the boundary as a double (`2465/44100 =
0.05589569160997732`) finds exactly one hit, but the memory is gone a moment
later. The persistent source of truth for a rebuild is still unidentified.

### The probe toolkit (debug builds only)

`/run/minimod.chop` takes one command; it runs on the **UI thread** from the
button hook:

| command | purpose |
|---|---|
| `dump` | view state, slice count, layer fields, chop-type vectors |
| `count` | slice count of the displayed sample |
| `snap` | full snapshot: marker, count, per-slice field values and hashes |
| `snaptrack` / `snapoff` | snapshot on every button press and every knob turn |
| `lay` | 0xb30 bytes of the layer |
| `sl <i>` | 0x140 bytes of slice `i` |
| `rw <addr> <len>` | arbitrary memory, 8 bytes per line |
| `ww <addr> <hex>` | write one qword |
| `rp` | repaint the screen through `FN_01149880` |
| `vt [n]` / `vl [n]` | runtime vtable of the sample / of the slice container |
| `mk <off> [n]` | words at `base+off` |
| `scan` / `scan2` / `find` | heap searches for the editor-shaped object |
| `samp` | sample validity (`0x2943640`) |
| `alloc` / `allocoff` | malloc-size logging |
| `findval <u32>` / `findq <u64>` | value search across writable memory |
| `wvec` | write watch on the slice vector header and element buffer |
| `addx <ed> <frame>` / `rmallx <ed>` | guarded Insert / Remove All with an explicit editor |

### What is left

The rebuild's **input** is the missing piece. Two concrete ways to find it:

1. Watch the element buffer (`wvec` now arms it) while pressing Split, and read
   the writer's caller — the split code is the one function that knows the
   boundary array. The last attempt caught a libc `memmove` inside the vector
   assignment, so the caller has to be recovered from a deeper frame
   (`backtrace()` in the hook, or `__builtin_return_address(1..3)`).
2. Search for the persistent position array by *shape*: four ascending doubles
   with `0x18` stride whose values are `frame/rate`, found while the Chop view
   is open on a freshly chopped pad.

Once the input array or the split function is known, the feature is: set Chop
Type Manual, obtain N slices, write our N onset frames into that array (or call
the split function N times with our frames), and verify by reading the Slice
`+0xd0` values back.

## Work required to finish

1. **Find how the editor is obtained.** `editor+0x4c8` holds a `Sample*` that
   `0x2a43640` accepts, which is *not* `layer+0x4a8`. Trace a caller of
   `0x21a3b10` or `0x21a7140` (they are reached through indirect calls; the
   jump tables Ghidra shows around `0x6d405e0` are relocation data, not
   switch tables). Follow `0x21ac170`, `0x21a47c0`, `0x21a6420` and
   `0x21aa9e0` — all take the editor as their first argument.
2. **Confirm the Insert Slice call** with a real editor: `addx <editor>
   <frame>` must raise the slice count by exactly one and draw a marker.
3. **Apply** — set Chop Type to Manual (CC 18 = 127), Remove All Slices, insert
   one slice per detected frame, read the slice list back and compare, and
   report `n slices` only after that comparison passes. Keep it on the UI
   thread; the analysis may stay on the worker.
4. **Verify** on the emulator with known frame positions (A03 drums, silence,
   a very short slice, an existing Manual chop, pad and page switches,
   save/reload), check `NRestarts=0`, and re-test HPSS, AutoTrim and pitch.
5. **Ship** only then: rebuild `mod/build/minimod.so`/`minimod.zip`, update
   `README.md` and `docs/SPECTRAL-CHOP.md`, commit and push.

## Checks

```sh
python3 mod/tests/spectral_onsets.py
python3 mod/tests/hpss_dsp.py
python3 mod/tests/wav_edges.py
./mod/build.sh
```

The emulator tooling is `/Users/jakubkolcar/projects/customs/virtual-mpc-sample/lab.py`.
Treat its project data as user data: do not reset it without asking.
