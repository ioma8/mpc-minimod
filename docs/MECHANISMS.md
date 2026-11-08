# MPC Sample 1.3.0.12 — mechanism catalogue

The firmware internals this project reverse-engineered: what each mechanism is,
where it lives, how it was established, and which feature uses it. This is the
reference; the method is in [`RE-METHOD.md`](RE-METHOD.md), the tools in
[`TOOLING.md`](TOOLING.md), per-feature behavior in
[`FEATURES.md`](FEATURES.md), and the investigations that led here in
[`HISTORY.md`](HISTORY.md).

## Conventions

* **mod offset** — what `FN()`, `install_hook()` and the `mk` helper take.
  **Ghidra VA = mod offset + `0x100000`.** Runtime address = `base + offset`,
  where `base` is the module's load address from `/proc/<pid>/maps`
  (`0xaaaaaaaa0000` in the lab; the binary is a PIE and the stock launcher
  disables ASLR).
* Evidence markers: **[rt]** measured at runtime (emulator or device),
  **[hook]** observed by a live hook, **[static]** decompiled only,
  **[doc]** established elsewhere in this repository.
* The target is `/usr/bin/MPC Sample`, 119,243,440 B, sha256
  `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`,
  firmware **1.3.0.12 only**. The autorun package also pins the first 4 KiB
  (`44507c7a…`) as a cheap build check.

---

## 1. Process, image and loader

| mechanism | detail | evidence |
|---|---|---|
| one big PIE | `/usr/bin/MPC Sample`, ~120 MB, statically linked C++ runtime | [rt] |
| two address spaces | Ghidra VAs are `+0x100000` from mod offsets; mixing them silently patches the wrong function | [rt] |
| our code runs inside it | `LD_PRELOAD` an aarch64 shared object; the constructor resolves `base` with `dl_iterate_phdr` | [rt] |
| its own allocator | an `LD_PRELOAD` `malloc` hook never sees slice or sample allocation, so memory handed to the app by the mod could not be freed by it — never allocate Slice or Sample objects | [hook] |
| symbols are stripped | no `_Znwm`; runtime identity comes from offsets, vtables or an unstripped build linked separately | [rt] |
| entry-patch hooking | 4-instruction `ldr x16, #8; br x16; .quad hook` slot in a relay page within ±128 MB of `base`; trampoline replays the displaced prologue instructions | [rt] |

Hook installation rules, the relay/trampoline layout and the refusal conditions
are in [`MODDING.md`](MODDING.md).

---

## 2. Screen, page and paint

| mechanism | detail | evidence |
|---|---|---|
| the main screen | `SampleModeScreen`, vtable file offset `0x6e212a0`; verify it on every access | [rt] |
| page visibility | `FN(0x112fd50)(object) != 0` — on the screen it answers "Sample page visible"; on `screen+0x1ee0` it answers "Chop group visible" (= chop mode) | [rt] |
| paint hook | the paint vtable slot (slot 27, byte `+0xd8`) is replaced; it fires per repaint and is the only safe place to touch the model | [hook] |
| repaint | `FN(0x1149880)(screen, 0, 0)` forces one; it does **not** rebuild cached sub-views | [rt] |
| page switch is asynchronous | a CHOP press only *schedules* the switch; polling in the same callback cannot observe it | [rt] |
| view caching | the Chop view keeps its own slice layout; raw model writes repaint nothing, while the native slice API broadcasts and does | [rt] |

### The screen object is a flat widget container

`SampleModeScreen::resized` (`0x1660370`) walks ~55 embedded children; the
widgets live *inside* the screen object, so a vtable scan finds them at fixed
offsets. One measured inventory (screen object, first `0x9000` bytes):

| count | class | first offsets |
|---|---|---|
| 26 | `ui::TextButton` | `+0x00a8 … +0x0b98` |
| 15 | `juce::Label` | `+0x0bc0 … +0x0d48`, `+0x1500 … +0x1580` |
| 12 | `ui::ImageButton` | `+0x0da0 … +0x1490` |
| 33 | `ui::Slider` (6 vptrs each) | `+0x15f8`, `+0x1c18`, `+0x1e68`, … `+0x64b8` |
| 4 | `ui::SampleModeWaveformViewport` (vptrs `0x6e22408`, `0x6e225b8`, `0x6e22618`, `0x6e22640`) | `+0x18c8`, `+0x1968`, `+0x1970`, `+0x1a60` |
| 4 | `juce::ImageComponent` | `+0x1848`, `+0x18a8`, `+0x1b98`, `+0x1bf8` |
| 33+33 | `AccelerationFilter` + `GestureTracker` | one pair per knob |

