# mpc-mini — a minimal MPC Sample mod

`mpc-mini` is an `LD_PRELOAD` library for the Akai MPC Sample (app
`/usr/bin/MPC Sample`, firmware 1.3.0.12) that adds three small features without
touching the firmware on disk. It is delivered as an SD `autorun` package (see
[`ECOSYSTEM.md`](ECOSYSTEM.md) for the boot contract) and disappears on reboot.

## ⚠ Firmware compatibility — 1.3.0.12 ONLY

**This mod runs on exactly one firmware build and on nothing else.**

| item | value |
| --- | --- |
| `/usr/bin/MPC Sample` (whole file) | `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508` |
| `/usr/bin/MPC Sample` (first 4 KiB) | `44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f` |
| firmware version | 1.3.0.12 |

Every hook and every field offset is an **absolute file offset** into that exact
binary: `0x14d0890`, `0x159f2f0`, `0x1e27360`, `0xf535c0`, `0x10817b0`,
`0x1149880`, the vtables `0x706e5d0` / `0x6e212a0`, the `"Filter"` constant
`0x4584638`, the info-struct fields `+0x3d4/+0x3dc`, the property slot `+0x20`.
A later firmware reorders code and read-only data, so those addresses point at
unrelated instructions — the mod does **not** adapt, and there is no fallback
detection beyond the guards listed below.

What happens on a different build:

* **SD / autorun package — safe.** `autorun.sh` hashes the first 4 KiB of the
  stock app and compares it with `44507c7a…`; on a mismatch it exits without
  copying or loading anything. Nothing runs, nothing crashes.
* **`lab.py preload` or a manual `LD_PRELOAD` — unsafe.** There is no such guard
  in that path: an old library patched into a new app will write at wrong
  addresses and take the app down.
* **Per-hook sanity only.** `install_hook` requires `paciasp` or `bti c` at the
  entry and refuses an address that already holds a `B`. That catches obvious
  mismatches, it is not a version check.

Supporting a future firmware means producing a new profile of addresses for that
build (see the "What would make it update-tolerant" discussion in the project
notes): a per-build hash table, signature scanning instead of fixed VAs, and a
passive fallback when the build is unknown.

Everything below was measured against this build and exercised in the emulator
(`virtual-mpc-sample`).

## Status

| # | feature | control | state |
| --- | --- | --- | --- |
| 1 | octave shift in 16 Levels → Tune | `PLUS` / `MINUS` while 16 LEVELS is lit | working, verified |
| 2 | sample autotrim to silent edges | `SHIFT` + `B3` | **broken** — the trigger runs, but the write lands in an unrelated property (root cause in "Root cause" below and in [`TRIM-PAGE.md`](TRIM-PAGE.md)) |
| 3 | chromatic MIDI from an external controller | automatic while 16 LEVELS is lit | note path verified with simulated MIDI |

Shift overlay on the Sample page:

```
SHIFT held : Loop Lock   Normalize   AutoTrim
released   : Trim        Tune        Filter
```

## Architecture

The library is a shared object loaded into the app process. On init it:

1. **Finds the app base** with `dl_iterate_phdr`, selecting the `PT_LOAD` that
   contains file offset `0xb55b00` (the app is PIE; runtime addresses are
   `base + file VA`).
2. **Builds a relay page** via `mmap(MAP_FIXED_NOREPLACE)` near the base so a
   `B` (26-bit, ±128 MB) from the app can reach it. An earlier version mapped it
   far away and the app died on the first hook — the branch range is the
   constraint.
3. **Installs hooks**: the word at `entry + 4` is overwritten with `B relay`, and
   the displaced word is replayed by a trampoline before jumping back to
   `entry + 8`. Two entry styles are handled:
   * `paciasp` — the relay emits `AUTIASP` and the trampoline re-emits `PACIA`
     so the return address stays signed correctly;
   * bare `bti c` — no PAC handling.
4. Refuses to hook the same address twice (a double `AUTIASP` corrupts the
   signature) and logs when the relay runs out of slots (64 slots of 64 bytes; an
   earlier 8-slot page silently dropped hooks and invalidated several probe
   results).

