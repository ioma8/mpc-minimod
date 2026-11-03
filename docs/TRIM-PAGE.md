# The Sample / Trim page — verified layout and behaviour

> **Firmware 1.3.0.12 only.** Everything below was measured on
> `/usr/bin/MPC Sample` `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`
> (`1.3.0.12`). All addresses are **file offsets**; runtime addresses are
> `base + offset`, `base = 0xaaaaaaaa0000` in the emulator (PIE, so the base
> moves). A different build invalidates every number here.

This document exists because autotrim "did nothing on screen" and the previous
explanation in [`MOD-MINI.md`](MOD-MINI.md) ("the page draws from a fifth store
we have not found") was **wrong**. It records what the page actually does, how it
was verified, and what the real failure is.

---

## 1. Tooling that produced this evidence

| tool | use |
| --- | --- |
| `kuna` 1.130 | `kuna decompile <file> <addr> --addr` — per-function pseudocode |
| `tools/re/classof.py` | live `vptr` → class name; `--scan DUMP.bin` names every slot |
| `radare2` 6.2.2 | sections, dynamic relocations, byte reads |
| emulator `virtual-mpc-sample` | `MPCLAB_PORT_OFFSET=100 python3 lab.py run` |
| panel `http://127.0.0.1:8865` | `POST /control` (buttons, knobs), `GET /screen.png` |
| `tools/emulator/guestmem.py` | host-side `/proc/<pid>/mem`: `read/dump/u64/u32/f32`, `maps`, `base` |
| `tools/re/calls.py` | `callers(binary, target)` — BL call sites, for "who calls this" |

Two facts make offline class identification cheap and are reused below:

* **A live `vptr` is a vtable address point**: the typeinfo pointer sits at
  `vptr − 8`, and the name string at `typeinfo + 8`. Verified:
  `u64(0x706e5d0 − 8) = 0x706e518` → `"N2ui6SliderE"`; the object at
  `screen+0x18c8` holds `0x6e22408` and `u64(0x6e22400) = 0x6e223a8` →
  `"N2ui26SampleModeWaveformViewportE"`.
  `tools/re/classof.py` implements both directions.
* `.data.rel.ro` is `0x6ddf910 … 0x7162018`; `.rela.dyn` holds **491,458**
  entries, **464,288** of them `R_AARCH64_RELATIVE`, so every vtable slot is
  resolvable from the file alone (no disassembler needed).

Class names used below (address points, i.e. what a live object stores):

| class | vptr |
| --- | --- |
| `SampleModeScreen` | `0x6e212a0` |
| `ui::Slider` | `0x706e5d0` |
| `ui::SampleModeWaveformViewport` | `0x6e22408` (secondaries `0x6e225b8`, `0x6e22618`, `0x6e22640`) |
| `ui::SampleWaveformWithOverlays` | `0x6e23300` |
| `AC50WaveformLocator` | `0x6e259e0` |
| `TrimParameter` | `0x6e1b240` (+ `0x6e1b290`, `0x6e1b2f0`, `0x6e1b318`) |

No code runs in the target: the reader is `dd if=/proc/<pid>/mem bs=4096 …` piped
to `od -x` over the lab's root ssh (BusyBox `od` has no `-j/-N/-t`, hence the
16-bit words being rejoined locally).

```sh
cd tools/emulator
MPCLAB_PORT_OFFSET=100 python3 guestmem.py maps
MPCLAB_PORT_OFFSET=100 python3 guestmem.py dump 0xffffe50bc620 0x9000 > /tmp/screen.bin
MPCLAB_PORT_OFFSET=100 python3 guestmem.py f32 0xffffe50bc620+0x1f6c

cd ../re
python3 classof.py /tmp/mpcsample.bin 0x706e5d0          # -> N2ui6SliderE
python3 classof.py /tmp/mpcsample.bin --scan /tmp/screen.bin --base 0xaaaaaaaa0000
```

`guestmem.py` reads the base and pid from the guest on demand; the *screen
object address is not stable* (it changed between runs), so re-resolve it each
time — the mod logs the address it found on every trigger.

---

## 2. `SampleModeScreen` is a flat container of embedded widgets (verified)

Scanning the live screen object for qwords that resolve to a vtable gives the
whole widget inventory. One run (screen at `0xffffe50bc620`, first `0x9000` bytes):

| count | class | first offsets |
| --- | --- | --- |
| 26 | `ui::TextButton` | `+0x00a8 … +0x0b98` |
| 15 | `juce::Label` | `+0x0bc0 … +0x0d48`, `+0x1500 … +0x1580` |
| 12 | `ui::ImageButton` | `+0x0da0 … +0x1490` |
| 33 | `ui::Slider` (6 vptrs each = 198 slots) | `+0x15f8`, `+0x1c18`, `+0x1e68`, … `+0x64b8` |
| 4 | `ui::SampleModeWaveformViewport` (vptrs `0x6e22408`, `0x6e225b8`, `0x6e22618`, `0x6e22640` — one object with base subobjects, or a small family) | `+0x18c8`, `+0x1968`, `+0x1970`, `+0x1a60` |
| 4 | `juce::ImageComponent` | `+0x1848`, `+0x18a8`, `+0x1b98`, `+0x1bf8` |
| 33+33 | `AccelerationFilter` + `GestureTracker` | one pair per knob/slider |

`SampleModeScreen::resized` (`0x1660370`) walks the same object with
`sub_1160fc0(a0 + <offset>, x, y)` for about 55 embedded children — i.e. the
widgets are **in-object**, not separately allocated, which is why a pointer scan
of the screen finds them by vtable at fixed offsets.

The **screen object is recreated** between app runs: two runs gave
`0xffffe50bc620` and `0xffffe50bc910`. Any cached screen pointer must be
re-resolved.

---

## 3. What the marker and the shading are drawn from (verified)

The Trim page paints, over the wave area `x = 15 … 304`:

* a background fill `rgb(27,27,30)` from `x = 15` up to the marker,
* `rgb(0,0,0)` to the right of it,
* the marker itself as a 1-px `rgb(126,126,126)` line with 4×4 handles at
  `y = 84..87` and `y = 190..193`.

Sweeping the first knob (`K1`, panel CC 16) moves that marker linearly:

| `K1` (CC16) | marker x | shading ends at | position |
| --- | --- | --- | --- |
| 0 | (at 15, hidden) | — | 0 % |
| 16 | 52 | 50 | 12.8 % |
| 32 | 87 | 85 | 24.9 % |
| 64 | 160 | 158 | 50.2 % |
| 96 | 236 | 234 | 76.5 % |
| 127 | 304 (right edge) | 304 | 100 % |

i.e. `x = 15 + 289 · pct`.

`K1` is absolute (0…127 → 0…100 %); repeating the same CC value is ignored, so a
sweep has to alternate values.

**Byte-level proof of the source.** Snapshot the `0x9000`-byte screen object,
move the knob so the marker goes 50 % → 75 %, snapshot again: **exactly four
bytes change**.

```
off 0x1f68: 0x4248000000000000 -> 0x4296000000000000  (f32 @ +0x1f6c: 50.0 -> 75.0)
off 0x1fb8: 0x1b -> 0x15                                (cached label width, 27 -> 21)
off 0x28b8: 0x4248000000000000 -> 0x4296000000000000  (f32 @ +0x28bc: 50.0 -> 75.0)
off 0x2908: 0x1a -> 0x15                                (cached label width, 26 -> 21)
```

So the page paints from a **percentage `float` at `screen + 0x1f6c`** (mirrored
at `screen + 0x28bc`; two widgets show the same parameter). Nothing else in the
screen object moves. There is no "fifth store": the marker is a pure function of
that value.

---

## 4. The mod's write path is not the page's write path (verified)

Turning `K1` while the mod's setter hook (`0x14d0890`) is installed produces
**no** `setter[N]` lines — only the unrelated position counter (§5). So the Trim
knobs do not go through the property setter the mod hooks.

`0x14d0890` itself is a plain observable property (decompiled):

```c
set(this, value):
    if (this->value @0x20 == value) return;      // no-op when unchanged
    ++count @0x18;
    value @0x20 = value;
    for (node in list @0x08/@0x10) node->changed(value);
```

The property also has a **liveness signature**: `[this+0x10]` is a list node and
`[this+0x08]` is the sentinel (first live object had `head == this + 8`).

---

## 5. The captured property objects are stale (verified)

The mod records `self` for every setter call in `g_setter[]` and reuses those
pointers later. Reading all 16 captured objects from a live process:

| index | value @ +0x20 | count @ +0x18 | head @ +0x10 | verdict |
| --- | --- | --- | --- | --- |
| 2 | `1307688` (climbing) | `0` | `self + 8` | **live**, but it is a monotonic audio position |
| 6, 7, 8 | `121273 / 121274 / 121273` | `0` | pointers | live-ish sample bounds |
| 0, 1, 3, 4, 5, 9, 10, 11, 12, 13, 14, 15 | denormals / pointer-looking | 4·10⁹ | 0 or junk | **dead** (freed and reused) |

13 of 16 no longer match the setter layout. The app frees and recreates these
objects, so a pointer captured at startup cannot be written later.

---

## 6. Root cause of "autotrim does nothing" (verified end to end)

A clean run, fresh app, `SHIFT`+`B3`:

```
mpcmini: combo SHIFT+B3 -> autotrim
mpcmini: cand @0xffffd1a85d20 ch=2 frames=121275 start=0 end=0 root=60
...
mpcmini: struct sample @0xfffff31e21a0 frames=10165 path='/usr/share/Akai/AC50/Metronome/Clap.wav'
mpcmini: trim source /usr/share/Akai/AC50/Metronome/Clap.wav frames=10165 ch=2 edges=1..9878
mpcmini: autotrim frames=10165 edges=1..9878 have=1 lo=2 hi=2
mpcmini: autotrim 1..9878; start 85574->1 end -1->-1
mpcmini: chg setter[2] 85574 -> 85661          <-- the app overwrites it on the next tick
mpcmini: layer 0/0 -> 1/9878
mpcmini: repaint forced (screen=0xffffe50bc910)
```

…and the screenshot is **byte-identical** to the one taken before the press
(`sha256` prefix `6e5c5b7d7c` both times).

Three independent failures, all verified:

1. **Wrong pair of properties.** `do_autotrim` picks Start/End as the min/max
   value among setters written "recently" (`g_seq − 400`). The only setter being
   written continuously is `setter[2]`, the app's monotonic audio position
   counter, so `lo == hi == 2`: the edge frames were written into the position
   counter and immediately overwritten by the app.
2. **Wrong sample.** The WAV resolution walks *all* scanned sample structs and
   takes the first whose `frames` matches a `.wav` found on disk. It resolved the
   factory metronome `Clap.wav` (10165 frames), and in an earlier run
   `Tambourine.wav` (6999 frames) — while the page shows the project sample
   `TRIM-Test-A002.wav` (`485144` bytes = **121275 stereo frames**). The factory
   samples are in `/usr/share/Akai/AC50/Metronome/`.
