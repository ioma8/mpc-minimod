# MPC Sample 1.3.0.12 — firmware mechanisms we found

> **Historical.** Written while the mod was C. `mod/src/minimod.c` and the other C and C++ sources are
> gone (see git history); the equivalent code is in `rust/crates/`, mapped in the "Final state" section of
> [`RUST-PORT.md`](../RUST-PORT.md). File, line and function references below point at the removed C, and build
> commands that compile it no longer apply (`mod/build.sh` builds the Rust crate).

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

The current, Ghidra-traced menu path is documented in
[SAMPLE-OPTIONS-INPUT.md](SAMPLE-OPTIONS-INPUT.md). The November 4 claim that a
changed float property is a DATA detent was wrong and caused ignored turns.

| mechanism | detail | evidence |
|---|---|---|
| button dispatcher | mod `0x159f2f0`; `event+8` code, `event+9` down/up; `*(*(event)+0x10)` is the surface | [static] + [rt] |
| decoded controls | mod `0x159f230(event, raw, lo, range)`; controller at `event+8` | [static] + [rt] |
| DATA decoding | controller `0x64`; delta = `raw > 64 ? raw-128 : raw` | [static] |
| DATA signal | mod `0x140dbc0(surface+0x2da8, delta)` emits every integer event, including repeated identical deltas | [static] + [rt] |
| menu ownership | match that signal object, move by sign, consume while open/busy; forward all other signal objects | [rt] |
| knob suppression | native decoded-control hook blocks non-DATA controls while the menu owns input; no read interception | [rt] |
| button codes | CHOP `0x4b`, SHIFT `0x31`, DATA push `0x6f`, screen buttons `0x0c/0x0d/0x0e` | [rt] |
| panel feedback | `/state?since=0`: LED 75 CHOP, LED 82 PLAY | [rt] |

The float broadcaster `0x13bb930` is still used by absolute knobs and the
existing AutoTrim instrumentation. It is not the menu's encoder source.
There are no transport or property-change fallbacks for menu navigation.

### External MIDI in 16 Levels

| mechanism | detail | evidence |
|---|---|---|
| native MIDI note dispatcher | mod offset `0x211b090` / Ghidra VA `0x221b090`; four arguments, event at argument 3, source ID in argument 4 | Ghidra Java bridge decompile of stock firmware and SampQOL v0.1.5 |
| note fields | event type `3` at `+8`, velocity at `+0x10`, note number at `+0x30`; transform note-ons only | Ghidra decompile of SampQOL's callback and emulator event trace |
| chromatic pitch setter | mod offset `0x27e4400` / Ghidra VA `0x28e4400`; pass event property block `event+0x14` and `note-root` as argument 4 | stock firmware decompile; setter writes note pitch properties |
| source-ID comparison | SampQOL treats the argument as signed 32-bit; `0x7ffffffa` is the physical-pad sentinel and is excluded | SampQOL callback decompile; emulator showed external source IDs have the high bit set |
| selected mode | use the mod's 16 LEVELS button state while the Sample page is visible | `buttons()` state hook and emulator |
| native flow | mutate note properties, then call the original dispatcher once; firmware retains routing, note-off and sustain handling | emulator, C3/C#3/G3 and no-restart check |

The former raw-ALSA thread and direct `note_on(pad)` replay were removed: the
app already owns its MIDI endpoints, and replaying a pad does not transpose a
sample. See [MIDI-CHROMATIC.md](../MIDI-CHROMATIC.md) and
[`../../mod/tests/emulator_midi_chromatic.py`](../../mod/tests/emulator_midi_chromatic.py)
for the source investigation and live acceptance check.

## 10. Sample pool, pad occupancy and Extract

| mechanism | call (Ghidra VA) | evidence |
|---|---|---|
| first empty pad (0–127, `-1` if full) | `0x296dd00` | [doc] |
| pad occupancy predicate | `0x29047b0` | [doc] |
| stock Sample/Chop Extract handler | `0x17580d0`, argument is the Sample screen | [doc] |
| `ExtractAndAssignSampleCommand` ctor / execute | `0x153ed40` / `0x153f0b0` | [doc] |
| sample frame-count getter / reader loop | `0x2a41f80` / `0x21c6e40` | [Ghidra](hpss/reverb-reader-boundary.txt) |
| extraction writer callback / concrete writer | `0x1e8aff0` / `0x1e93570` | [Ghidra](hpss/read-loop.txt), [Ghidra](hpss/wav-write-block.txt) |
| extraction settings object | `screen + 0x9ba0`; range start `+0x78`, end `+0xd0` (exclusive) | [doc] |

