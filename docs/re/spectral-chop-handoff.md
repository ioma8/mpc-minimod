# Spectral Chop: investigation handoff

## Goal

From **SAMPLE OPTIONS → Spectral Chop**, detect onsets in the selected pad's
sample and apply those exact frame positions as stock **Manual** chop markers
on that same pad. Do not extract files or assign other pads. Report the number
of slices, or `no onsets` without changing the pad. Keep HPSS, AutoTrim and
pitch behavior intact.

**State: not finished.** Detection works; the marker write does not. The action
reports `Spectral: not applied` and leaves the pad untouched. Everything below
that is marked *verified* was measured on the emulator, not inferred.

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
- a full heap scan for the same predicate finds 8 objects; the most plausible
  one (`+0x528 == 0x81`, plausible controller and command manager) faults
  inside `0x2a43640` when Insert Slice is called on it — the fault is at
  `0x21a3b20` with a corrupt frame, i.e. the object is not the editor the
  commands expect;
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