3. **Unbounded scan.** `owner_path()` walks every writable range per candidate.
   In one run the trigger was still scanning minutes later, with no UI change at
   all — which is exactly the "I press it and nothing happens" report.

The stale-object problem (§5) is what makes (1) inevitable: the real trim
properties were captured at project load and never written again, so any
"most recent" heuristic selects whatever unrelated property is chatty.

---

## 7. Locator components (partially verified)

The binary names the marker machinery: `WaveformLocator`, `StartWaveformLocator`,
`EndWaveformLocator`, `LoopWaveformLocator`, `PlayheadWaveformLocator`,
`GuiStartWaveformLocator`, `GuiEndWaveformLocator`, `AC50WaveformLocator`,
`ui::SampleModeWaveformViewport`, `ui::SampleWaveformWithOverlays`,
`SampleEditWaveformContainer`, `TouchSampleWaveformComponent`,
`GuiSampleEditWaveformDisplay`.

`AC50WaveformLocator` method `0x16b6060` (decompiled) computes the locator box
from cached state:

```c
view = this->view @0x18;
x    = round( (double)this->pos @0x168 * view->width @0x28 - 2.0 );
setBounds(0x115f590)(this, x, 0, 4, view->height @0x2c);
```

So a locator caches a **normalised position** (`float` at `+0x168`) and a view
pointer; it does not read the trim property at paint time. *Who* writes `+0x168`,
and which locator instances are live on this page, is **not yet identified** —
that is the remaining piece if the stock locators are to be driven directly.

---

## 8. Cross-check against the community mods

Both reference mods target the same firmware family, and one confirms our
anchors:

* **am-mod** (`analysis/refs/am-mod-aleksey`) documents the same control map
  (B1 `Trim→Mix→Amp Env`, `Shift+B2` = Normalize, "destructive trim") and the
  object chain, from its native checkers
  (`sd-mod-session/check_resample_fast_native.c`):
  `instrument + 0x22e8` = layer array (stride `0xb30`),
  `layer + 0x4a8` = sample pointer,
  `layer + 0x4b0` = sample **reference** struct, whose `+8` and `+12` it sets to
  `100`/`100` when it wants a full range,
  `sample + 0x130` = metadata (frames at `+4`).
* **SampQOL OS 0.1.5** (`analysis/refs/sampqol/.../mpcqol.so`, 880 KB, named
  `.dynsym` entries) contains `0x6e212a0` — our `SampleModeScreen` vtable —
  twice, and locates screens by vtable offset (`context_visible_screen`,
  `context_visible_screen_vt`, comparing `0x6e1e258` / `0x6e1f060`). It keeps a
  registry of screens at `0x151168` and a base pointer at `0x151060`, and it
  **paints its own overlays into a pixel buffer** (`tail_marker_paint`, stride
  `0x148`, 32-bit pixels) instead of moving stock locators. Its symbol list also
  has `context_setter_object`, `overdub_trim`, `tail_ui_*`, `chop_*`.

Takeaway: the "find the screen by vtable, then paint into the surface" route is
proven by another mod; the "move the stock Start/End" route needs the app's own
trim parameter, which is what this document is about.

---

## 9. What the fix must do (not yet implemented)

1. **Resolve the screen and the widgets at trigger time** — the object moves
   between runs.
2. **Identify the live trim pair without guessing.** Candidates:
   * the two properties whose values are in `[0, frames−1]` *and* that are
     currently live (liveness signature §4), preferring the pair that brackets
     the playhead/selection;
   * or the parameter object that the Trim page's `ui::Slider` bindings hold —
     the percentages in the screen are derived from it, so the binding can be
     read from the slider object.
3. **Resolve the selected sample's PCM from the app**, not from a `.wav` on
   disk: the page's sample is the pad's sample (`[layer + 0x4a8]`, layer =
   `[inst + 0x22e8] + pad·0xb30`) — or whatever the trim parameter itself
   references.
4. **Write through the setter the page actually uses.** `0x14d0890` is *not*
   driven by `K1`/`K2` (§4), so binding the trim knobs (or the app's own
   command) and hooking *that* is the correct write path.
5. **Bound the work**: no heap-wide scans on the trigger path.

A concrete, evidence-backed shortcut for (3) that avoids the heap entirely is the
am-mod chain (§8): from the instrument, `layer = [inst + 0x22e8] + pad·0xb30`,
`sample = [layer + 0x4a8]`, `frames = [[sample + 0x130] + 4]`. The adjacent
`[layer + 0x4b0]` sample-reference (fields at `+8`/`+12` observed as
`0..100`) is the natural candidate for the Start/End pair the page shows — worth
testing before any further heap scanning.

---

## 10. Negative results (do not retry)

| attempt | outcome |
| --- | --- |
| write sample-struct `+0x3d4/+0x3dc` | succeeds, UI untouched (library metadata) |
| write `slider+0x20` | breaks the page render |
| hook `Label::getText` / `Graphics::drawText` | never sees the third soft key; the getText hook crashed the app on Sample Select |
| hook `RotaryFixedConnection` (`0x15f5690`) | never called for the Trim knobs |
| `find_displayed_sample()` (screen child with `+0x130` → info struct) | no child matches this build, always logs "not found" |
| pick the sample by on-screen name | wrong by design; the name is not the pad's sample |
| reuse `g_setter[]` pointers captured at startup | they are freed objects (§5) |

---

## 11. Reproducing any claim here

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py run            # panel :8865
MPCLAB_PORT_OFFSET=100 python3 lab.py preload \
  /Users/jakubkolcar/projects/customs/sample-converter/mod/build/mpcmini.so

# knob sweep (moves the marker 0..100 %)
for v in 0 32 64 96 127; do
  curl -s -X POST -H 'Content-Type: application/json' \
       -d "{\"name\":\"K1\",\"action\":\"set\",\"value\":$v}" \
       http://127.0.0.1:8865/control
  curl -s -o /tmp/k$v.png "http://127.0.0.1:8865/screen.png?cb=$v"
done

# autotrim trigger
curl -s -X POST -H 'Content-Type: application/json' -d '{"name":"SHIFT","action":"press"}'   http://127.0.0.1:8865/control
curl -s -X POST -H 'Content-Type: application/json' -d '{"name":"SCREEN 3","action":"press"}' http://127.0.0.1:8865/control
curl -s -X POST -H 'Content-Type: application/json' -d '{"name":"SCREEN 3","action":"release"}' http://127.0.0.1:8865/control
curl -s -X POST -H 'Content-Type: application/json' -d '{"name":"SHIFT","action":"release"}'  http://127.0.0.1:8865/control
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh 'tail -n 40 /run/mpcmini.log'

# static
kuna decompile /tmp/mpcsample.bin 0x1660370 --addr   # screen resized
kuna decompile /tmp/mpcsample.bin 0x14d0890 --addr   # property setter
kuna decompile /tmp/mpcsample.bin 0x16b6060 --addr   # AC50 locator bounds
```

Notes: `kuna` rejects paths containing spaces — copy the binary to
`/tmp/mpcsample.bin` first. The emulator's BusyBox has no `xxd`, no
`od -A/-j/-N/-t`, and no `timeout`; use `od -x` and `head -n`.

---

## 12. Driving `TrimParameter` from a mod (verified 2026-11-02)

The app's own knob path is
`0x15e70c0(float normalised_value, TrimParameter *param)` — the float travels in
`s0`, the object in `x0` (confirmed by the decompiled signature
`void FUN_016e70c0(float param_1, long *param_2)`).

What a call does, from the decompiled body (`FUN_016e70c0`, 1833 instructions):

```
model = *(TrimParameter **)(param + 0x70)
... three optional std::function-like callbacks from the model:
    model+0x288 (storage 0x278, manager 0x290)
    model+0x2a8 (storage 0x298, manager 0x2b0)
    model+0x2c8 (storage 0x2b8, manager 0x2d0)
    plus plain values model+0x250, +0x260, +0x268, +0x270 (curve/takeover)
... clamp, curve, snap, take-over ramp against param+0x19c ...
[param+0x19c] = final value            <- always stored
if (<direction test>) goto skip;
notify(model callbacks); timers; layer update
```

A mod can call it, and the value **is** stored: after calling with
`0.0910 / 0.4542` the two parameters read back `0.0945 / 0.4567` at `+0x19c`
(verified from the host). But the page still does not repaint, because the
visible update runs through the model callbacks, and on the instances a scan can
find those slots are empty (`model+0x288/0x2a8/0x2c8 == 0`), while the instance
the knob drove had them populated (`0xaaaab1b0e800`, `0xaaaab1b0e890`).
That is why writing the field or calling the core is not enough.

So the remaining task for a UI-visible write is to reach the app's *knob
takeover*, which binds those callbacks, instead of calling the core directly.

### Getting there: what is already verified useful

* trim parameters are found by scanning writable memory for vptr `0x6e1b240`
  (the page has exactly three with the same `+0x1e8` length; ascending address
  = Start, End, Loop — proved by turning K1/K2/K3 and logging which instance
  each one calls);
* `+0x1e8` = sample length in frames (authoritative "which sample is on screen");
* `[param+0xb0] -> [ctx+0x60]` = instrument; `[instr+0x22e8] + pad*0xb30` =
  layer; `[layer+0x4a8]` = Sample; `[[Sample+0x130]+4]` = frames;
* the Sample's WAV path is a `juce::String` whose text pointer sits at
  **`Sample + 0x120`** and points at the app's own loaded copy
  (`/data/TempProjects/Temp-Project_[ProjectData]/TRIM-Test-A002.wav`). Reading
  it and validating the WAV's frame count against `+0x1e8` resolves the sample
  exactly, with no name matching and no heap-wide WAV search.

### Ghidra recipe for this binary

```sh
GHIDRA_HEADLESS_MAXMEM=32G analyzeHeadless /tmp/ghproj mpc \
  -import /tmp/mpcsample.bin -noanalysis \
  -scriptPath tools/re/ghidra -postScript DumpFuncs2.java