The pattern that works there, and the reason HPSS needs no guessed model writes:
**capture the native command, patch only the values it is about to copy, let the
firmware keep ownership of files, history and pad assignment.** Details:
[HPSS-HANDOFF.md](../../HPSS-HANDOFF.md).

Add Reverb also uses this path. The native reader streams only source frames,
but its final writer callback accepts a mutable frame count. Add Reverb replaces
that final block with the remaining processed buffer and extends the block
count, letting the native WAV writer include the reverb tail without reading
beyond the source. Static trace and run-time validation are detailed in
[REVERB-HANDOFF.md](../REVERB-HANDOFF.md).

### Extending an Extract output at the writer boundary

The exact firmware path (Ghidra VAs; subtract `0x100000` for mod offsets):

| Function | Finding |
|---|---|
| `0x2a41f80` | Reads the source sample frame count from `sample+0x130+4`. |
| `0x21c6e40` | Streams provider blocks and stops when its current position reaches the reader length at `reader+8+4`; this is why requesting an end beyond the source did not produce extra blocks. |
| `0x1e8aff0` | Extract writer callback reads block frame count at `+0`, audio pointer at `+8`, then calls the concrete writer. |
| `0x1e93570` | Concrete WAV writer consumes `frames × channels` float samples and advances the output by the supplied frame count. |

The output can therefore be longer without fabricating source-reader frames:
substitute the final source block's audio pointer with the processed buffer at
the current output offset, raise that block's frame count to the remaining
rendered frame count, call the original writer synchronously, then restore the
original pointer and count before returning to native reader cleanup. The mod
marks the full processed buffer consumed only after this final writer call.
The emulator confirmed the concrete writer finalizes the WAV at the increased
length and native Extract still registers and assigns it to a free pad. Other
Extract calls pass through unchanged. Runtime acceptance is
`mod/tests/emulator_reverb.py`.

### Built-in H3 reverb effect

The firmware includes a native stereo effect class named
`Air::H3M::StereoReverb` (`StereoReverb` in its effect registry). Factory
`0x3d46910` maps the type to a thunk at `0x3d445b0`; that allocates the instance
and calls constructor `0x3dac6b0`. The constructor registers 18 parameters.
Main-vtable slot 11 (`0x3dabed0`) processes stereo audio, slot 9
(`0x3dad0b0`) prepares/resizes its buffers, and slot 21 (`0x3daca50`) applies
parameter updates. The internal topology and parameter labels/ranges are not
fully reconstructed. Add Reverb instantiates this native effect directly for
offline rendering. Full trace and reproduction commands:
[BUILTIN-REVERB.md](BUILTIN-REVERB.md).

Normal live firmware use is through the H3 patch graph: `FUN_03cf5180` parses each
`H3Part`, calls registry `FUN_03d46910` with its type and host context, creates
the effect wrapper/default parameter map, and sends `H3pc` and `H3ps` setup
messages before linking the effect. The graph processor `FUN_03d27ca0` walks
that chain and invokes each effect's vtable slot 11 (`+0x58`) on the stereo
buffers for each block. Add Reverb instead uses a standalone offline instance:
call factory `base+0x3c445b0` with `(NULL, &context)`, then vtable slot 9 with
`&context`, slot 11 per stereo block, and slot 1 to destroy. The context layout
is `{float sample_rate, uint32_t reserved, int32_t block_size}`. Slot 9's second
argument is a context pointer, not a boolean; passing `1` crashes because the
base setup dereferences it. This lifecycle was verified in the emulator. See
[the full H3 reverb trace](BUILTIN-REVERB.md).

### H3 instrument host boundary and Bass Synth