Runtime switches (all off by default): `MPCMINI_HOOKS` (hook mask),
`MPCMINI_SAFE=1` (pass-through), `MPCMINI_DEBUG=1`, `MPCMINI_PROBE=1` with
`MPCMINI_PROBES=<bits>`. The log is `/run/mpcmini.log`.

## Firmware anchors (validated)

| address | meaning |
| --- | --- |
| `0x1545a50` | 16-Levels constructor |
| `0x21472f0` | 16-Levels processing |
| `0x159f2f0` | physical button dispatch `(event)`; `event+8` = code, `event+9` = value |
| `0x1e27360` | note on `(input, src, pad, props, vel)` |
| `0x1e270d0` | note off `(input, src, pad)` |
| `0x23aff60` | control-surface event |
| `0x14d0890` | **generic property setter** `set(this, value)` |
| `0x1660370` | screen resized |
| `0x2941f80` | sample frames getter (`[x0+0x130] → +4`) |
| `0x286bbe0` | instrument accessor `(program, index)` |
| `0x15f5690` | rotary connection core (not used by the Trim knobs) |
| `0xf535c0` | string assign `(dest, const char*)` |
| `0x10817b0` | `Graphics::drawText` |
| `0x102d5e0` / `0x102e000` | `Graphics::setColour` / `fillRect` |
| `0x1149880` | component repaint `(p, 0, p[0x28])` |
| `0x11bae00` | `Label::getText` |
| `0x16b6060` | `AC50WaveformLocator` bounds: `x = round(pos@+0x168 · view->width@+0x28 − 2)` |
| `0x115f590` | `Component::setBounds(comp, x, y, w, h)` (3655 direct callers) |
| `0x3adf708` / `0x3492d10` | "Normalize" menu item built and added as id 5 (`0x45a28d0`) |

Object identities (vtable address points, i.e. what a live object stores):
`SampleModeScreen` `0x6e212a0`, `ui::Slider` `0x706e5d0`,
`ui::SampleModeWaveformViewport` `0x6e22408`,
`ui::SampleWaveformWithOverlays` `0x6e23300`,
`AC50WaveformLocator` `0x6e259e0`,
`TrimParameter` `0x6e1b240` (secondary vtables `0x6e1b290`, `0x6e1b2f0`,
`0x6e1b318`). Resolve any of them with `tools/re/classof.py`.
Property offsets on 16-Levels objects: `P_TYPE 0x88`, `P_ENABLE 0x2f8`,
`P_ORIG_PAD 0x228`.

**Class identification without symbols.** This build keeps RTTI, and a live
`vptr` is a vtable *address point*: `vptr − 8 → typeinfo → +8 → name`. Verified:
`u64(0x706e5d0 − 8) = 0x706e518` → `"N2ui6SliderE"`. `tools/re/classof.py`
resolves a vptr, or `--scan` names every slot of a memory dump. `.data.rel.ro` is
`0x6ddf910 … 0x7162018` and `.rela.dyn` has 491,458 entries (464,288 of them
`R_AARCH64_RELATIVE`). Class names of the Sample page, from the same table:
`ui::SampleModeScreen`, `ui::Slider`, `ui::SampleModeWaveformViewport`,
`ui::SampleWaveformWithOverlays`, `SampleEditWaveformContainer`,
`GuiSampleEditWaveformDisplay`, `TouchSampleWaveformComponent`,
`Start/End/Loop/Playhead/Slice/Cue WaveformLocator`, `Gui*WaveformLocator`,
`AC50WaveformLocator`, `TrimParameter`, `NormalizeSampleInBackgroundCommand`.

**Sample-page object layout** (measured, see [`TRIM-PAGE.md`](TRIM-PAGE.md)):
the `ui::SampleModeScreen` object embeds its widgets — 26 `ui::TextButton`,
15 `juce::Label`, 12 `ui::ImageButton`, 33 `ui::Slider` objects, 4
`ui::SampleModeWaveformViewport` vptrs (`+0x18c8/+0x1968/+0x1970/+0x1a60`).
The Trim page's marker is a pure function of the percentage float at
`screen + 0x1f6c` (mirror `+0x28bc`); the screen object is recreated between
runs, so its address must be re-resolved.