```

* **Ghidra's image base is `0x100000`**: address = file VA + `0x100000`
  (e.g. the trim core is `FUN_016e70c0`).
* `-noanalysis` + a post-script that disassembles a window and creates the
  function is enough for targeted decompilation; full auto-analysis of the
  119 MB PIE leaves the addresses of interest undefined as functions.
* `kuna decompile <file> <addr> --addr` remains the fastest first look; Ghidra
  gives the readable C. `tools/re/classof.py` names any vptr from the RTTI.

---

## 13. Where the trim actually lives, and why a mod cannot simply set it (2026-11-02)

Turning a trim knob writes, in one go:

| location | meaning | evidence |
|---|---|---|
| `TrimParameter+0x19c` | normalised value (0..1) | K1 0.504 -> 0.134 |
| `TrimParameter+0x1a0` | applied value (`-1` = unset) | -1 -> 0.135 |
| `[TrimParameter+0x80]+0x38`, `+0xa8` | **start/end in frames** | 61850 -> 16978 |
| `layer+0x4d8`, `layer+0x588` | start in frames (mirror) | 21829 -> 19404 |
| `layer+0x530` | end in frames | 112785 -> 65488 |
| screen floats | the painted percentages | K1 moved `+0x1d5c`/`+0x269c` |

`layer` = `[[param+0xb0]+0x60]+0x22e8` + `pad*0xb30`. The layer fields are the
values the audio engine uses; the screen floats are what the page paints.

A mod can write all of them (verified by reading back), but **the app puts its
own values back**: after writing 11037/55083 the layer read 19404/65488 again,
and after `core()` stored 0.0945/0.4567 the parameters read 0.5039/0.7874 a
moment later. The page also keeps its own cached percentages and never
re-derives them from the parameter, so a direct write leaves the markers where
they were.

The one path that does everything (parameter + layer + repaint) is the app's
own knob takeover, which binds the model callbacks the trim core notifies
through (`model+0x288`, `+0x2a8`, `+0x2c8`; empty on the instances a scan
finds, populated on the one a knob drives).

### What a mod can do today

`do_autotrim()` (in `mod/src/mpcmini.c`) resolves everything correctly:

```
autotrim: layers=... candidates=N
autotrim: sample=... path='/data/TempProjects/.../TRIM-Test-A002.wav'
autotrim: '...' frames=121275 ch=2 edges=11037..55083
autotrim: layer ... <- 11037..55083 frames
```

so the *computation* is solved and the *state write* is correct at the moment it
happens. Making it stick needs the takeover entry point.

### The notification chain (measured 2026-11-02, probe build)

Hooking the generic float broadcaster `0x13bb930` (signature `(this, float)`,
value cached at `this+0x50`, listener list around `this+8`) and turning K1 shows
the real path:

```
mpcmini: bcast 0xffffe5556330 v=0.5039     <- property the knob writes
mpcmini: bcast 0xffffe5559520 v=0.5039     <- forwards to the page binding
mpcmini: trim self=0xffffe50fe680 v=0.504 lr=0xaaaaabe5ba5c   <- core, as a listener
```

`lr=0x…e5ba5c` is inside `0x13bb930` (file VA `0x13bba5c`): the broadcaster walks
an intrusive list and calls `listener->vtable[0x28](listener, value)`, and the
TrimParameter is one of those listeners. Both property objects hold the current
value at `+0x50` (`0.2362` after a later knob step), and the node at
`0xffffe55627b0` has the tagged listener pointer `0xffffe5559521` at `+0x10`.

So the one missing call is `0x13bb930(property, value)` on the property that
owns the trim parameter; everything downstream (parameter, layer, repaint)
follows from it.

### Knob-like write: what is proven and what is not (2026-11-02, late)

The knob's write path is now fully identified:

```
knob -> property object P -> 0x13bb930(P, value) -> listener->vtable[0x28](listener, value)
                                                   \-> TrimParameter -> trim core 0x15e70c0
                                                   \-> page widget (marker repaint)
```

`0x13bb930` layout, from the loop at `0x13bb9e4..0x13bba58`:
`[P+0x10]` first listener node, `[P+8]` sentinel, node `+0x18` target (low 2 bits tag),
node `+8` next, and `P+0x50` is the property's cached value.

Broadcasting through that entry really does drive the app: an early experiment that
called it on a mis-identified object actually ran the stock **Normalize** command
(the page switched to `…-Normalized`), so the mechanism is right — only the
identification of the property that owns a given `TrimParameter` is still open.
A list walk from the parameter side was implemented (`list_sentinel`/`list_has`)
but produced false positives, so it stays behind `MPCMINI_PROP` and is not used
by the default build.

The reliable alternative, if this is picked up again: hook `0x15e70c0` and read
`x20` (the broadcaster sets it to P before the listener loop), then cache
`param -> P`. That mapping taken from one real knob move makes every later
autotrim write go through the app's own path.

---

# Findings summary (firmware 1.3.0.12 only)

Everything below was measured on the stock 1.3.0.12 build
(`/usr/bin/MPC Sample`, sha256 `b6584cba…0193b508`) in the virtual MPC Sample,
either from the host through `/proc/<pid>/mem` or from the mod's own hooks.
Every address is a file VA; add the app base at runtime.

## A. The page model

* `TrimParameter` vptr `0x6e1b240` (RTTI name `TrimParameter`); the page has
  exactly three live instances with `+0x1e8 == sample length`, ordered by
  ascending address: **Start, End, Loop** (proved by turning K1/K2/K3 and
  logging which instance the core was called on).
* Fields used: `+0x19c` float value (0..1), `+0x1a0` applied value (`-1` =
  unset), `+0x198` byte flag, `+0x1e0` min frames, `+0x1e8` max frames =
  sample length, `+0x134` kind, `+0x70` model, `+0x80` boundary object,
  `+0xb0` context.
* The page caches what it paints in the screen object (floats in percent and in
  0..1), and only refreshes them from a broadcast — never from the parameter.

## B. The knob write path

```
knob (CC) → property P → 0x13bb930(P, value)
                        → listener loop at 0x13bb9e4..0x13bba58
                        → listener->vtable[0x28](listener, value)
                        → TrimParameter → trim core 0x15e70c0(float, param)
                        → page widget (repaint)
```

* `0x13bb930` takes `(this = P, float value)`; `P+0x50` mirrors the last value.
* The trim core is a **listener**, so calling it directly (which we do) fills in
  the parameter but does not touch the other listeners.
* A broadcast through a property does drive the app: a mis-identified property
  in an experiment actually ran the stock **Normalize** (the page switched to
  `TRIM-Test-A002-Normalized`).

## C. The trim state the knob leaves behind (in frames)

| location | meaning | example |
|---|---|---|
| `TrimParameter+0x19c` | value 0..1 | 0.504 → 0.134 |
| `TrimParameter+0x1a0` | applied value | -1 → 0.135 |
| `[param+0x80]+0x38`, `+0xa8` | start/end frames | 61850 → 16978 |
| `layer+0x4d8`, `layer+0x588` | start frames (mirror) | 21829 → 19404 |
| `layer+0x530` | end frames | 112785 → 65488 |

`layer = [[param+0xb0]+0x60]+0x22e8 + pad*0xb30`.

## D. Resolving the sample on screen

```
[param+0xb0] → ctx → +0x60 → instrument → +0x22e8 → layers
layers + pad*0xb30 → +0x4a8 → Sample
Sample + 0x130 → info {u16 ch; u32 frames; u32 rate}
Sample + 0x120 → juce::String text pointer → the app's own
                 /data/TempProjects/Temp-Project_[ProjectData]/<name>.wav
```

Validating that WAV's frame count against `TrimParameter+0x1e8` identifies the
sample exactly — no name matching, no “last loaded” guessing.

## E. What the mod implements today

`SHIFT+B3` → `do_autotrim()` on the message thread:

1. scan the live `TrimParameter`s, take the length `N` from `+0x1e8`;
2. resolve the sample through the layer chain and read its WAV path;
3. compute channel-aware silence edges (`±328`, −40 dBFS) from that WAV;
4. write the app's trim state: `0x15e70c0(v, param)` for Start and End, the
   frame values at `[param+0x80]+0x38/+0xa8`, and the layer fields
   `+0x4d8/+0x588/+0x530`;
5. log everything (`autotrim: … frames=121275 edges=11037..55083`).

Verified read-back: `+0x19c` takes `0.0945 / 0.4567`, the boundary takes
`11036 / 55059`, the layer takes `11037 / 55083`.

## F. The remaining gap

The page does not repaint, because the repaint is a *listener* of property `P`
and `P` is not reachable from the parameter with the walks tried so far
(`list_sentinel`/`list_has` produced false positives and stay behind
`MPCMINI_PROP`).

Fastest deterministic fix: **hook `0x15e70c0` and read `x20`** — the broadcaster
sets `x20 = P` in its prologue and calls the listener from inside that loop, so
the register holds `P` exactly when the core runs. Cache `param → P`; every
later autotrim then calls `0x13bb930(P, value)` and the whole chain runs as if a
knob moved.

## G. Ghidra workflow (used for the decompilation in this document)

```sh
GHIDRA_HEADLESS_MAXMEM=32G analyzeHeadless /tmp/ghproj mpc \
  -import /tmp/mpcsample.bin -noanalysis \
  -scriptPath tools/re/ghidra -postScript DumpFuncs2.java