Ghidra's FMBass registration path is `FUN_03d46910`; its factory is
`FUN_03d45d50`, constructor `FUN_03e0af30`, parameter table `0x072acc20`, and
main vtable `0x07185c18`. Slots 9, 11, 13, and 21 are setup, render, event, and
parameter update. Unlike the effect classes above, FMBass requires a live H3
host object: its constructor calls `FUN_03cedde0`, which queries `H3pp`
(`0x48337070`) through host vtable slot 4. The effect audio-context struct does
not implement this host interface; using it for FMBass construction crashed
the emulator. The current **Bass Synth** menu item therefore renders a local
plucked C2 tone in `mod/src/bass_sample.c` and uses native Extract to assign its
WAV to the next empty pad. K1/K2/K3 control harmonic brightness, drive, and
low-pass cutoff. Details and validation are in [BASS-SAMPLE.md](BASS-SAMPLE.md).

Ghidra maps the standalone event and voice path. FMBass owns a 12-voice manager
at instrument `+0xc0` (active count at manager `+0x68`) and a queue at instrument
`+0x358` (size `0xe58`, read index `+0xe10`). The queue callback closure,
invoker, and active flag are at `+0xe28`, `+0xe30`, and `+0xe38`. The standalone
instrument callback at `+0x38` is null after prepare, so a correctly shaped
callback is needed to accept probe events. `FUN_03e0c1a0` drains the ring only
when active; it dereferences the closure and calls the invoker. Always validate
both pointers before a probe call.

Ghidra's program image base is `0x100000`; runtime offsets must subtract it.
Thus `FUN_03e0c1a0` at Ghidra VA `0x3e0c1a0` is called at `app_base+0x3d0c1a0`.
Using the VA directly as a runtime offset called unrelated code and crashed.
With the corrected offset, queue read indices advanced and slot-42 event
handling allocated a group through manager slot 2 (`FUN_03e0d0a0`). Note-on
dispatch (`FUN_03e0eb60` → `FUN_03e0ed60` → `FUN_03e0b430`) fans into child
voice vtable `0x06ed2040`. Entry `+0x10` (`FUN_03e068f0`) initializes the voice
DSP; entry `+0x20` (`FUN_03e060a0`) updates pitch/velocity; entry `+0x48`
(`FUN_03e08840`) renders. The parent FMBass renderer (`FUN_03e06450`) walks
active groups and invokes their render method at `+0x48`.

A corrected queue drain has succeeded in the emulator: one group and one valid
child were active, note-on and note-off read indices advanced, and the firmware
remained up. The first audio block and full native output were still exactly
zero. Other attempts crashed intermittently during dispatch, including cases
where the closure and invoker were non-null. Therefore the host callback and
pump are not a complete native lifecycle, and the intermittent dispatch fault
is not explained by a missing queue closure alone. Parameter-table defaults
for Amp Attack/Decay/Sustain/Release are `0/0.6/1/0.1`, and Master Volume is
about `0.707`; explicitly setting the amp envelope and master still yielded
silence. These findings disprove both the pitch-only event theory and the
zero-default amp theory.

The group stop bit at `+0xb` is clear, so it does not explain silence. The outer
wrapper renderer `FUN_03d27ca0` walks the graph node list at `graph+0x88`; this
pointer is null after prepare with both minimal FMBass XML and the exact
firmware-embedded FMBass preset XML. The allocated voice's DSP countdown at
`+0x3dc` stays at 1 after graph processing. This effect-chain wrapper has no
FMBass render node and cannot render the instrument by itself.

Ghidra RTTI scanning found `H3MPCInstance` (RTTI at `0x06fa3cf0`) and
`H3MPCSynthBase` (`0x06fa3eb8`). `H3MPCInstance`'s process and build methods
resolve to the same generic functions `FUN_03d27ca0` and `FUN_03d28620`.
`H3MPCSynthBase` derives from that class and from `VoiceManageable`, audio-plugin
parameter interfaces, an observer, and `juce::Timer`; the adjacent
`H3MPCVoiceManagedInterface` vtable contains abstract voice-management hooks.
This identifies the wider plugin object model, but not a safe offline FMBass
host/render lifecycle. `H3MpcSynthDemo` is a distinct synth implementation
whose constructor initializes its own voices. Do not infer that it can render
FMBass.