The screen object is **recreated** between app runs; any cached pointer must be
re-resolved (`g_screen` is refreshed from the paint hook).

### What the Trim page paints

Over the wave area `x = 15…304` the page draws a background fill, a 1-px marker
at `x = 15 + 289·pct`, and 4×4 handles. A byte-level screen diff across a
50 % → 75 % knob move shows **exactly four** changed words:

```
screen+0x1f6c  f32  50.0 -> 75.0   (the painted percentage)
screen+0x28bc  f32  50.0 -> 75.0   (mirror, second widget)
screen+0x1fb8  u8   label width cache
screen+0x2908  u8   label width cache
```

So the marker is a pure function of `f32 screen+0x1f6c`; there is no fifth store.

### Locators

`AC50WaveformLocator::bounds` (`0x16b6060`) computes its box from a cached
normalised position:

```c
view = this->view @0x18;
x    = round((double)this->pos @0x168 * view->width @0x28 - 2.0);
setBounds(this, x, 0, 4, view->height @0x2c);
```

A locator caches `pos`; it does not read the trim property at paint time. Who
writes `+0x168` on this page was never pinned — the native command updates the
model instead.

### Screen identity by vtable

Screens are identified by comparing `*(uintptr_t*)object` with
`base + <screen vtable>`, never by name. `SampleModeScreen` = `0x6e212a0`.
Other screens seen in the app: `0x6e1e258`, `0x6e1f060` (used by SampQOL's
`context_visible_screen`).

---

## 3. Pad state and layers

```
binding  = screen + 0x178  pad-state binding for the selected pad
binding  + 0x60            pad state
state    + 0x60            command manager for the slice commands
state    + 0x22e8          vector<Layer> begin; stride 0xb30; 8 layers (0x5980 B)
layer    + 0x1a8 / +0x220  trim Start / End knob values  (float, knob stride 0x78)
layer    + 0x418           selected marker: 0x80 start, 0x81 end, else slice index
layer    + 0x4a8           the Sample
layer    + 0x4b0           refcounted companion of the sample
layer    + 0x6e0 bit 0     chop flag
layer    + 0x7b8           trim start frame   (u32)   <- native command
layer    + 0x810           trim end frame     (u32)   <- native command
layer    + 0x868           trim start mirror  (u32)   <- native command
```

* The pad-index → layer path is `FUN_02968450` (integer argument is the pad
  index), resolving through the program's pointer vector at `+0x2808/+0x2810`.
  [static], [doc]
* `FUN_02902e60` copies pad blocks and documents the `0xb30` stride;
  `FUN_02918be0` applies one block; `FUN_0290dca0` is the per-pad dispatch
  (decompiled copies in [`re/`](re/README.md)). [static]
* The **native trim command writes frames** at `+0x7b8/+0x810` and the mirror at
  `+0x868`. Verified by diffing the layer around a SHIFT+B3 cycle, and asserted
  by `mod/tests/emulator_autotrim.py`: on a 10,501-frame sample the three steps
  produce `14..8159`, `23..7503`, `24..7453`, and the fourth press restores
  `0..10500`. [rt]
* The older, property-based write path used `+0x4d8/+0x588/+0x530` instead; the
  native path does not touch those words. Treat them as superseded. [rt]
* The page keeps its own copy: `FUN_0290dca0` reads `pad+0x1588` (Start) and
  `pad+0x15e0` (End), 0x58 apart, and pushes them into the page widgets. This is
  the page's cache, not the model. [static]

---

## 4. The Sample

```
sample + 0x30   -> holder
holder + 0x120  juce::String-like text pointer -> the app's own WAV path
sample + 0x130  -> meta
meta   + 0     u16 channels
meta   + 4     u32 frames
meta   + 8     u32 sample rate
meta   + 0x10  the slice container
meta   + 0x70 / 0x78 / 0x80   vector<Slice*> begin / end / capacity
meta   + 0x3e4 int32 root note, MIDI 0..127 (default 60)
```