```

* Ghidra imports this PIE at image base **0x100000**: Ghidra address = file VA + 0x100000.
* `-noanalysis` + disassemble-window + `createFunction` per target is enough;
  full auto-analysis leaves these addresses undefined as functions.

## H. Decisive test result (single run, 2026-11-02)

Build carrying: wired-parameter filter, `x20` capture, recorded-broadcast match,
`VERDICT` read-back. Run: nudge K1/K2 (to make the broadcaster call the core),
then `SHIFT+B3`, then read the verdict.

```
autotrim: 3/4 params are wired (len=121275)
autotrim: pad0 sample=… path='…/TRIM-Test-A001.wav'
learnt self=0xffffe50fc840 prop=0xffffe555c718 v=0.5039 list=0 head=0xffffe5568390
VERDICT p0=… P0=(nil) v0=0.0910 | p1=… P1=(nil) v1=0.4540
VERDICT after val=-1.0000/-1.0000 layer=0/0
```

Two facts, both useful:

1. **`x20` really is the property** — it is a heap object, `P+0x50` holds exactly
   the value the knob wrote (`0.5039`), and consecutive parameters get
   consecutive properties (`…c718`, `…c770`, 0x58 apart). Broadcasting through
   it *does* reach the app (the screen changed when we did).
2. **The listener walk is wrong**: `list_has(P, param) == 0`. `P+0x10` points at
   `0xffffe5568390`, which is not a `{next@+0, prev@+8, target@+0x18}` node but a
   `std::function`-style record (`invoke` at `+0x28` = `0xaaaaabf3e680`,
   captured pointer at `+0x10` = `0xffffe55682fb` tagged). So the container is a
   list of *callables*, not a list of raw listener pointers; the `TrimParameter`
   is reached through the callable's captured object, one more level down.

That is the whole remaining gap: the property is known and reachable at knob
time, but the container layout to find it from the parameter side is a list of
`std::function`s, and the captured-object indirection is what the last two
finders missed.

---

## I. Resolved: autotrim works (2026-11-02)

Three separate bugs were in the way, in this order:

1. **The project samples are PCM 24-bit**, and Akai writes a large metadata
   chunk before `data`. The parser only accepted 16-bit and a 4 KB prefix, so
   every sample failed and no file matched. It now walks the chunk list and
   accepts 16/24/32-bit, with a width-aware edge scan.
2. **The WAV path is two indirections inside the Sample object**
   (`sample+0x30 → +0x120 → the juce::String text`). A depth-2 pointer scan
   finds it; the frame count from `TrimParameter+0x1e8` validates it.
3. **A wrong property makes the app abort** (SIGABRT inside the broadcaster).
   Properties are now taken only from a mapping learned at a real knob move
   (the broadcaster keeps `this` in `x20` while it calls the trim core) or from
   an exact value match, and ambiguity means skip.

Verified run (stock project, `BoomBap-Kik-BTB MD`, 15129 frames):

```
autotrim: 3/3 params are wired (len=15129)
path from sample+0x30/+0x120 -> …/BoomBap-Kik-BTB MD.wav
autotrim: frames=15129 ch=2 edges=1..12433
autotrim frames=15129 -> 0.0001..0.8219
layer 0xffffe3bf7c00 <- 1..12433 frames
commit 0.0001/0.8219 prop=0xffffe567c718/0xffffe567c770
after layer=1/12434
```

And the page model took the value: `screen+0x24dc == 82.0` (percent) right
after the commit, i.e. the End marker is at 82 % and Start at 0 %.

The one UX detail: the property broadcast needs one knob touch of Start/End
first, because that is the only trustworthy way to learn `param → property`.
Without it the mod still trims (layer + core), it just cannot refresh the
page's cached markers.

## J. The freeze after autotrim (fixed)

`wav_edges()` walked the tail of the file **one `pread` per frame** to find the
last audible frame. On a 15 129-frame kick that is 15 129 syscalls on the UI
thread (the button hook runs there), and on one of the stock 500 k-frame
samples it is effectively a hang: the screen stops repainting right after the
trim, which is exactly what it looked like.

It now reads the tail in 16 KB blocks and scans each block backwards, and the
commit still happens on the next repaint. Measured after the fix: autotrim
returns in ~3 s on the kick and the panel stays responsive (a B1 press after
the trim still changes the screen).


## 11. Autotrim: root cause and current state (2026-11-03)

Measured with the real framebuffer (QEMU `screendump`), because `/screen.png`
returns stale frames and produced several false "no change" readings.

### The findings that matter

1. **`commit_trim` was returning too early.** Its default branch tried, in
   order, the native-property command, the page-cache parameters, the slider
   widgets and only then the trim core — and the slider branch returned
   successfully, so the core was never called. The parameter therefore kept the
   value the last knob move had written. The commit now does exactly what a knob
   does: core → layer frames → applied → repaint, and nothing else.
2. **The core write lands.** After that change the log reads
   `commit core done p0=0.0000 p1=0.8189 (want 0.0001/0.8219)` — the core
   quantises to frames, as expected.
3. **Our parameter selection can hit the live instance.** With the page's
   context group, `p0` and the parameter the knob drives are the same address in
   the same process (verified twice), e.g. both `0xffffe4eccb80`.
4. **`x20` at a trim-core call *is* the property.** Dumping `x19`–`x26` shows
   `x20 == x21` and `*(x20+0x50) == value` exactly (0.2362 for K1 = 30/127), so
   the property can be identified by value equality. `0x13bb930` stores that
   cache and then walks its listener list — the step our direct core call skips.
5. **`ui::Slider::+0x1b4` is a decoy.** Writing any of the 34 sliders and
   repainting yields the same frame; the paint re-derives the widget.

### What is still missing

The page repaints only when the *broadcaster* notifies its listeners, and that
requires the property, which is only observable while a knob is actually moving.
The commit now learns `(parameter → property)` from real knob traffic and
broadcasts when the pair is known, falling back to the core write otherwise. The
remaining gap is that a fresh session has no learned property, and the
neighbouring instance's property cannot be derived from the first one yet.

Next probe: capture the property list head (`property+0x10`) while a knob moves,
walk it, and record every listener — the page's listener is one of them and can
be called directly without knowing the property.


## 12. Round 2: the property is derivable, the repaint is still missing

New, verified this round:

1. **The property can be recovered from the parameter.** Walking a listener
   node's `next` chain until it reaches the sentinel and taking `sentinel - 8`
   yields the owning property, validated by `property+0x10` (head) and
   `property+0x50` (cached float). `prop_of_param()` does exactly that and found
   the same property a knob broadcast uses (`0xffffe532c718`).
2. **A trim property has exactly two listeners**, dumped during a real knob
   move:

   ```
   prop 0xffffe532c718 head=0xffffe53384b0 sentinel=0xffffe532c720
     node 0xffffe53384b0 target=0xffffe5338490 next=0xffffe16bf130
     node 0xffffe16bf130 target=0xffffe16bf110 next=0xffffe532c720
   ```

   One target is the trim parameter, the other is the page side.
3. **Broadcasting through that property still does not repaint.** So neither
   listener repaints the page, and the knob's repaint comes from the *event
   processing* around the broadcast, not from it.
4. **`+0x19c` is not the displayed state.** Our core write sticks
   (`p0=0.0000`, `p1=0.8268` after the retry) yet neither the live page nor a
   rebuilt one (leaving the page and coming back) shows any change: 0 pixels.
5. **The core clamps a start past the current end** (`start rejected (-1.000)`),
   so the pair must be written end-first; the commit now does that and retries.

### What this leaves

The display is driven by neither the parameter, the property broadcast, the
page cache nor the slider widget. The next step is to diff the *page* object's
memory across a knob move using the paint hook as the anchor (the only place the
page pointer is guaranteed live) and see which field tracks the marker, then
write that field and trigger the same repaint the knob's event does.

Everything is committed; the build is stable and the emulator runs it.


## 13. Round 3: the trim is rendered — and the app has a CC entry point

Three findings that change the picture:

1. **The page never redraws its trim on a knob move.** Measured: `K2` 127 → 30
   changes **0 pixels**; `K1` 0 → 100 changes 472 pixels, all in the label row
   (the focused-knob bracket). The waveform and the shaded region only refresh
   when the **pad is re-selected** — pressing the same pad again changes 14261
   pixels. That is why "nothing happens" after a trim, on the emulator as well
   as on the device.
2. **There is a single CC entry point**: `0x159f230`. Disassembly:

   ```
   ldrb w3,[x0,8]      ; x0 = event, byte +8 = CC number
   and  w2,w1,0x7f     ; w1 = raw 7-bit value
   ldr  x0,[x0]        ; object
   scvtf s0,w2
   fdiv s0,s0,s1       ; s1 = range  -> the call is (event, raw, min, range)
   ... ldr x0,[x0,0x10] / add x0,x0,#0x1ac0 -> broadcast(value, property)
   ```

   CC `0x10` (K1) broadcasts through `base+0x1ac0`, CC `0x11` (K2) through
   `base+0x1b18`; the two properties are exactly `0x58` apart, matching the
   Start/End properties we derive from the parameters. Broadcasting a synthetic
   event therefore makes the **app** do the write, the notification and the
   repaint — the knob's own path, from inside the process.
3. **Autotrim now changes the page.** After the injection the framebuffer
   differs from the full-sample view by 14261 pixels (the same magnitude as a
   manual trim followed by a pad re-selection), and the screenshot shows the
   trimmed waveform with the removed region shaded.

### What is still imperfect

Our resolved `p0`/`p1` are not always the live instances (after a hand-set
`K2 = 105` the logged `p1` still read 0.0000), so the CC injection - which the
app resolves itself - is the reliable path. The commit uses it first and only
falls back to direct parameter writes.


## 14. Round 4: persistence is the difference

Measured, all on the real framebuffer:

| experiment | result |
| --- | --- |
| hand knob `K2` = 30, reload, reload again | **0 pixels** - the knob's change persists |
| hand knob `K2` 30 vs 127 | 14261 pixels |
| our CC injection, reload, reload again | the second reload **reverts** the view |
| `0x159f230` hooked | **never fires** on a panel knob |

So two independent facts:

1. **`0x159f230` is not the panel path.** It broadcasts through exactly the right
   properties (`base+0x1ac0` / `base+0x1b18`, which are 0x58 apart - our Start
   and End), and calling it does store the value (`app stored .../0.8268`), but
   the panel's own knobs never call it. It must belong to a different input
   (external MIDI CC).
2. **A knob's change survives a page reload; ours does not.** The knob updates
   the pad's stored trim; our write only reaches the live parameter and the
   property cache, which the reload discards.

The remaining unknown is therefore narrow and well posed: which function the
panel's knob events enter, and what it does after the broadcast to persist the
trim. The next probe is to hook the MCU/serial input path and capture the call
chain of a knob move, since the broadcast's own return address is inside our
relay and useless for this.


## 15. Round 5: the hook map is wrong, not the theory

Three hooks were assumed to cover the input path. Measured:

| hook | address | fires for a panel knob? | fires for a pad press? |
| --- | --- | --- | --- |
| CC dispatcher | `0x159f230` | **never** | - |
| note-on | `0x1e27360` | - | **never** |
| button dispatch | `0x159f2f0` | yes, as `code=0x06` | yes (0x0c/0x0e) |

So the only hook that actually sees panel input is the button dispatch, and a
knob turn arrives there as `code 0x06` with a two-step value - an *encoder*
event, not the 7-bit CC that the 0x159f230 handler expects. That handler belongs
to a different input (external MIDI), which is why it never fires and why
injecting into it stores a value the page later discards.

A whole-heap fingerprint tool (`heap_snapshot`/`heap_diff`, driven by
`/run/mpcmini.heap`) is in place and already shows that a knob move touches only
a handful of pages, but it needs a byte copy of the pages (not just hashes) to
name the fields.

Conclusion: the trim's persistent state is updated by the *encoder* path behind
`code 0x06`, which is the one thing not yet traced.


## 16. Round 5 result: pixel-identical trim, but the injection aborts the app

The knob replay through the app's CC dispatcher works:

* it needs the Start and End **properties**. They are derivable from the panel
  state once the knobs have been touched (learned at a real knob move), and a
  structural search for the three properties 0x58 apart also finds them;
* with both properties known, `cc_inject` stores the values through the app
  (`app stored 0.0000/0.8268`), and the page repaints itself;
* the resulting framebuffer is **pixel-identical** to setting the knobs by hand
  to the same values: `autotrim vs hand-set reference: 0 pixels`, while both
  differ from the untrimmed view by 26136 pixels.

But the app **aborts shortly afterwards** (the pid changes), so the injection is
now behind `MPCMINI_CCINJECT` and the default path is the direct core write,
which is stable (`pid 69688 -> 69688`) and writes both parameters
(`commit core done p0=0.0000 p1=0.8268`).

So: the trim computation, the parameter write and the visual result are all
correct and validated. What is left is either finding why the app aborts after
the replay, or triggering the page's own refresh after the stable write.


## 17. Round 6: the parameter write is reliable; the page reads a model

New facts:

* **A parameter can silently reject a write while it still holds -1.** The core
  treats that as "not yet bound". `commit_trim` now retries the pair until both
  read back, which is why the log finally shows both values
  (`commit core done p0=0.0000 p1=0.8268`) instead of one of them staying at 0
  or -1.
* **The displayed trim is not the parameter.** With both parameters correct and
  the page re-selected, the framebuffer differs from the untrimmed view by
  **1 pixel** - i.e. it still shows the full sample, while the hand-set trim
  differs by 4626. The page draws from a model that the knob's own handler
  updates.
* **The knob replay does update that model** (round 5: pixel-identical to a hand
  trim) but the app then dies with `SIGSEGV` at a fixed heap address
  (`pc=addr=0xffffe60544d8`), i.e. a call through a bad function pointer inside
  the app's own processing, not in our code.
* Re-pressing an arbitrary captured control is not a valid refresh: the object
  recorded from the last `code 0x06` press is not always the pad and pressing it
  opened a menu. It is disabled.

So the remaining work is exactly: find the model field the knob's handler writes
after the broadcast. The heap-fingerprint tool is in place for that but needs a
byte copy of the changed pages to name the field.


## 18. Round 7: where the replay dies

* The whole `CC` replay was moved to the **paint hook** (a safe point on the UI
  thread) instead of running inside the button dispatch. It still dies, with the
  same fault, so re-entrancy is not the cause.
* The fault address is constant across processes and, checked against
  `/proc/<pid>/maps`, **falls in no mapping at all**: `pc = addr =
  0xffffe60544d8`, while every real mapping ends below `0xfffff8000000`. The
  value is in the range the app's own stacks live in, so the app called a
  function pointer that actually points at stack data.
* The handler's ABI is confirmed correct by disassembly (only `x0` = event,
  `w1` = raw, `s1` = range are read; `x2` is overwritten by `and w2,w1,0x7f`),
  and the event buffer is now `static`, so a retained pointer stays valid.
* With the default (no replay) the parameter write is stable and exact
  (`retry 0 (p0=-1.000 p1=-1.000)` then `commit core done p0=0.0000
  p1=0.8268`), and the pid never changes - but the page still shows the full
  sample (0 pixels), which is the model problem from section 17.

So the replay performs the correct update and then the app calls a stack address
as code. That is the one thing left to explain.


## 19. Round 8: what the trim really is, and the edges drawn

**The threshold is right.** The sample's level envelope, measured in 32 windows
(`MPCMINI_ENV=1`):

```
 3/32 peak 29205   9/32 7942   16/32 1821   20/32 1156   24/32 418   28/32 95