Property-object layout (verified live): `[+0x08]` list sentinel / `[+0x10]` first
node, `[+0x18]` change count (`u32`), `[+0x20]` value. A live object has
`[+0x08] == this + 8`. The app **frees and recreates** these objects, so pointers
captured at startup go stale — 13 of 16 captured blocks no longer matched this
layout in one run.

Panel button codes measured on the wire: `SHIFT 0x31`, `16 LEVELS 0x28`,
`PLUS 0x36`, `MINUS 0x37`, `DATA push 0x6F`, soft keys B1/B2/B3
(`SCREEN 1/2/3`) `0x0C / 0x0D / 0x0E`.

## Feature 1 — octave shift

With 16 LEVELS active the pad row becomes a chromatic keyboard, and `PLUS` /
`MINUS` move it by an octave. The mod intercepts those codes in the button
dispatch (swallowing them so the stock action does not also run) and calls the
note path with `pitch = ±12`.

Verified: `PLUS` → `pitch +12`, `MINUS` → `pitch -12`, app stable.

## Feature 2 — autotrim

The **edge maths is right** (channel-aware threshold on the sample's own PCM);
everything around it — which sample, which property, which write path — was
wrong and is now documented in [`TRIM-PAGE.md`](TRIM-PAGE.md).

### The property setter

`0x14d0890` is not trim-specific; decompiled it is a generic observable setter:

```
set(this, value):
    old = [this + 0x20]
    if (old == value) return
    ++count @0x18
    [this + 0x20] = value
    …notify listeners via a vtable call…
```

The mod captures every object this setter is called on (`g_setter[]`, 15 on a
clean boot) and later writes the computed edges through the trampoline.

**Two corrections, both verified later** (details in
[`TRIM-PAGE.md`](TRIM-PAGE.md) §4–§5):

* the Trim knobs (`K1`/`K2`) do **not** go through this setter — turning them
  produces no `setter[N]` call at all, so this is the wrong write path;
* the captured pointers go **stale** — the app frees and recreates those
  objects, so a pointer captured at startup writes into freed memory later.

### The sample info struct

The signature scan looks for `u16 channels` at `+0`, `u32 frames` at `+4`,
`u32 rate == 44100` at `+8` in writable memory. The same struct carries:

| offset | meaning |
| --- | --- |
| `+0x3d0` | tempo (float, e.g. `116.375`) |
| `+0x3d4` | library trim start (frames) |
| `+0x3dc` | library trim end (frames) |
| `+0x3e4` | root note (`60`) |

Those `+0x3d4/+0x3dc` fields are **library metadata**: writing them does not move
the UI markers, which is what proved they are not the live state.

### Locating the right sample and its PCM — as implemented, and why it fails

The current code tries to guess both the property pair and the sample:

1. **Pick the struct by frame count.** The live End value is taken from the
   captured properties; the struct chosen is the one whose `frames` is the
   smallest value `>= End + 1` (within 4×).
2. **Locate and validate PCM**: scan forward up to 4 MB from that struct for a
   run of 256 `int16` that is audio-like (`|v| < 22000`, `> 100` non-zero, few
   `0xAAAA` patterns), then validate the expected length
   `frames × channels × 2` by checking that samples at `len − 128` are also
   audio-like. Candidates that fail are rejected (`reject +0x… tail=…`).
3. **Map it to a file**: `path_near()` / `owner_path()` walks the heap for a
   `.wav` string near the struct and reads the WAV header to confirm `frames`.

Both guesses are wrong on this build; see "Root cause" below for the verification. The
step-3 heap walk is also unbounded and can run for minutes.

Edges themselves are correct: the first/last frame whose magnitude exceeds ±328
(≈ −40 dBFS), scanned from both ends (channel-aware, so stereo does not double
the indices).

## Feature 3 — chromatic MIDI

The note-on hook receives `(input, src, pad, props, vel)`. A hit that carries the
16-Levels properties is a pad press; a note **without** them is an external MIDI
note. When 16 LEVELS is active the mod translates the incoming note onto the
16-Levels sample: `pitch = pad − 24` (MIDI 60 = C4 = the sample's own pitch) and
plays it through the app's note path; note-off is tracked per pad.

Verified with simulated notes: note 64 → `pitch +4`, note 55 → `pitch −5`, plus
matching note-off.

Physical controllers could not be exercised: every `/dev/snd/midiC?D0` except the
panel device returns `EBUSY` because the app owns them. The interposition point
for real hardware is `librbinmidi.so` (`midiRegisterRecvMsg`, `midiRecvPayload`)
or the ALSA sequencer.

## Shift overlay label

Holding SHIFT on the Sample page relabels the three soft keys. Getting the third
one to read **AutoTrim** required understanding how the stock labels are built:

* a `juce::String` keeps short text **inline in its `StringHolder`**: characters
  start at the holder and the refcount sits at `+0x18`; measured bytes are
  literally `5374617274…` = `"Start"`, `"End"`, `"Filter"`;
* the three shift labels are built **once at startup** through the string assign
  `0xf535c0`; pressing SHIFT issues **no assignment at all**, so there is nothing
  to intercept — the app only reveals different widgets;
* the third slot is a single widget shared by the tab row and the shift row.

The working mechanism therefore rewrites the stored text and forces a repaint:

1. on SHIFT press, find every writable holder whose inline text is `"Filter"`
   (2 of them) and overwrite the 16-byte inline buffer with `"AutoTrim"`
   (8 chars + NUL fit; the refcount at `+0x18` is untouched);
2. restore `"Filter"` on release;
3. force a repaint with `FN(0x1149880)(screen, 0, screen[0x28])`, where `screen`
   is found by scanning for the `SampleModeScreen` vptr `base + 0x6e212a0`.

`Graphics::drawText` is a dead end for this slot: with logging enabled it is
called only for the *first* soft-key label and the footer labels
(`rect=16,4 72x23` repeatedly), never for the third.

## Build and try it

```sh
cd mod
./build.sh     # build/mpcmini.so  (aarch64 glibc, LD_PRELOAD)
./pack.sh      # build/mpc-mini.zip (autorun package with a valid autorun.check)
```

Emulator:

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py run          # panel on :8865
MPCLAB_PORT_OFFSET=100 python3 lab.py preload \
  /Users/jakubkolcar/projects/customs/sample-converter/mod/build/mpcmini.so
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh 'cat /run/mpcmini.log'
```

`preload --off` removes it; a reboot also does, because the drop-in lives in
`/run`.

Device: unzip `build/mpc-mini.zip` and copy `autorun/` to the card root (or
install the zip as a boot-thingy app).

## Emulator and workflow hazards (all hit during this work)

* **Ports**: another emulator on the machine held the default 5554/5555, so
  `MPCLAB_PORT_OFFSET=100` shifts every port (panel becomes `8865`, ssh `2122`).
* **The SD card is baked at boot**: the host folder `sdcard/` becomes a FAT image
  *when the VM starts*. Creating `/media/...` inside the guest does nothing — it
  is tmpfs and the app re-mounts over it. Adding samples later needs a VM restart
  (or `sdcard_image()` + `card_swap()` once a card device already exists).
* **The app saves the project on exit.** A speculative write that corrupts state
  can therefore survive restarts; the cure is `lab.py reset` (fresh eMMC), which
  is how the emulator was recovered after the early write experiments.
* **`/screen.png` can go stale** (it returned identical frames across app
  restarts while the panel itself stayed live). Trust the log and the mod's own
  read-backs; treat screenshots as a secondary check.
* **`mapped()` needs `ranges_load()`** — probes that skipped it silently treated
  every address as unmapped, producing several false "not found" readings.
* **A bulk source edit once deleted the mod's `rd`/`wr`/`mapped`/`ranges_load`
  helpers**, so several "builds" failed while an older library kept being
  deployed. Check compiler output, not just that a `.so` exists.

## Root cause — the write lands in the wrong property (verified)

The earlier note here said the page "draws from a fifth store we have not found".
That is **disproved**; the full evidence is in [`TRIM-PAGE.md`](TRIM-PAGE.md).
Short version:

* The marker and the shading are a pure function of one percentage `float` in the
  screen object (`screen + 0x1f6c`, mirrored at `+0x28bc`). Turning `K1` changes
  **only** those bytes (0→100 % sweeps the marker over `x = 15…304`) — there is
  no hidden store.
* The Trim knobs do **not** go through the hooked property setter `0x14d0890`:
  turning `K1`/`K2` produces no `setter[N]` log lines at all.
* The pointers in `g_setter[]` are **stale**: the app frees and recreates those
  objects, so 13 of 16 captured blocks no longer have the setter layout
  (`[+0x08]` sentinel, `[+0x18]` count, `[+0x20]` value). Reusing them writes
  into freed memory.

A clean trigger shows what that produces. `SHIFT`+`B3` on a fresh app:

```
mpcmini: trim source /usr/share/Akai/AC50/Metronome/Clap.wav frames=10165 ch=2 edges=1..9878
mpcmini: autotrim frames=10165 edges=1..9878 have=1 lo=2 hi=2
mpcmini: autotrim 1..9878; start 85574->1 end -1->-1
mpcmini: chg setter[2] 85574 -> 85661          <-- app overwrites it next tick
```

`lo == hi == 2` is the app's **monotonic audio-position counter**, not a trim
parameter: the only property being written continuously is that one, and the
selection heuristic is "most recent, then min/max value". The projected screen
stayed byte-identical (`sha256` prefix `6e5c5b7d7c` before and after).

Three independent defects, each verified:

| # | defect | evidence |
| --- | --- | --- |
| 1 | Start/End are picked by recency + min/max, so they resolve to the audio-position counter | log above: `lo=2 hi=2`, read-back `85574->1`, counter continues from `85661` |
| 2 | the sample is resolved from *any* `.wav` on disk that matches the struct's frame count — here a factory metronome, not the page's sample | `Clap.wav` (10165) / earlier `Tambourine.wav` (6999) vs the page's `TRIM-Test-A002.wav` = 485144 B = 121275 stereo frames |
| 3 | the `.wav` search is an unbounded heap walk and can run for minutes, so the press looks like a no-op | one run produced the scan lines and then no `trim source` for several minutes; the UI never changed |

Anything that fixes this must stop guessing: resolve the **live** trim pair at
trigger time, resolve the **selected** sample from the app (its layer / the
parameter itself), and write through the setter the page actually uses. The
candidate designs are listed in [`TRIM-PAGE.md`](TRIM-PAGE.md) §9.

`0x14d0890` remains correct as a *description* of a property object (it is a
plain observable: compare `[+0x20]`, store, notify listeners, `[+0x18]` is a
change count) — it is just not the object or the path that the Trim page uses.

## Negative findings (kept so they are not retried)

| attempt | outcome |
| --- | --- |
| write sample struct `+0x3d4/+0x3dc` | succeeds, UI untouched (library metadata) |
| write `slider+0x20` | breaks the page render |
| hook `Label::getText` and return another string | no effect on paint; the hook also crashed the app when Sample Select opened |
| hook `Graphics::drawText` and draw over the slot | never sees the third label |
| synthetic control-surface event replay | dispatcher needs more state; no effect |
| `RotaryFixedConnection` (`0x15f5690`) | never called for the Trim knobs |
| assign at the `Filter` string site | changes tab row and shift row together (one shared widget) |
| write property objects with small-int values | triggered sample loading (selection parameters) |
| reuse `g_setter[]` addresses captured at startup | they are freed and reused by the app |
| `find_displayed_sample()` (screen child with `+0x130` → info struct) | no child matches this build; always logs "not found" |

## Repository layout

```
mod/src/mpcmini.c    the mod
mod/build.sh         zig cc -target aarch64-linux-gnu -shared
mod/pack.sh          builds the autorun zip (pins the firmware by first-4KB hash)
mod/build/           mpcmini.so, mpc-mini.zip
docs/ECOSYSTEM.md    boot contract, boot-thingy, SampQOL/am-mod, apps, emulator
docs/MOD-MINI.md     this document
docs/TRIM-PAGE.md    the Sample/Trim page: layout, marker source, root cause
```