The root note is the note the sample plays at its original speed; the 16-Levels
renderer and the app's root-note setting both use it. Measured on the emulator:
A01 `RnB-Kik-Grove1 91V` = **88**, other pads 60. A chromatic note must be
measured from this note, not from C3. [rt] — confirmed against
[`ADDIN-ECOSYSTEM.md`](ADDIN-ECOSYSTEM.md) (scale mode reads the same field).

### Reading the sample's audio from memory

```
sample + 0x170  -> holder object (RTTI "12MemorySample")
holder + 0x18      the same object as sample + 0x130 (the data/meta above)
holder + 0x28   -> sample holder
holder[0]          the decoded float32 audio buffer
```

`meta` is `{u16 channels, u32 frames, u32 rate, u32 bits}` with `bits == 32`, so
the buffer is interleaved float32 of `frames × channels`. This is the loaded
sample, not a file; it also works before the sample is saved. [doc] (scale mode's
`sample_audio()`), spot-checked here: A01 header `(2, 10501, 44100, 32)`. minimod
reads AutoTrim and transform input from this buffer first and falls back to the
WAV for streamed samples.


`FN(0x2943640)(sample, 1)` is the sample-validity predicate (1 for a live
sample). [rt]

The path at `+0x120` is the app's project working copy, usually
`/data/TempProjects/<Project>_[ProjectData]/<name>.wav` — not the user-content
file. The app may re-serialise and re-dither that working copy (measured: `JUNK`
chunk shrinks a few bytes, PCM re-dithered by up to ±2 LSB), so **never write
to it** and never assume its layout is stable. [rt]

---

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

`FUN_02a50d20` constructs the Slice, registers its listeners (`FUN_02a4b170`,
`FUN_02a4a080`), notifies observers (`FUN_0102d0d0`), links it into the
container (`FUN_0246b140`) and calls the reconciler. The K1/K2 knobs edit the
same `+0x78`/`+0xd0` words for the slice a pad press selects — measured, not
inferred: K1 moved slice 0's `+0xd0` 2465 → 4906, K2 moved slice 1's `+0xd0`
4907 → 5143 → 5295 → 6051, carrying the neighbour's mirrors. [rt] + [static]

Because the API allocates, links listeners and broadcasts, a chop written
through it needs no rebuild and no forced repaint.

---

## 6. The chop-type binding

```
screen  + 0x168   chop parameter binding
binding + 0x80    cached parameter object (fallback setter lives here)
binding + 0x9c    chop type, int   (Manual = 0; list at file 0x727f870)
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
[rt]

---

## 7. The observable property system

The firmware's generic "parameter changed" plumbing:

```
knob -> property object P -> 0x13bb930(P, value)
                              -> walks an intrusive list, calls
                                 listener->vtable[0x28](listener, value)
                              -> parameter object -> page widget / model