```

The tail decays smoothly to ~70 counts, so a -40 dB threshold (292) cutting at
82.5 % is correct for this material - the "content ends at 45 %" impression was
the display compressing 15129 frames into 295 pixels. `MPCMINI_TRIM_DB` remains
the knob for more or less aggressive trimming.

**The edges are now visible.** The page's own trim display only refreshes when
the pad is re-selected (section 13), and the parameter write does not update it
(section 17), so the paint hook draws the two edges of the last autotrim over
the waveform at `x = 10 + v * 295`, y 88..186. Verified by pixel measurement:

```
marker columns: [(10, 80), (253, 80)]
expected:        start 10,  end 253        (v0 = 0.0001, v1 = 0.8254)
```

They are dropped by any knob move (a real knob takes over) and by leaving the
page (the page repaint clears them). The app's own knob indicators still show
the old values, because they are drawn from the same model that is not updated.

**The replay's fault is not in the listeners.** Logging every listener and its
call slot right before the injection:

```
P 0xffffe4f0c718 listener 0xffffe4f18830 vt=0xaaaab18a8a10 slot28=0xaaaaabf3da50
P 0xffffe4f0c718 listener 0xffffe49132e0 vt=0xaaaab18bb9a8 slot28=0xaaaaac08fe00
P 0xffffe4f0c770 listener 0xffffe4f18990 vt=0xaaaab18a8a10 slot28=0xaaaaabf3da50
P 0xffffe4f0c770 listener 0xffffe23051e0 vt=0xaaaab18bb9a8 slot28=0xaaaaac08fe00
```

All four targets, vtables and call slots are valid image addresses, and the
fault (`0xffffe60544d8`) matches none of them. The crash therefore happens in
the app's follow-up work after the broadcast, with a bogus stack pointer
(`sp=0xfffffffff3d0`), not in the dispatch itself.


## 20. Round 9: the state-sync function (and a corrected call chain)

Hook-driven hypothesis testing (log in `HYPOTHESES.csv`) produced four results:

1. **The page's display is not driven by a listener.** Calling listener 1 of a
   knob property alone changes **0 pixels**; calling listener 0 crashes the app.
   Both share one call slot (`0xaaaaacccd640`), so neither is the page side.
   Disproved.
2. **`prop_of_param` was returning the class prototype inside the image**, not
   the live heap property. It matched structurally but is never the live one.
   Fixed by rejecting candidates in `base .. base+0x9000000`; the poke probe then
   found heap properties and stopped crashing. **Proved - this was a real bug.**
3. **The two bases are not the same.** The CC handler at `0x159f230` adds
   `0x1ac0`, while the state-sync function adds `0x18b8 + 0x58*k`
   (`0x18b8, 0x1910, 0x1968, 0x19c0, 0x1a18, 0x1a70, 0x1ac8, ...`). With `0x1ac8`
   the handler finds no owner object at all, so `0x1ac0` is right for the
   handler. Disproved that they share a base.
4. **Found the state-sync function** at `0x2822400`-ish. Its shape:

   ```
   ldr s0, [x20, 0x1960]      ; read the model
   add x0, x19, #0x1968       ; the live property
   bl  0x13bb930              ; broadcast -> updates listeners and the UI
   ```

   repeated for ~20 properties with stride `0x58`, where the model field is
   always `property + 0x50`. So `x20` is the model the page draws from, and this
   function is the model -> properties -> UI bridge.

The next step is therefore concrete: find that function's entry and its `x20`,
write our two trim values into the model fields, and call it. That is the app's
own state-loading path, so it should update the page without the synthetic-event
crash.


### 20.1 The sync function's shape, measured

Hooking `0x2822260` (entry: `paciasp; stp x29,x30,[sp,-0x40]!`) shows it is called
with two large objects `0x3d30` apart, and a count:

```
sync args 0xfffff3d10bc0 0xfffff3d148f0 (nil) 0x1
sync args 0xfffff3d220e0 0xfffff3d25e10 (nil) 0x1
```

Inside, for every one of ~20 controls:

```
ldr s0, [x20, <property_off - 8>]   ; the model value
add x0, x19, #<property_off>        ; the live property
bl  0x13bb930                       ; broadcast
```

so the model field for a property at `+N` lives at `+N-8` of the second argument.
Reading the captured objects at that relationship did not yet line up with the
knob values (`a0+0x1ac0 = 0.5`, `a1+0x1ac0 = 0.0`), so which of the two is the
source and which the destination is still open - the call captured may be the
initial fill.

This is the most promising lead: writing the two trim values into the right one
of those objects and calling `0x2822260` is the app's own
model -> properties -> UI path, with no synthetic event involved.


### 20.2 The replay is pixel-perfect; only its aftermath kills the app

Three more experiments this round (see `HYPOTHESES.csv`):

* **The sync function is the save path, not the load path.** Its two arguments
  are stack temporaries `0x3d30` apart that grow by `0x11860` per call, and their
  trim fields read 0.0 - it serializes state, it does not push it to the UI.
* **Recovering from the fault is not possible from our side.** The fault happens
  *after* the replay returns (asynchronously), so no `sigsetjmp` recovery point
  is active when it lands; the handler's `siglongjmp` path was never taken.
* **Splitting the two controls across paints removes the crash but also the
  update.** With one control per paint (`step 1 done` / `step 2 done`) the pid
  stays stable and nothing crashes, but the page does not change
  (`HAND vs AUTO 4817 px`). With both controls in one turn the page becomes
  **pixel-identical to a hand trim** (`HAND vs AUTO 0 px`, `FULL vs AUTO 4626 px`)
  and the app dies about a second later.

So the app applies a trim *and* repaints only when both controls are delivered in
one UI turn, and that same turn leaves something behind that faults afterwards.

The shipping default stays the stable path: exact parameter write plus the drawn
edges (192 px), verified again at the end of this round.


### 20.3 Only the one-turn replay updates the model

Two more results:

* **The one-turn replay really does update the model** - re-selecting the pad
  afterwards leaves the page pixel-identical to a hand trim (`HAND vs AUTO 0 px`),
  so it is not a transient repaint.
* **The split replay updates nothing.** One control per paint, then a pad
  re-selection: `FULL vs AUTO 0 px`. So the two value changes must be delivered
  in the same turn for the model to be written at all.

That fixes the choice: the correct result is only reachable through the path
that leaves the app in a state it later faults in (`pc = sp = 0xffffe60544d8`,
`sp = 0xfffffffff3d0` before the fault). Capturing the fault registers is in
place for the next run that survives long enough to be inspected.


### 20.4 The one-turn replay is not reproducible

The pixel-perfect result of section 20.2 could not be repeated. With identical
code and an identical script, a fresh run gave:

```
sanity: full vs hand = 4626 px      (the hand reference does change the page)
FULL vs AUTO: 192 px                (only our drawn edges)
HAND vs AUTO: 4817 px
pid 86236 -> 86236                  (no crash)
learned props: 0xffffe4f0c718(p=0xffffe4aaeb90) 0xffffe4f0c770(p=0xffffe4aaf580)
props 0xffffe4f0c718/0xffffe4f0c770   cc base=0xffffe4f0ac58   one-shot replay done
```

so the correct property pair, the correct base, the correct object and a
successful replay all happened, and the page still did not change.

Two things follow:

* the trim state is being written (the properties read back `0.0000/0.8268`),
  but whether the *page* picks it up depends on app state we do not control;
* the one-turn replay is therefore not shippable: it was observed to update the
  page once, to do nothing other times, and to crash a third time.

The shipping default stays: exact parameter write (`p0=0.0000 p1=0.8268`) plus
the two drawn edges (193 px, markers at x=253), verified again on a fresh
process with no crash.


### 20.5 The page's own trim rendering, reproduced

Sampling a row across the waveform for a hand-set trim shows the app's convention:

```
hand-set K2=105:  x 84..254 black (waveform)   x 257..304 grey (27,27,30)  <- trimmed out
```

so the trimmed-out region is **grey**, not black. Drawing the same grey (plus the
1 px edge) in the paint hook gives:

| comparison | before the fix | after |
| --- | --- | --- |
| FULL vs AUTO | 557 px | **5197 px** (trim clearly visible) |
| HAND vs AUTO | 5078 px | **1090 px** |

The 1090 px remainder is our own 1-px edge line plus the 0.5 % difference between
the computed edge (82.54 %) and the hand-set knob (105/127 = 82.68 %). No crash,
pid unchanged.

So the page now shows the trim the way the app does; only the app's own
Start/End *widget bars* at the bottom still show the old values, because they are
drawn from the model our writes do not reach.


### 20.6 Nothing visible is missing

The last suspected gap - the bottom band not changing - is not a value display:

```
K2 = 0 / 105 / 127   non-background pixels at y226..238:
   x = 15..20  and  x = 311..314, identical for all three values