Direct event dispatch remains intermittent even with non-null queue closure
and invoker. Direct calls to the FMBass part renderer have not been proven safe.
Do not link `fmbass_sample.c` into the release build. Current Bass Synth
production uses the local C2 renderer in `mod/src/bass_sample.c`, assigns
through native Extract, and restores original source audio bytes after Extract
to defeat one-LSB PCM dither. The November 6 production emulator rerun passed
with a 65 Hz peak, 66,150 stereo frames at 44.1 kHz, source audio preserved,
pad assignment, and no restart. See [`BASS-SAMPLE.md`](BASS-SAMPLE.md) for the
full probe chronology and validation.

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
| control-surface / input | `buttons()`, `cc_capture()`, `data_signal()` | must not touch the model; publish a request only |
| UI / paint | `screen_paint()`, after the stock paint | the only place model calls are safe |
| worker | HPSS DSP and file I/O | never touches the model; commits through the native command on the UI thread |
| audio | playback; calls the marker resolver for the slice it plays; Bass Synth mixes into a private planar block before the native writer | do not touch the model, allocate, or lock |

Cross-thread hand-off is an atomic flag plus a phase machine, one phase per
paint pass. Evidence: [rt] — blocking the input thread and polling for a page
switch in the same callback both failed.

### Bass Synth live preview

The stock note-on/off hooks (`0x1e27360` / `0x1e270d0`) receive the pad index.
While SAMPLE OPTIONS is on its Bass Synth page, pad indices 0–15 queue C2–D#3
to the four-voice DaisySP preview. The hooked audio writer is `0x03f398f0`
(Ghidra VA `0x040398f0`): it receives planar float channels, converts them to
S32 interleaved output and calls `snd_pcm_writei`. The hook copies the planar
block into thread-local scratch, mixes the preview into channels 0 and 1, then
calls the original writer. This avoids changing the app's mixer buffers or
installing a second audio-device path. K1/K2/K3 updates are atomically read at
each block so active voices respond immediately. Evidence: Ghidra decompilation
and emulator counters showing pad notes, nonzero mixed audio, knob values and
no app restart; full details are in [BASS-SAMPLE.md](BASS-SAMPLE.md).

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

## 16. Live looper audio (input capture and device output)

| mechanism | detail | evidence |
|---|---|---|
| the capture stream | `RW_INTERLEAVED`, `S32_LE`, **4 channels, 44.1 kHz**, period 128, buffer 1024 (lab) | [rt] `/proc/asound/Loopback/pcm0c/sub0/hw_params` |
| input read site A | `FUN_04039450` (mod `0x3039450`) reads S32 with `snd_pcm_readi` and expands it to planar float; **not called** while the app monitors input | [static] + [hook] never fired |
| input read site B | `snd_pcm_readn` call at Ghidra `0xefbaa4` (mod `0xdfbaa4`) inside the app's own input thread; **not called** either | [static] + [hook] patched to a relay stub, 0 calls |
| what works | exporting `snd_pcm_readi`/`snd_pcm_readn` from the preload library and forwarding through `dlsym(RTLD_NEXT, ...)`: every read in the process lands there, whichever device object owns the pcm handle | [rt] ~8.5k reads in 25 s; loop recorded the tone source |
| output writer | `FUN_040398f0` (mod `0x3f398f0`) turns planar float into interleaved S32 and calls `snd_pcm_writei`; second argument is the planar channel array, third the frame count, fourth the offset | [static] decompile |
| output tap | interposing `snd_pcm_writei` gives the exact device block; mixing there reaches the jacks no matter which device object rendered the app's audio | [rt] mixed loop measured in the tap |
| pad dispatch | pad events are not in the button dispatch (no `btn 06` entries); they reach the app through observer dispatch `FUN_020ce3e0` (mod `0x1fce3e0`), which calls the note handler `0x1e27360` at `0x20ce480` and the pad-grid listeners | [hook] + [rt] |
| blocking the dispatch | dropping `FUN_020ce3e0` while the looper is open stops the pad-grid selection change, but also stops the note handler, so the looper never sees a pad; reverted | [rt] pad state stayed `IDLE` |
| SHIFT + pad shortcut | the app pairs `SHIFT` with a pad press for **Full Level**; the shortcut is upstream of the note handler, so the looper keeps `SHIFT` from the app while open | [rt] a "Full Level / Disabled" dialog appeared over the looper before this |
| lab PCM socket | `ws://127.0.0.1:5557` reported silence (bridge `out` levels 0.0) while the app wrote non-silent audio (in-process write peak 1.0); host-side PCM is not a trustworthy acceptance signal in this lab | [rt] |

The looper itself is documented in [LIVE-LOOPER.md](../LIVE-LOOPER.md).