```

* `0x13bb930`: `[P+0x10]` first listener node, `[P+0x08]` sentinel, node `+0x18`
  target (low 2 bits tagged), node `+8` next, `[P+0x50]` cached value.
* `0x14d0890` is a *different*, simpler property class:
  `if (value@0x20 == v) return; ++count@0x18; value@0x20 = v; for (node in
  list@0x08/@0x10) node->changed(v);` — it de-duplicates, `0x13bb930` does not.
* Broadcasting through `0x13bb930` really drives the app: a mis-identified
  property once ran the stock **Normalize** command, which proves the mechanism
  and shows how easy it is to hit the wrong property.
* The Trim knobs go through `0x13bb930`; their listener list includes the trim
  core `0x15e70c0(float value, TrimParameter*)`, so calling the core directly
  fills the parameter but skips the other listeners.

### Trim parameters

`TrimParameter` vptr `0x6e1b240`; the page has exactly three live instances
(Start, End, Loop, ordered by ascending address):

| field | meaning |
|---|---|
| `+0x19c` | value 0..1 |
| `+0x1a0` | applied value (`-1` = unset) |
| `+0x1e0` | min frames |
| `+0x1e8` | max frames = sample length |
| `+0x134` | kind |
| `+0x70` | model |
| `+0x80` | boundary object: `+0x38` start frame, `+0xa8` end frame |
| `+0xb0` | context |

Control ids: **Start `0x462`, End `0x466`**, four apart in the same knob group.
The dispatch funnel is `FUN_029c6510(surface, 0, id, id)`; it calls the
surface's virtual `vtable+0x160` = `FUN_02904e90`, a getter that reads
`FUN_02912f10(*(page+0x22e8) + idx*0xb30) + 0x78` for `0x462` and `+0xd0` for
`0x466`. The matching setter is `FUN_01a68020(0, surface, controlId, value,
0xd28)` → `FUN_01a679e0`. [rt]

For reference, the four-byte write of `0.5` into a learned Start property's
`+0x50` *did* move the painted edge to 50 % — the property is what the page
paints — but the page still needed a knob nudge to repaint, which is why
AutoTrim ended up using the native command instead (see
[`FEATURES.md`](FEATURES.md#autotrim)).

---

## 8. The slice command / undo system

| command | ctor (Ghidra) | arguments |
|---|---|---|
| Insert Slice | `0x1a60af0` | `(controller, {sample, refcounted}, frame, editor)` |
| Split Slice | `0x1a60d50` | `(controller, {sample, refcounted}, index, editor)` |
| Remove Slice | `0x1a5d1c0` | `(controller, {sample, refcounted}, index, editor)` |
| Remove All | `0x1a5cf40` | `(controller, {sample, refcounted}, editor)` |
| UI wrapper | `0x21a3b10` | `(editor, {sample, refcounted}, frame)` |
| add at playhead | `0x21a9fa0` | `(editor)` — uses `editor+0x848` |
| dispatcher | `0x21a7140` | `(editor, msg, kind)` — reads marker at `editor+0x528` |

The "editor" object they expect carries `+0x4c8` sample, `+0x4d0` selection,
`+0x528` marker, `+0x848` playhead, `+0x10c8` command controller. **It was never
observed at runtime**, and the Chop UI does not use these commands: with every
ctor hooked, Split and Merge fire none of them and edit the container directly.
The commands are an undoable API for paths outside this view. [rt] + [static]

Undo/telemetry anchors: `"Insert Slice"` at file `0x46a80e0`, key
`"slices_added"`, `"chopMinSliceTime"` at `0x469ecf0`.

---

## 9. The control surface and input path

| mechanism | detail | evidence |
|---|---|---|
| button dispatcher | mod `0x159f2f0`; `event+8` code, `event+9` down/up; `*(*(event)+0x10)` is the surface | [static] + [rt] |
| decoded controls | mod `0x159f230(event, raw, lo, range)`; controller at `event+8` | [static] + [rt] |
| DATA decoding | controller `0x64`; `delta = raw > 0x40 ? raw-0x80 : raw` | [static] |
| DATA signal | mod `0x140dbc0(surface+0x2da8, delta)`; one listener-list walk per event, **no de-duplication** | [static] + [rt] |
| menu ownership | match that signal object, move by sign, consume while open/busy; forward every other signal object | [rt] |
| knob suppression | the decoded-control hook blocks non-DATA controls while the menu owns input; no read interception | [rt] |
| button codes | CHOP `0x4b`, SHIFT `0x31`, DATA push `0x6f`, screen buttons `0x0c/0x0d/0x0e`, B3 `0x0e` | [rt] |
| panel feedback | `/state?since=0`: LED 75 = CHOP, LED 82 = PLAY | [rt] |
| control-surface transport | `/dev/snd/midiC0D0`, ALSA timestamped framing: 32-byte frames, MIDI bytes at `+0x10`, one button per frame, note on/off on channel 1 | [rt] + [doc] |

The decoded-control dispatcher's decisive branch at Ghidra `0x169f280`:

```text
cmp  w3, #0x64          ; controller 0x64 = DATA
b.ne return
ldr  x0, [x0, #0x10]    ; context
cmp  w1, #0x40
sub  w2, w1, #0x80
csel w1, w2, w1, hi     ; signed delta
mov  x2, #0x2da8
add  x0, x0, x2         ; the signal object
b    0x150dbc0
```

`FUN_0150dbc0` locks the signal's mutex at `+0x20`, walks its listener list and
invokes vtable slot `+0x28` with the integer delta. +1 followed by +1 is two
events. The float broadcaster `0x13bb930`, by contrast, compares and stores
`property+0x50` before notifying — mistaking it for the encoder source is what
made the first menu implementation drop turns.

`event` is a compact struct: `+8` type/controller, `+9` down, `+0x10` velocity,
`+0x14` property mask (u16), `+0x16` property 0, `+0x30` pad/slot byte.

---

## 10. External MIDI

Address space is stated per row: `mod` offsets are what the shipped hooks use;
`Ghidra` VAs are `mod + 0x100000` and come from the decompiler.

| mechanism | detail | evidence |
|---|---|---|
| native note dispatcher | mod `0x211b090` (Ghidra `0x221b090`); four arguments, event at argument 3, source ID in argument 4 | Ghidra decompile; SampQOL v0.1.5 |
| routing key | **source ID** (`param_4`); physical pads use `0x7ffffffa`; external MIDI has its own ID (e.g. `0x90b6e9eb`) | [static] + [rt] |
| note fields | type `3` at `+8`, velocity at `+0x10`, note/slot at `+0x30`, properties at `+0x14` | [static] |
| router | Ghidra `0x221aa30` → `0x2215070` decodes the event; `0x2214fc0` recomputes the slot byte at struct `+0x30` | [static] |
| 16 Levels writer | Ghidra `0x21472f0`: type at `levels+0x88`; always writes exactly one parameter slot (or velocity) per level 0..15 | [static] |
| pitch setter | mod `0x27e4400` (Ghidra `0x28e4400`): `props[index+2] = clamp(steps·(s0/(max-min)), 0, 255)`; `props[0] |= 1<<index`; index 0 range −120..120, 241 steps | [static] |
| the app's own call | `w3 = level − originalPad` (signed semitones), `event+0x30` = selected pad | [hook] |
| external-note source | keep the firmware's source for pass-through; rebuild the note for the selected pad and dispatch with `0x7ffffffa` | [rt] |

`FUN_02215070` defaults `event+0x16..0x17 = 0x78 0x78` before the router
recomputes the slot. Note-offs are type 3 with velocity 0 and go through the
same path.

**The sequencer does not see this.** The pad-grid observer loop `FUN_020ce3e0`
(call site `0x20ce480`) is where the sequencer records; it invokes the pad pump
`FUN_01f27360`, which then calls the 16 Levels writer and the dispatcher.
External MIDI enters at `FUN_01f25cf0`, below the recorder, so neither stock nor
chromatic external MIDI is captured in a sequence. This is a firmware feature
gap, not a mod regression. [rt] — see [`HISTORY.md`](HISTORY.md#6-sequence-recording-not-implemented).

---

## 11. Sample pool, pad occupancy and Extract

| mechanism | call (Ghidra VA) | evidence |
|---|---|---|
| first empty pad (0–127, `-1` if full) | `0x296dd00` | [doc] |
| pad occupancy predicate | `0x29047b0` | [doc] |
| stock Sample/Chop Extract handler | `0x17580d0`, argument is the Sample screen | [doc] |
| `ExtractAndAssignSampleCommand` ctor / execute | `0x153ed40` / `0x153f0b0` | [doc] |
| sample frame-count getter / reader loop | `0x2a41f80` / `0x21c6e40` | [Ghidra](re/hpss/reverb-reader-boundary.txt) |
| extraction writer callback / concrete writer | `0x1e8aff0` / `0x1e93570` | [Ghidra](re/hpss/read-loop.txt), [Ghidra](re/hpss/wav-write-block.txt) |
| extraction settings object | `screen + 0x9ba0`; range start `+0x78`, end `+0xd0` (exclusive) | [doc] |

The pattern that works, and the reason HPSS needs no guessed model writes:
**capture the native command, patch only the values it is about to copy, let the
firmware keep ownership of files, history and pad assignment.**

### The writer boundary

| Function | Finding |
|---|---|
| `0x2a41f80` | reads the source frame count from `sample+0x130+4` |
| `0x21c6e40` | streams provider blocks and stops when its position reaches the reader length at `reader+8+4` — requesting an end beyond the source produced no extra blocks |
| `0x1e8aff0` | writer callback reads block frame count at `+0`, audio pointer at `+8`, then calls the concrete writer |
| `0x1e93570` | concrete WAV writer consumes `frames × channels` float samples and advances the output by the supplied count |

Therefore an output can be longer than the source without fabricating reader
frames: substitute the final source block's audio pointer with the processed
buffer at the current output offset, raise that block's frame count to the
remaining rendered frames, call the original writer synchronously, then restore
the original pointer and count before returning. The mod marks the full
processed buffer consumed only after that final writer call. The emulator
confirms the concrete writer finalises the longer WAV and native Extract still
registers and assigns it. [rt]

### Built-in H3 effects

The firmware registers a family of native effects under the H3 type registry
(`Air::H3M::…`): `StereoReverb`, `StereoReverbSimplified`, `SpringReverb`,
`NonLinearReverb`, `DattorroReverb`, `EconomyReverb`, `ModReverb`,
`SurroundReverb`, `AnimatedReverb`, `DJReverb`, `AlgorithmicReverb`, `Vinyl`,
`Decimator`. [static]

| effect | factory (mod offset) | notes |
|---|---|---|
| `StereoReverb` | `0x3c445b0` | instance `0x988` B, ctor `0x3cac6b0`, main vtable `0x7180be0`, 18 parameters; slot 11 `0x3c9bed0` processes, slot 9 `0x3cad0b0` prepares, slot 21 `0x3caca50` applies parameters; params live at `*(float**)(effect+0x18)` |
| `Vinyl` | `0x3c43cf0` | params[0] Drive, params[5] Noise Level |
| `Decimator` | `0x3c457a0` | params[0] Bit Depth, params[4] Sample Rate |

Offline lifecycle (verified [rt]): `create(NULL, &context)` → slot 9 with
`&context` → slot 11 per stereo block → slot 1 to destroy. The context is
`{float sample_rate, uint32_t reserved, int32_t block_size}`; slot 9's second
argument is a **context pointer, not a boolean** — passing `1` crashes because
the base setup dereferences it. Normal firmware use goes through the H3 patch
graph (`FUN_03cf5180` parses parts, `FUN_03d27ca0` runs the chain, `H3pc`/`H3ps`
setup messages); the mod instantiates the effect directly instead.

### H3 instrument host boundary (FMBass)

FMBass is a *instrument*, not an effect: registry `FUN_03d46910`, factory
`FUN_03d45d50`, constructor `FUN_03e0af30`, parameter table `0x072acc20`, main
vtable `0x07185c18`; slots 9/11/13/21 are setup/render/event/parameter update.
Its constructor calls `FUN_03cedde0`, which queries `H3pp` (`0x48337070`)
through host vtable slot 4. The effect context does **not** implement that host
interface, so constructing FMBass with it crashes. [static] + [rt]

Standalone event/voice path: 12-voice manager at instrument `+0xc0` (active
count `+0x68`), queue at `+0x358` (size `0xe58`, read index `+0xe10`), queue
callback closure/invoker/active at `+0xe28/+0xe30/+0xe38`. `FUN_03e0c1a0`
drains the queue only when active. Note-on dispatch is
`FUN_03e0eb60 → FUN_03e0ed60 → FUN_03e0b430` fanning into child voice vtable
`0x06ed2040` (init `+0x10`, pitch/velocity `+0x20`, render `+0x48`).

A corrected queue drain did produce one active group and advancing indices, but
the rendered audio stayed **exactly zero**; the outer graph renderer
`FUN_03d27ca0` walks `graph+0x88`, which is null after prepare, so the graph has
no FMBass render node, and the voice's DSP countdown at `+0x3dc` never advances.
Direct part-renderer calls are not proven safe. This is why the shipped Bass
Synth renders with DaisySP instead — see [`FEATURES.md`](FEATURES.md#bass-synth).

### Bass Synth live preview

Stock note-on/off hooks (`0x1e27360` / `0x1e270d0`) receive the pad index. While
the Bass Synth page is open, pad indices 0–15 queue C2–D#3 to a four-voice
DaisySP preview. The device output is `snd_pcm_writei`: interleaved **S32_LE,
4 channels, 44100 Hz** (read from the app's hw params on the first write). The
app imports `snd_pcm_writei@ALSA_0.9`, so an unversioned preload symbol does not
interpose; the mod patches the app's PLT stub (`0xb51ff8`) with a 16-byte slot to
its own hook and calls libasound itself. The hook adds the preview to interleaved
channels 0/1 before the write, leaves the app's planar/engine buffers untouched,
and decays each voice over 1.2 s. Pads are swallowed whenever the Bass page is
open — including during a save render — so a missed preview cannot start a
looping sample underneath. Activation enables and resets the synth before
publishing `active`, and the paint tick re-enables it if the page is open without
it.

The earlier anchor `0x03f398f0` is a different PCM writer (planar float → S32);
it is not called during normal playback, so mixing there was inaudible.
---

## 12. TouchPreferences overlay (hidden settings UI)

The settings UI exists but has no panel binding; it is a touch-only overlay.

| mechanism | detail |
|---|---|
| class | `TouchPreferencesOverlay`; RTTI `0x070f1470`, primary vtable address point `0x070f14e8` |
| constructor | `FUN_03804fe0` |
| factory path | `FUN_03519cb0` allocates `0x5e8` B, sets the title `Preferences`, calls the constructor with the app's existing service/model objects |
| old component | `FUN_03337be0` — a *different* component (do not use as the overlay constructor) |
| layout | sidebar `x=0,y=0,w=114`; content `x=114,w=W-114`; footer `x=W-70,w=64`; all height `H-30` |
| tab switch | `FUN_03336d10(self, index)` — clears the content container, validates the index, calls the tab's activation method at vtable `+0x168`, then adds it |
| apply | `FUN_03336c70` — each tab's `+0x160` method, then `FUN_010bea70(dependency2)` (mutex at `+0x38`, flushes if `+0x109`), then close |
| close | `FUN_03336c10` (direct), `FUN_03336c20` (action `0x3104`), `FUN_02f8ee00` (removes/closes through the native screen host) |

Activation is **not implemented**: the constructor and factory only identify a
construction path; ownership, presentation and runtime call sites were never
established. See [`DEVICE-NOTES.md`](DEVICE-NOTES.md#4-preferences--the-hidden-touch-ui).

---

## 13. Persistence and serialization

* Pad/sample serialization is a separate class from the runtime layer. The
  deserializer (`0x1e02230`) stores `SlicePoints` as
  `{double timeInSeconds, float, double energy}` (0x18 bytes) in vectors —
  Threshold / Regions / Beat / Manual variants. On the live layer,
  `layer+0xb0…` is inline `std::string` data, **not** `SlicePoints`; reading it
  as such is wrong. [static] + [rt]
* A manual chop therefore has a persisted form not exercised here. The pad path
  `FUN_02968450` resolves pads through `+0x2808/+0x2810`. [static]
* Project state lives in `/data/TempProjects/...`; the emulator restores it on
  restart, so transient model edits survive only if the app saves them. [doc]
* The shipped WAV writer emits **float32** for Extract outputs, even from a
  24-bit source. Project working copies are 24-bit PCM and may be re-dithered by
  the app. [rt]

---

## 14. Threads and ownership

| thread | what runs there | rule |
|---|---|---|
| control-surface / input | `buttons()`, `cc_capture()`, `data_signal()` | must not touch the model; publish a request only |
| UI / paint | `screen_paint()`, after the stock paint | the only place model calls are safe |
| worker | HPSS/effects DSP and file reads | never touches the model; commits through the native command on the UI thread |
| audio | playback; marker resolver for the playing slice; Bass Synth mixes into a private planar block before the native writer | no model access, no allocation, no locks |

Cross-thread hand-off is an atomic flag plus a phase machine advanced one phase
per paint pass. [rt] — blocking the input thread and polling for a page switch
in the same callback both failed.

---

## 15. Constants and limits

| limit | value | source |
|---|---|---|
| slices per sample | 16 | [rt] (Split refused at 16) |
| onsets the mod applies | 16, truncated in time order | feature choice |
| chop types | Threshold, Regions 4, Regions 8, Regions 16, Manual — list at file `0x727f870` | [rt] |
| minimum slice length | `chopMinSliceTime` at file `0x469ecf0` (value/units not measured) | [static] |
| marker sentinels | `0x80` start, `0x81` end, else slice index | [rt] |
| undo | slice edits are not undoable in this view | [rt] |
| pad count / layers | 128 pads; 8 layers per pad, stride `0xb30` | [rt] |
| MIDI root for chromatic playback | C3 (note 60) | feature choice |
| sample rates accepted by transforms | 8 kHz … 192 kHz | implementation |

---

## 16. Still unknown

* where Threshold chop mode takes its slice positions from (native; not needed
  once the insert API was found);
* the property object that owns a given `TrimParameter` — identified as a
  concept, never pinned to an instance (the native trim command made it moot);
* whether a manual chop is persisted through the deserializer's `SlicePoints`
  and how it reloads;
* the value and units of `chopMinSliceTime`;
* a safe offline host/render lifecycle for native FMBass;
* behaviour on real hardware for everything emulator-verified;
* which SD mount path the autorun folder is read from (the protocol is known,
  the mount path was never pinned from the rootfs).