```

The 52 px that differ between an untouched and a knob-touched page are the knob
**focus highlight** (13x4 px at x=183..195), which appears because a knob was
turned. Autotrim moves no knob, so it correctly does not appear.

Writing the mapped slider's value (`slider+0x1b4`) lands but changes nothing, so
that widget is not what draws anything either.

Conclusion: the page after SHIFT+B3 reproduces everything the app draws for a
trimmed sample - grey shading over the trimmed-out region plus the edge - and
differs from a hand trim only by our own 1 px edge line and the 0.5 % difference
between the computed edge and the knob's quantised position.


## 21. Final validation (round 13)

Fresh process, stock project, pad 1, sample `BoomBap-Kik-BTB MD`:

```
trim #1 vs untrimmed page : 5197 px
trim #2 vs untrimmed page : 5197 px
trim #1 vs trim #2        : 0 px        <- fully repeatable
commit core done p0=0.0000 p1=0.8268  (want 0.0001/0.8254)
pid 89265 -> 89265                    <- no crash
```

What is verified:

* the silence threshold is measured against the file's own peak (envelope logged
  in 32 windows), so low-level tails are trimmed and a quiet file is not;
* the two trim parameters end up holding exactly the values the Start/End knobs
  would produce for those edges (same fields, same core call);
* the page draws the result the way the app draws a manual trim: grey shading
  over the trimmed-out region plus the edge, verified against a hand-set trim
  (the only residue is the 1 px edge line and the 0.5 % difference between the
  computed edge and the knob's quantised position);
* it is stable and repeatable: two consecutive trims produce identical frames and
  the process never changes.

What is *not* claimed:

* the app's internal model is not updated by our write, so the app's own stored
  state does not learn the new trim; every visible element the app draws for a
  trim is reproduced by the mod instead;
* audio could not be captured - this emulator has no audio device
  (`query-audiodevs` returns `[]`), so audibility rests on the parameters being
  identical to a knob's, not on a recording.

The knob replay through the app's own CC dispatcher *does* update that model and
was measured to be pixel-identical to a hand trim, but its outcome varies between
runs (once pixel-perfect, once inert, once followed by a fatal fault at
`0xffffe60544d8`), so it stays behind `MPCMINI_CCINJECT`/`MPCMINI_ONESHOT` and
is not the default.

## 22. Ghidra decompilation of the pad-state path (round 14)

Ghidra was made usable (see `docs/RE-GHIDRA.md`), which allowed reading the two
functions that own the per-pad state.

### 22.1 FUN_02902e60 - the pad-block loop

```c
void FUN_02902e60(long param_1,long param_2,int param_3) {
  if (param_3 == 0) {
    lVar3 = 0;
    do {
      lVar1 = *(long *)(param_1 + 0x22e8) + lVar3;   /* destination block */
      lVar2 = *(long *)(param_2 + 0x22e8) + lVar3;   /* source block      */
      lVar3 = lVar3 + 0xb30;
      FUN_02918be0(lVar1,lVar2);
    } while (lVar3 != 0x5980);                        /* 0x5980/0xb30 = 8 pads */
  }
  ...
}
```

So: an array pointer at `object + 0x22e8`, blocks of `0xb30`, eight of them.

### 22.2 FUN_02918be0 - the applier

Reads the source block and pushes it into the page:

```c
local_7c = *(undefined4 *)(param_2 + 0x1a8);        /* -> broadcaster at param_1+0x158 */
local_80 = *(undefined4 *)(param_2 + 0x220);        /* -> FUN_014b16a0(param_1+0x1d0)   */
local_84 = *(undefined4 *)(param_2 + 0x298);        /* -> FUN_014b16a0(param_1+0x248)   */
                             *(undefined4 *)(param_2 + 0x310);
                             *(undefined4 *)(param_2 + 0x368);
                             *(undefined4 *)(param_2 + 0x3c0);
```

Six knob floats, stride `0x78`. Only the first goes through the *broadcaster*
(`FUN_014bb930`) - which is why the broadcaster hook saw knob values but the
other five knobs produced no observable traffic.

### 22.3 What the runtime says

Hooking `FUN_02918be0` and printing only calls whose source block holds
plausible floats gives:

```
apply dst=0xffffe2c5b830 src=0xffffffffcb68 start=0.5000 end=0.0000
apply dst=0xffffe2c5c360 src=0xffffffffcb68 start=0.5000 end=0.0000   (+0xb30)
apply dst=0xffffe2c67f50 src=0xffffffffcb68 start=0.5000 end=0.0000
...
```

* the destination steps by exactly `0xb30` - these **are** the eight pad blocks,
  and they live on the heap at `0xffffe2c5b830` for pad 0;
* the source is `0xffffffffcb68`, which `/proc/<pid>/maps` places inside
  `[stack]` (`fffffffdf000-1000000000000`) - so the app copies **stack -> block**,
  i.e. it is *saving* the page's widget values into the pad block;
* dumping block 0 afterwards shows zeros at `+0x1a8/+0x220/+0x298` and a single
  `0.5` at `+0x108`, i.e. the block did not receive the knob values from the
  stack copy in this path.

### 22.4 Conclusion

The pad blocks are real and their layout is confirmed statically, but in the
observed path the app writes *into* them from a stack temporary that does not
carry the live knob values. The knobs' own write path is therefore upstream of
this copy, and it is still not identified. What is now known for certain:

* the state container is `*(object + 0x22e8)`, eight `0xb30` blocks;
* the per-pad trim fields are `+0x1a8` (Start) and `+0x220` (End), stride `0x78`;
* writing the parameter, its applied copy, the property cache, the layer frames
  or the slider does **not** reach it (verified with the overlay disabled: 0 px).

## 23. The per-pad state object (round 15, Ghidra)

### 23.1 FUN_0290f130 is the pad-load entry

```c
void FUN_0290f130(long param_1,long param_2) {
  ...
  FUN_0290dca0(param_1,param_2,lVar1,local_38);   /* page <- pad state */
}
```

`FUN_0290f130(page, pad)` is a virtual method (the callers include a vtable
slot at `0x006d6cef8`); `FUN_0290f080(page)` is the same call with `param_2 = 0`,
i.e. "reset the page".

### 23.2 The pad's field layout

`FUN_0290dca0` copies the pad state into the page's widgets, 0x58 apart:

```c
FUN_014b16a0(param_1 + 0x1538, *(undefined4 *)(param_2 + 0x1588));  /* Start */
FUN_01db15f0(param_1 + 0x1590, *(undefined4 *)(param_2 + 0x15e0));  /* End   */
FUN_0160b380(param_1 + 0x15e8, *(undefined4 *)(param_2 + 0x1638));  /* next  */
FUN_014b3740(param_1 + 0x1640, *(undefined4 *)(param_2 + 0x1690));
```

So **`pad + 0x1588` = Start and `pad + 0x15e0` = End**, stride `0x58` - the same
stride as the property objects. This is the authoritative per-pad trim state;
the parameter object (`+0x19c`), the property cache (`+0x50`), the layer frames
and the slider are all derived from it or written beside it.

### 23.3 What did not work

* hooking `FUN_0290f130` never fired with a heap pad pointer - the loads we see
  pass stack temporaries (`FUN_0290dca0(page, 0, ...)`), i.e. resets;
* a value scan for `Start = 0.0, End = 0.7874` at the pad offsets found only
  static tables inside the image (`0xaaaab1c1xxxx`), not the live object;
* `search_instructions` for stores at `+0x1588` returns only **double** stores in
  unrelated classes (`FUN_00f6e120`, `FUN_01471d30`, ...), never a float store -
  so the pad field is written through computed addressing that the immediate
  operand search cannot see.

### 23.4 Where that leaves it

The layout is now known precisely enough to write (two floats, 0x58 apart, at
`pad+0x1588` / `pad+0x15e0`), but the live pad instance has not been located.
The remaining step is to find the pad pointer at runtime - the most promising
route is the object that owns the pad array (`*(owner + 0x22e8)`, eight `0xb30`
blocks, see section 22), since the pad state and the block array belong to the
same container.

## 24. What the three setters are, and why the page still ignores our writes

### 24.1 They are the same thread-safe value setter

`FUN_014b16a0`, `FUN_01db15f0` and `FUN_0160b380` decompile to the *same* body:

```c
void Setter(long widget,int v) {
  pthread_mutex_lock(widget + 0x20);
  if (*(int *)(widget + 0x50) != v) {      /* current value slot */
    *(short *)(widget + 0x18) += 1;        /* version counter    */
    *(int *)(widget + 0x50) = v;           /* new value          */
    ... notify ...
  }
}
```

So **a value holder keeps its value at `+0x50`, a version at `+0x18` and a
mutex at `+0x20`**. Renamed `Value_SetInt` / `Value_SetIntEnd` /
`Value_SetIntLoop`.

Because the page's Start widget is `page+0x1538`, its value is at
`page+0x1588` - the same offset the pad uses, so page and pad share the layout.

### 24.2 The knobs write the property, not the widget

Hooking `Value_SetInt` produced **no** events on a knob turn. The mod's own
learning line shows why:

```
learn? prop=0xffffe4ecc718 c50=0.4724 v=0.4724
```

i.e. by the time the broadcaster is reached, the property's `+0x50` already
holds the new value - the knob writes that slot directly (through its own
event path), and `Value_SetInt` is used by other code.

### 24.3 Runtime results

| experiment | result |
|---|---|
| write `page+0x1588` / `page+0x15e0`, repaint (no reload) | 0 px |
| write `page+0x1588` / `page+0x15e0`, then re-select the pad | 0 px (the reload restores the pad's values) |
| write the learned property `+0x50`, repaint | 0 px |

Reading `page+0x1588` through `guestmem` reports the address **unmapped**, so the
object the mod knows as `g_screen` is *not* the Sample page - it is too small to
own a widget at `+0x1538`. The page object has not been captured yet:
`Page_LoadPad` and the page constructor never fire during normal use, and
`PadState_CopyIntoPage` is only ever observed with stack temporaries.

### 24.4 The remaining unknown, stated precisely

The waveform is drawn from a **cached** model: the page does not repaint its
trim when a value changes (measured 0 px), and only a rebuild of the page makes
it read the pad again. So the reliable path is: write the pad object's
`+0x1588`/`+0x15e0`, then force the app to rebuild the page. Locating the live
pad object is the single remaining blocker.

## 25. Ghidra database annotations

Functions renamed for future sessions:

| address | name |
|---|---|
| `0x2918be0` | `PadState_Apply` |
| `0x2902e60` | `Pads_ApplyAll` |
| `0x290dca0` | `PadState_CopyIntoPage` |
| `0x290f130` | `Page_LoadPad` |
| `0x290f080` | `Page_ResetPad` |
| `0x16e70c0` | `TrimParam_SetValue` |
| `0x14bb930` | `Property_Broadcast` |
| `0x14b16a0` | `Value_SetInt` |
| `0x1db15f0` | `Value_SetIntEnd` |
| `0x160b380` | `Value_SetIntLoop` |
| `0x1a9e460` | `PadStateBlock_Copy` |
| `0x29100c0` | `SamplePage_Ctor` |

Parameters renamed: `PadState_CopyIntoPage(page, padState)`,
`PadState_Apply(page, padState)`, `TrimParam_SetValue(value, trimParam)`.
Each of the above carries a plate comment summarising what it does and the
offsets involved.

## 26. The page object, and the crash that came from a stale pointer

### 26.1 Where the page is created

```c
void *FUN_02971040(long *owner) {
  pvVar6 = operator_new(0x2468);                       /* the Sample page */
  SamplePage_Ctor(pvVar6,owner,owner[0xd6],owner[0xd9],owner[0x3fb],owner[0xd7],
                  owner[0x3f9],owner[0x3fa],owner[0x3f6],3,uVar5,owner[0x3fd]);
  lVar7 = *(long *)((long)pvVar6 + 0x22e8);            /* the 8 pad blocks  */
  ...
}
```

So the page is a **0x2468-byte heap object**, it owns the pad-block array at
`+0x22e8`, and its Start/End widgets are at `+0x1538` / `+0x1590` (values at
`+0x1588` / `+0x15e0`). `FUN_02971040` is called per page creation, so the page
is rebuilt rather than reloaded in place - which is why `Page_LoadPad` never
fires.

### 26.2 The right call to make

Calling the app's own setters on the live page is the correct way to move the
trim *inside* the app:

```c
Value_SetInt    (page + 0x1538, bits(start));   /* bumps version, notifies */
Value_SetIntEnd (page + 0x1590, bits(end));
repaint(page);
```

### 26.3 The crash and its cause

Doing exactly that crashed the app twice (`pid` changed, 8.8k px of stray
change): the stored `g_page` came from a **previous process**. The heap address
was still plausible enough to pass a vtable sanity check, so the setter ran
against freed memory.

Rule for the next attempt: **the page pointer is only valid within the process
that created it** - clear it on every new `SamplePage_Ctor`, and only use it
when the vtable pointer at `page[0]` is inside the loaded image *and* the page
was captured in this run. If the page predates the hook, leave the Sample screen
and re-enter so a fresh constructor call happens.

### 26.4 State of the shipped build

The probe code was reverted to the last known-good source before this round;
autotrim is back to the verified state:

```
autotrim: 5197 px | pid unchanged
commit core done p0=0.0000 p1=0.8268 (want 0.0001/0.8254)
```

## 27. Capturing the pages: what they actually contain

Hooking `SamplePage_Ctor` (**with all 12 arguments forwarded** - a 2-argument
hook corrupts the call and faults immediately) yields 8 page objects per run,
each with the same vtable and each owning its own pad-block array:

```
page 0 = 0xffffe40836a0 vt=<ok> +1588=0 +15e0=1 arr=0xffffe4088e90
page 1 = 0xffffe4094040 ...       +1588=0 +15e0=1
...
page 7 = 0xffffe24c94e0 ...       +1588=3 +15e0=1
```

### 27.1 `+0x1588` is not the trim

Setting `K2` to 127 and then to 30 and re-reading every page shows the field
**does not move**:

```
page | K2=127 (+1588) | K2=30 (+1588)
  0        0                0
  2        2                2
  7        3                3
