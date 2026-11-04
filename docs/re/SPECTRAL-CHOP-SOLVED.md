# Spectral Chop — solved: model, recipe, and the path that got there

Firmware **1.3.0.12** only. Branch `feature/spectral-chop`. Everything marked
*verified* was measured on the emulator in this lab, not inferred.

## 1. The result

**SAMPLE OPTIONS → Spectral Chop** detects the onsets of the sample on the
selected pad and **rewrites that pad's own chop** so every slice boundary is one
of the detected frames. It creates no samples, uses no free pads and writes no
file. Before reporting `n slices` it reads every boundary back from the model.

Verified with the release build on the emulator, `NRestarts=0`:

| input | onsets | result |
|---|---|---|
| `A03` "Drums 2s", 88200 frames | 10 | 10 slices; all end frames matched on read-back; markers at x = 18, 54, 93, 133, 172, 213, 237, 252, 293 px of the 320 px waveform |
| synthetic frames 500 / 3000 / 7000 / 11000 | 4 | markers at x = 23, 71, 148, 225 px — exactly those frames |

The marker positions are irregular, i.e. they follow the detector, not an even
division. Pads then play the new slices; playback does not restart the app.

## 2. The model (all verified at runtime)

### 2.1 Objects

```
screen              SampleModeScreen, vtable file offset 0x6e212a0
screen + 0x178      pad controls
  + 0x60            pad state
    + 0x22e8        std::vector<Layer>, stride 0xb30   (0x2904ae0 walks it)
      + 0x418       selected marker: 0x80 start marker, 0x81 end marker,
                    otherwise a slice index
      + 0x4a8       the Sample
      + 0x6e0 bit0  chop flag (1 while the Chop view owns the page)
    + 0x60          command manager for slice commands
```

### 2.2 Slices

```
slice list = *(sample + 0x130) + 0x10        // an observable object
  list + 0x60 / 0x68 / 0x70  std::vector<Slice*> begin / end / capacity
  Slice + 0x78    start frame     (duplicate at +0x128)
  Slice + 0xd0    end frame
```

A Slice is about 0x340 bytes. The count is `(end - begin) / 8`; the stock
maximum is **16**.

`K1`/`K2` move `+0x78`/`+0xd0` of the slice selected by a **pad press**, and
keep the `+0x78` ↔ previous `+0xd0` mirrors consistent. That was measured
directly: on a 4-slice pad, turning K1 moved slice 0's `+0xd0` from 2465 to
4906, and turning K2 moved slice 1's `+0xd0` 4907 → 5143 → 5295 → 6051, each
time carrying slice 1's `+0x78`/`+0x128` with it.

### 2.3 The view caches its layout

Writing the Slice fields is necessary but **not sufficient**:

| action after a write | markers redrawn? |
|---|---|
| `FN_01149880(screen,0,0)` (repaint) | no |
| a pad press (changes the selection) | no |
| **leave and re-enter Chop** | **yes** |

Re-entering rebuilds the view from the model, and in **Manual** mode the written
boundaries survive that rebuild. In Threshold/Regions mode a rebuild recomputes
from the algorithm and discards them — so Manual is part of the recipe.

### 2.4 The Chop buttons

`FUN_01754b30` (Ghidra `0x1754b30`) is the button-state refresh. It computes
`count = (*(list+0x68) - *(list+0x60)) >> 3` and calls each button's
`setEnabled` through vtable slot `0x60`. Split is enabled only when the chop
type is **Manual** and `count < 16`; Merge when the selected marker is above
the first slice. That rule is the firmware's, and it is why Split greys out at
16 slices.

### 2.5 The slice commands exist but the UI does not use them

| command | entry (Ghidra) | arguments |
|---|---|---|
| Insert Slice | `0x1a60af0` | `(controller, {sample, refcounted}, frame, editor)` |
| Split Slice | `0x1a60d50` | `(controller, {sample, refcounted}, index, editor)` |
| Remove Slice | `0x1a5d1c0` | `(controller, {sample, refcounted}, index, editor)` |
| Remove All Slices | `0x1a5cf40` | `(controller, {sample, refcounted}, editor)` |
| UI add-slice wrapper | `0x21a3b10` | `(editor, {sample, refcounted}, frame)` |

Pressing Split and Merge fires **none** of them: the Chop UI edits the Slice
objects directly. The commands remain a correct API, but nothing in this view
calls them, and the "editor" object they need was never observed (see §4).

## 3. The recipe as implemented

`mod/src/minimod.c`, `spectral_write_markers()`:

1. **Enter the Chop view.** Reuse the stock button path (CHOP = code `0x4b`)
   and poll `SampleModeScreen+0x1ee0` visibility until the app has switched
   (the switch lands on the next message pass, not synchronously).
