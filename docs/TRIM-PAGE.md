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