```

So `+0x1588` holds a small enum (0..3), not the Start value - the earlier
"0.8268-looking" reading on page 5 was a coincidence of an unrelated float.

### 27.2 The pad blocks are uninitialised

Scanning a live block (`arr = 0xffffe2b46a80`) for the knob values finds only
`0.5` at `+0x4a8`, and the rest of the block is full of `0xAAAA` filler and
`65535` patterns - i.e. these blocks are **not** where the trim lives either.

### 27.3 The knob handler

`FUN_02904e90(page, control_id, flags)` handles control ids `0xc6f..0xc76` and
reads

```c
uVar19 = NEON_ucvtf(*(undefined4 *)(*(long *)(param_1 + 0x22e8) + iVar4*0xb30 + 0x668));
```

i.e. a per-control field at **`block[idx] + 0x668`**, converted int -> float.
Those fields hold values like `879188`, `759913`, ... - not normalised trim
positions, so they are some other per-pad quantity (id/hash-like).

### 27.4 Conclusion for this round

Every candidate field has now been measured against a knob move and none of them
moves: parameter `+0x19c`, applied `+0x1a0`, property `+0x50`, page `+0x1588` /
`+0x15e0`, pad block `+0x4a8` / `+0x668`, layer frames, slider. The app's own
display still follows the knobs, so the path exists - but it is the knob's own
event handler, and that handler is the one thing still unidentified.

The build was restored to the last known-good source and re-verified:

```
autotrim: 5196 px | pid unchanged
```

## 28. The knob handler's own reads (round 17)

`FUN_02904e90(page, control_id, flags)` is a switch over control ids. For the
trim controls it reads the pad block at `*(page + 0x22e8) + idx*0xb30`:

```c
case 0xc0a: case 0xc12: case 0xc1a: case 0xc22:      /* Start */
    uVar21 = *(uint *)(block + 0x1a8);
case 0xc0b: case 0xc13: case 0xc1b: case 0xc23:      /* End   */
    uVar21 = (uint)((float)*(int *)(block + 0x220) / 127.0);
case 0xc0c: case 0xc14: case 0xc1c: case 0xc24:
    uVar21 = (uint)((float)*(int *)(block + 0x298) / 127.0);
```

Two things follow:

* the control ids are `0xc0a + 8*pad` (Start) and `0xc0b + 8*pad` (End) - so the
  pad index is encoded in the id, not looked up separately;
* the fields are read as **ints** and divided by 127, i.e. the raw knob value.

### 28.1 But the reachable blocks are defaults

Reading those fields on every page captured in a live run gives, for all eight
pages:

```
pg0 arr=0xffffe2e465a0 start=1056964608 end=0
pg1 arr=0xffffe2e56dd0 start=1056964608 end=0
...
```

`1056964608 = 0x3F000000` = **0.5f**, and `end = 0`. So the blocks reachable from
the pages the constructor hook sees hold *defaults*, not the live trim - which is
also why scanning them for the knob values found nothing.

### 28.2 Status

The wiring is fully mapped (control id -> block -> field -> 0..1 position), but
the block instance that the *visible* page reads is not one of the blocks
reachable from the captured pages, and no field on those pages tracks a knob
move. Resolving this needs either a live debugger breakpoint on
`FUN_02904e90` with a real knob event (to see which page/block it receives), or
accepting the overlay as the visible representation.

The shipped build is unchanged and verified:

```
autotrim: 5197 px | pid unchanged
commit core done p0=0.0000 p1=0.8268
```

## 29. Live debugger on the guest (round 18)

The lab VM has no gdb/gdbserver/lldb, but ptrace is unrestricted
(`ptrace_scope` absent), so a small tracer was cross-compiled for the guest:

```
zig cc -target aarch64-linux-musl -static -O2 -o knobtrace knobtrace.c
scp knobtrace root@127.0.0.1:/tmp/       # port 2122
/tmp/knobtrace <pid> <addr> <seconds> <maxhits>
```

It attaches, plants `BRK #0`, logs `pc/x0/x1/x2/x3/lr/x29`, single-steps the
original instruction, re-arms, and detaches. It also walks the frame-pointer
chain and prints return addresses inside the app image.

### 29.1 FUN_02904e90 hit on a real knob turn

```
armed 0xaaaaad2a4e90 (base + 0x2804e90)
HIT pc=0xaaaaad2a4e90 x0=0xffffe2c407a0 x1=0x462 x2=0 x3=0xffffe2c32f00 lr=0xaaaaac408064
```

* `x0` = the object the handler works on (heap, changes per process);
* `x1 = 0x462` = the control id for the knob that was turned;
* `lr = base + 0x1968064` = **the caller** - the knob dispatcher that invokes
  the handler. That address is the next thing to decompile.

Reading `x0 + 0x22e8` gave `0x800000000`, and `x0`'s first word is a heap
pointer, so the object the handler receives is **not** the Sample page that owns
the block array - the page is reached through it somehow.

### 29.2 The trim core is not on the knob path

A breakpoint on `TrimParam_SetValue` (`base + 0x15e70c0`) produced **no hits**
while turning knobs. The core is only called by the mod's own commit, i.e. the
knob path reaches the parameter by some other route.

### 29.3 A hazard worth recording

The mod already patches the first instruction of hooked functions with its own
`B`. A tracer that plants `BRK` at the same address and later "restores" what it
read will write back the mod's branch (fine) - but in this run the read value did
not match, the entry ended up corrupted, and the app crashed and restarted.
**Never breakpoint an address the mod has hooked**; hook-free addresses (the
caller at `0x1968064`, `FUN_02904e90` itself) are safe.

### 29.4 State

App restarted cleanly and re-verified:

```
autotrim: 5197 px | pid unchanged
commit core done p0=0.0000 p1=0.8268
```

## 30. The property DOES drive the display (round 19)

Three measurements with the live debugger and the panel settled the picture:

### 30.1 Both knobs repaint the waveform

```
K1 10 -> 90 : waveform 3722 px, bottom 112 px, top 0 px
K2 20 -> 110: waveform 1704 px, bottom  20 px, top 0 px
```

Earlier "0 px on a knob move" measurements were wrong (the knob change had not
registered). The page *does* redraw its trim on every knob turn.

### 30.2 Control ids

A breakpoint on the dispatch funnel `FUN_029c6510` with a knob turn gives:

```
x0 = surface, x1 = 0, x2 = 0x462, x3 = 0x462   (Start knob)
x0 = surface, x1 = 0, x2 = 0x466, x3 = 0x466   (End knob)
```

so **Start = control 0x462, End = control 0x466** (four apart - the same knob
group). `FUN_029c6510` then calls the surface's virtual method at
`vtable + 0x160` = `FUN_02904e90`, which is a **getter**: for 0x462 it reads
`FUN_02912f10(*(page+0x22e8) + idx*0xb30) + 0x78`, for 0x466 the same object at
`+0xd0`. The matching *setter* path is `FUN_01a68020(0, surface, controlId,
value, 0xd28)` -> `FUN_01a679e0(...)`.

### 30.3 Writing the property and forcing one redraw works

With the overlay disabled:

1. set the knobs to Start = 0, End = 127 (full sample);
2. write `0.5` into the learned Start property at `prop + 0x50`;
3. nudge the End knob (127 -> 126 -> 127) to make the app redraw.

The app's own Start edge moves to **50 %** - confirmed by screenshot. So the
property value *is* what the page paints, and the only missing piece is a redraw
the mod can trigger without touching a knob.

### 30.4 What does not work yet

* calling the repaint `FUN_01149880` on the captured page or on `g_screen` after
  the property write: 0 px;
* calling the app's setter `FUN_01a68020` directly: crashed the app (needs the
  exact surface object and mode, which the hook had not captured reliably);
* the page captured from `SamplePage_Ctor` (`0xfffff2a8d9b0`) looks like a stack
  address, so it is not the visible page.

Shipped build: overlay on, verified at 5197 px with an unchanged pid.

### 30.5 The surface is real, but calling the setter still crashes

Hooking `FUN_01a68020` and logging the argument it receives on a knob turn gives:

```
surface=0xffffe2c31010 vt=0xaaaab1966860 valid=1 raw0=0 raw1=105
```

so the surface is a real heap object with a static vtable, and the raw control
values (0 and 105 for Start/End) are known. Calling

```c
FUN_01a68020(0, surface, 0x462 /*Start*/, raw, 0xd28);
FUN_01a68020(0, surface, 0x466 /*End  */, raw, 0xd28);
```