2. **Grow to N slices.** While the count is below the onset count, synthesise
   `SHIFT` + `SCREEN 2` (Split) through the captured button event; each split
   adds one slice, up to the stock 16.
3. **Write the boundaries.** For slice `i`: `+0x78 = p[i]`, `+0x128 = p[i]`,
   `+0xd0 = p[i+1]`, with `p[n] = frames`.
4. **Shrink the list.** `*(list+0x68) = begin + 8*n`. The surplus Slice objects
   simply stop being referenced; the vector's capacity is untouched so nothing
   is reallocated.
5. **Rebuild the view.** Toggle CHOP off and on.
6. **Verify.** Re-read the count and every `+0xd0`; report success only if all
   of them match.

The failure branches are explicit: fewer than two onsets, no WAV, a slice list
that cannot be grown, or a boundary that did not read back — each is reported
instead of claiming success.

## 4. The path here (leads, dead ends, and what unlocked it)

### 4.1 The wrong turns

1. **Threshold chop type.** The first implementation opened the stock Chop view
   and selected its Threshold mode. That slices at *the firmware's* detector
   positions, so our positions were discarded. It looked like a success because
   markers appeared.
2. **Extract-based slicing.** Writing slices through the Extract command caused
   restarts and was removed.
3. **Hunting the "editor".** The command objects all need an object carrying
   `+0x4c8` sample, `+0x4d0` selection, `+0x528` marker and `+0x10c8`
   controller. Three different heap scans (by sample pointer, by editor shape,
   by marker value) found nothing usable, and the fault that ended the first
   attempt was read as "the command wants a different sample object".

### 4.2 The mistake that hid everything

**Ghidra VAs are mod offsets + `0x100000`.** `HPSS-HANDOFF.md` states the rule
and its own table proves it (the free-pad finder is `0x296dd00` in Ghidra and
`A_HPSS_FREE_PAD 0x286dd00` in the mod). The probe had been installing hooks
and reading data at `base + <Ghidra VA>`, i.e. 0x100000 past the target, so it
patched unrelated code and jumped into garbage.

Consequences once corrected:

* every "the hook never fires" disappeared — the hooks had been on the wrong
  functions all along;
* `isSampleValid = 1` on `layer+0x4a8`, so the command path's sample was fine;
  the earlier fault was the address, not the object;
* with the hooks live, it became measurable that Split and Merge fire **no**
  command and no dispatcher — only the marker resolver, the slice-list accessor
  and a slice-cell publisher. That is what redirected the work from the command
  API to direct model edits.

### 4.3 The hypothesis loop

Every step was written to `HYPOTHESES.csv` before it was tested.

| # | hypothesis | result |
|---|---|---|
| H1 | the rebuild lives in the function containing `0x1a601d0` | disproved — an unwind cleanup |
| H2 | the editor is findable by scanning the heap for `*(p+0x4c8)==sample` plus its shape | disproved — zero shaped candidates |
| H3 | a pad press in Chop inserts through `0x21a3b10`/`0x1a60af0` | disproved |
| **H4** | **probe addresses were 0x100000 too high** | **proved — root cause of every earlier failure** |
| H5 | the UI Split/Merge path does not use the command objects | proved |
| H6 | Split is refused at 16 slices | proved (also reported by the user) |
| H7 | K3 (CC 18) absolute value 127 selects Manual | proved — the screen label changes and Split enables |
| H8 | slices are allocated per edit through libc malloc | disproved — no allocation in 0x100..0x8000 while the pointers changed |
| H9 | a page watch on the slice vector catches the split writer | inconclusive — the first write is a neighbouring field |
| H10 | `+0xd0` is a start frame | disproved — it is the **end**; both slices of a 2-slice pad read the sample end |
| H11 | K1/K2 move a slice boundary | disproved *by my own test* (the selection was the start/end marker, not a slice) |
| H12 | write `+0xd0` and repaint | disproved — the write persists, the view does not change |
| H13 | the layer at `state+0x22e8` carries `SlicePoints` at `+0xb0..` | disproved — those words are inline `std::string` data |
| H14 | the mod could allocate a compatible Slice | disproved — the app has its own allocator |
| H15 | the boundary times exist as a transient `vector<SlicePoint>` of doubles | inconclusive — one hit, gone on read-back |
| **H16** | **`+0xd0` is the boundary K1/K2 edit for the pad-selected chop** | **proved — the user's guided run** |
| **H17** | **re-entering Chop rebuilds the view from the model** | **proved — markers landed on the written frames** |
| **H18** | **grow → write → shrink → re-enter → verify works from the feature** | **proved — 10 onsets, 10 slices, all read back** |

### 4.4 What the human in the loop changed

Two remarks from the user moved the work more than any single probe:

1. *"the knob 1 or 2 move boundaries of selected chop … you select given chop by
   pressing a pad"* — this corrected my misreading of K1/K2 as zoom controls and
   pointed at the per-slice boundary.