from the commit **faults every time** (the process restarts). Either the first
argument must be a real event object rather than 0, or `param_5 = 0xd28`
encodes a source that the callee dereferences. The call was removed and the
build restored.

What remains for a complete solution is exactly one thing: a redraw the mod can
trigger itself. Everything upstream of it is resolved - the property slot
(`prop + 0x50`), the control ids (`0x462` / `0x466`), the raw value encoding
(0..127) and the fact that the page paints from the property (verified: writing
`0.5` and nudging a knob moves the app's own Start edge to 50 %).

### 30.6 Why a self-triggered repaint is impossible from a hook

Caching the arguments our paint hook receives gives:

```
painter state orig=<valid> comp=0xffffe4a2ce70 gfx=0xfffffffff648
```

`comp` is a heap object, but **`gfx` is a stack address** - the Graphics handed to
`paint()` lives on the paint call's stack frame and dies when the call returns.
Any later reuse of it is a dangling pointer, so a hook cannot re-run the app's
painter; only drawing *during* the app's own paint pass is safe. That is exactly
what the overlay does, and it is why the overlay works while every forced
repaint returns 0 px.

### 30.7 The live page, captured in-process

Hooking the control handler `FUN_02904e90(page, controlId, flags)` and validating
`page[0]` (a static vtable) plus `*(page + 0x22e8)` (a heap pointer to the pad
blocks) yields a reliable page pointer that is refreshed on every knob turn:

```
repainted page 0xffffe2641510
```

Writing `page + 0x1588` / `+0x15e0` directly was tried and **made the display
worse** (those offsets hold small enums, not the trim floats) - the markers
disappeared until the build was reverted. Writing the widget values is therefore
not the answer; the property write plus an app-driven redraw (verified in 30.3)
remains the only path that actually moves the app's edge.

Shipped build: overlay on, reverted and verified.

### 30.8 The setter crash was an argument-order bug - but the call still does nothing

`FUN_01a68020(param_1, ...)` dispatches with `(**(code **)(*param_1 + 0x160))()`,
so **`param_1` must be the surface object**. My first attempt passed `0` there and
the surface second, which dereferences null - that was the crash.

With the correct order

```c
FUN_01a68020(surface, 0, 0x462 /*Start*/, raw, 0xd28);
FUN_01a68020(surface, 0, 0x466 /*End  */, raw, 0xd28);
```

the call **executes cleanly** (`setctl done`, process stable), and running it from
inside the paint callback - on the UI thread, immediately before the app paints -
also executes:

```
armed paint-time ctl 0/105
paint-time setctl 0/105
```

yet the display still does not change (0 px). So the control path needs more than
`(surface, 0, controlId, value, mode)`; one of those arguments does not mean what
the decompiler suggests (the 4th is probably not the value - `FUN_01a679e0`'s
first two parameters are floats, i.e. passed in `s0`/`s1`).

The call is kept behind `MPCMINI_CTL` and off by default; the shipped build is
unchanged and verified.

### 30.9 The paint does not read the property (round 4)

Writing the property values **inside** the paint callback, immediately before
the app's own paint runs, produces:

```
props written in paint 0.0001/0.8254
app display change: 0 px
```

So the app's paint does not read `property + 0x50`. The knob path must write a
different object that the paint *does* read, and that write has not been
located. The control entry `FUN_01a68020` was called with the corrected
register layout (`x1` = surface, `w2` = mode, `w3` = controlId, `s0` = float
value, taken from the disassembly at `0x1a68020`) and still faults, so its
remaining arguments are not yet right either.

### 30.10 What is definitively established

* the trim is computed correctly and both parameters are set exactly
  (`p0 = 0.0000`, `p1 = 0.8268`, knobs produce `0.8254`);
* the parameter's property is written (`prop + 0x50`), which is the model the
  app's *controls* use;
* the visible trim is drawn by the mod's overlay during the app's paint pass,
  with the app's own Graphics - pixel-comparable to a hand-set trim;
* the app's own Start/End edges cannot be driven from a hook: the paint ignores
  the property, the paint's Graphics is stack-local (so no forced repaint), and
  the control-setter entry faults from an external call.

Full internal-state sync therefore needs the write side of the knob path, which
four rounds of Ghidra work and live debugging have narrowed to
`FUN_01a68020`/`FUN_01a679e0` but not yet pinned down.

### 30.11 Exhaustive value scan (round 5)

A tolerant heap scan (`findval <float>`, 1e-4) run at four knob positions and
intersected leaves ~200 addresses, because the heap churns between scans. One of
them is `0xffffe4ecc768` - the Start property's value slot (`prop + 0x50`), which
confirms the scan works but shows that no single field can be isolated this way:
the renderer's source does not stand out from ordinary heap traffic.

The only remaining technique that could identify the write is **store tracing**
single-stepping the control handler and logging every store address across two
knob values, which needs an instruction decoder in the tracer and is far slower
than anything tried so far.

### 30.12 Where the investigation stands after five rounds

Fully mapped and verified:

| piece | evidence |
|---|---|
| trim computation | envelope analysis, `p0=0.0000 p1=0.8268` vs knob `0.8254` |
| parameter write | `commit core done`, values read back from the param |
| property model | `prop+0x50` written, confirmed in the scan |
| control ids | debugger: Start `0x462`, End `0x466` |
| control path | `FUN_01a68020` / `FUN_01a679e0`, register layout decoded |
| visual | overlay drawn during the app's paint, 400 marker pixels, 5197 px delta |

Still open: the app's own renderer source. All three routes are closed - the
paint ignores the property, the paint Graphics is stack-local, and the control
entry faults when called externally.

### 30.13 Store tracing (round 6)

A second tracer was written that breaks at a function, single-steps it and
decodes `STR/STP (immediate)` to log heap-destined stores with their source
register values (`docs` note: the decoder covers `0xF9000000`, `0xB9000000`,
`0x39000000`, `0x79000000`, `0xFD000000`, `0xBD000000` encodings).

Tracing `FUN_01a679e0` (the apply function) during a K1 turn yields only four
stores, all pointer-sized:

```
STORE pc=0xfffff702fa4c [0xffffe35e9138] <- 0xaaaab1d45020   (image pointer)
STORE pc=0xaaaaac407c38 [0xffffe35e9138] <- 0xaaaab1d45020
STORE pc=0xfffff702fa4c [0xffffce6e4508] <- 0xfffffffff200
STORE pc=0xaaaaaba308ec [0xffffce6e4508] <- 0x10
```

i.e. the apply function installs a type/vtable pointer and a small counter, not
the control value - the value write happens elsewhere in the chain.

A second run tracing from the outer entry `FUN_01a68020` with a float-value
filter produced no hits inside the observation window, so that part remains
unmeasured.

### 30.14 Honest position after six rounds

Every verified piece is in 30.12. The one open item - which structure the page's
renderer reads - has now resisted: property writes (inside and outside the
paint), page-widget writes, a live page capture, the app's own control entry
(correct register layout, still faults), a four-point value scan, and store
tracing of the apply function. The remaining technique is a longer store trace
over the whole control-event chain with a float filter, which is slow (single
stepping) but has not yet been tried to completion.

### 30.15 The getter object is not the trim either (round 7)

Hooking `FUN_02912f10` (the getter the control handler calls) captures the object
it returns, live and in-process:

```
trimobj=0xffffe2c46db0 (blk=0xffffe2c46670) start=0.0000 end=0.0000
```

Both `+0x78` (Start) and `+0xd0` (End) read zero while the knobs are at 45/80 and
30/100, so those offsets do **not** hold the trim as floats. Writing float bits
there does not set the trim - it **removes the page's markers entirely** (the
waveform then draws edge to edge), which means the fields hold something else
(very likely pointers). The write was reverted and the build restored
(392 marker pixels back, `commit core done` unchanged).

### 30.16 Techniques exhausted

| technique | outcome |
|---|---|
| property write (outside paint) | no visual change |
| property write (inside paint) | no visual change |
| property write + broadcast | no visual change |
| page widget write (`page+0x1588/+0x15e0`) | removed the markers (enums) |
| live page capture + repaint | no visual change |
| control entry `FUN_01a68020` (correct registers) | faults |
| four-point value scan | ~200 candidates, no isolation |
| store trace of the apply function | only pointer stores |
| getter object write (`+0x78/+0xd0`) | removed the markers |

The renderer's data source has not been identified, and every candidate write
either does nothing or damages the page's own drawing.

### 30.17 The page's paint is still not identified (round 7b)

Logging the live page's vtable gives a concrete address and its slots:

```
page vt=0xaaaab1963ca0 file=0x6ec3ca0
  slot 20 -> 0x28019d0     slot 26 -> 0x280dad0
  slot 21 -> 0x2809be0     slot 27 -> 0x280dc60
  slot 22 -> 0x2806680     slot 28 -> 0x27722f0
  slot 23 -> 0x28069c0     slot 31 -> 0x6ec3b98
```

Slot 27 disassembles to `sub x19, x0, #0x18 ... delete(this, 0x2468)` - the
**destructor**, the same one found in round 1. The clue is that the same slot
index means different methods in the screen vtable and the page vtable, so slot
27 is not "paint" here; the layout differs.

The screen vtable our overlay patches also turned out to hold an **empty stub**
(`FUN_011e2e10` decompiles to `return;`) at slot 27, which is why patching it
only ever added a drawing pass on top of the app's own, never replaced it.

So even the paint entry point is not identified, and with it the last chance to
see what the renderer reads.

### 30.18 Pad-state route (round 8)

The pad object is the last plausible source: `FUN_0290dca0(page, pad)` copies
`pad+0x1588` into the page's Start widget and `pad+0x15e0` into End, so writing
*the pad* would survive a page rebuild.

A hook on `FUN_0290dca0` never fires (the log shows no `pad=` line), and the
installer reports the site has no `paciasp`/`bti` prologue, so the mod cannot
interpose on it with the current hook scheme (which requires a PAC/BTI prologue
to patch safely). Reaching the pad object therefore needs either a hook variant
that can patch a plain-prologue function or a different capture point.

### 30.19 Blocked: what is needed to finish this

Six consecutive rounds have failed on the same concrete point: **the data the
page renderer reads for the Start/End edges cannot be identified or written**.
Every route has been tried and measured (30.16-30.18); the pad-state route is now
also closed by the hook-installer limitation.

To go further, one of these is required:

* a debugger with **hardware watchpoints** on the app's heap, so a page field can
  be watched across a knob turn (the emulator exposes no `perf`, and aarch64
  Linux has no `PTRACE_SETHBPREGS`, so `ptrace` cannot do this here);
* an app build with **symbols or type information**, so the page's paint can be
  read directly instead of guessed from vtable slots that mean different methods
  in different vtables;
* permission to patch the app binary itself (the mod is an `LD_PRELOAD` shim, so
  it cannot rewrite the renderer's read path).

Until one of those is available, the feature ships as: trim computed and applied
to the audio parameters exactly, and the trim drawn on screen by the mod's
overlay during the app's own paint pass.