2. *"split button works only when chop type is set to manual"* — this explained
   the greyed Split and identified Manual as a required state.

They were turned into one experiment: snapshot **every word of every slice** on
every button press and every knob turn, then have the user perform the gesture
in the panel. The diff of that capture was unambiguous — `+0xd0` and the
`+0x78`/`+0x128` mirrors — and it is the only reason the field was found.

### 4.5 The final jump

With the boundary known, the last unknown was why a direct write did not show
up. Three writings plus a repaint produced a byte-identical screen; a pad press
changed the pixels but not the markers. Toggling CHOP changed them to exactly
the written frames — the view had been cached all along, and re-entering is the
rebuild trigger. From there the recipe was mechanical.

## 5. Reproducing it

### 5.1 Build and run

```sh
./mod/build.sh          # build/minimod.so
./mod/pack.sh           # build/minimod.zip (SD autorun package)

cd ../virtual-mpc-sample
python3 lab.py run --headless
python3 lab.py preload /path/to/mod/build/minimod.so
```

Panel: `http://127.0.0.1:8765/`, screen `GET /screen.png`, state
`GET /state?since=0` (LED 75 is CHOP, 82 is PLAY).

### 5.2 Debug build and the probe

```sh
zig cc -target aarch64-linux-gnu -shared -fPIC -O2 -fno-omit-frame-pointer \
  -mbranch-protection=standard -DMINIMOD_LOG=1 -DMINIMOD_DEBUG=1 \
  -o mod/build/minimod-dbg.so mod/src/minimod.c mod/src/hpss.c mod/src/spectral.c
```

Write one command to `/run/minimod.chop`; it runs on the **UI thread** at the
next button event (never from a foreign thread). Read `/run/minimod.log`.

| command | purpose |
|---|---|
| `dump` | view state, slice count, layer fields |
| `count` | slice count only |
| `snap` | marker, count, per-slice field values and hashes |
| `snaptrack` / `snapoff` | snapshot on every button press and knob turn |
| `sl <i>` | 0x140 bytes of slice `i` |
| `lay` | 0xb30 bytes of the layer |
| `rw <addr> <len>` | arbitrary memory, 8 bytes per line |
| `ww <addr> <hex>` | write one qword |
| `rp` | repaint the screen |
| `vt [n]` / `vl [n]` | runtime vtables (sample / slice list) |
| `mk <off> [n]` | words at `base+off` |
| `scan` / `scan2` / `find` | heap searches for editor-shaped objects |
| `samp` | sample validity (`0x2943640`) |
| `alloc` / `allocoff` | malloc-size logging |
| `findval <u32>` / `findq <u64>` | value search in writable memory |
| `wvec` | write watch on the slice vector header and element buffer |
| `setpts f0,f1,…` | write boundaries only (no grow, no rebuild) |
| `apply f0,f1,…` | the full recipe, for experiments |

Useful details: `Read /run/minimod.log` by line offset rather than truncating it
(`tail -n +$((n+1))`) — truncating a file the app holds open can stop its
logging. Pad presses need a velocity in the panel API
(`{"name":"PAD 3","action":"press","value":100}`).

### 5.3 Reading markers off the screen

The screen is 320×240. Slice markers are grey vertical lines; scanning row
`y = 92` for pixels with `|r-g| < 8`, `|g-b| < 8`, `90 < r < 220` and
collapsing runs gives their x positions, which map to frames through the
waveform box (x 15…305 → frame 0…frames). That is how every claim above about
markers was measured, without relying on a screenshot impression.

### 5.4 Ghidra

```sh
GHIDRA_MCP_ALLOW_SCRIPTS=1 ~/Applications/ghidra_12.2_PUBLIC/ghidraRun ~/mcpsamplebin.gpr
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x21a3b10
```

**Subtract `0x100000` from every Ghidra address to get a mod offset.** Some
`.data.rel.ro` vtables read as garbage in Ghidra (the sample's own vtable does);
read those at runtime with `vt`/`vl` instead of concluding a class has no
methods.

## 6. Limits and open questions

- At most **16** slices — the stock chop limit. A sample needing more onsets is
  truncated by the detector, not by the write.
- The pad's previous chop is replaced, and stock slice edits are not undoable
  either.
- Growing uses synthetic Split presses; if the firmware refuses to split (for
  example a slice below its minimum length) the action refuses instead of
  writing a partial chop.
- The boundary state is not re-validated against the WAV after the write, only
  against the model; a concurrent user edit during the operation is not
  guarded.
- Real hardware has never been tested.
- Still unexplained: the strings at the top of the file (the deserializer
  `0x1e02230` and its `SlicePoints`/`ManualSlicePoints` layout) belong to a
  different class than the runtime layer. The persistent serialization of a
  manual chop, and where a rebuild takes its positions from in Threshold mode,
  were not pursued once the write path was found.
