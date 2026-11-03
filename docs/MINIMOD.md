# minimod — a minimal MPC Sample mod

**Version 0.3.2**

`minimod` is an `LD_PRELOAD` library for the Akai MPC Sample (app
`/usr/bin/MPC Sample`, firmware 1.3.0.12) that adds two small features without
touching the firmware on disk. It is delivered as an SD `autorun` package (see
[`ECOSYSTEM.md`](ECOSYSTEM.md) for the boot contract) and disappears on reboot.

## Status

| # | feature | control | state |
| --- | --- | --- | --- |
| 1 | pitch shift in 16 Levels, ±8 semitones (half the pad grid) | `PLUS` / `MINUS` while 16 LEVELS is lit | working, **verified on device 2026-11-03** |
| 2 | sample autotrim to silent edges | `SHIFT` + `B3` | analysis + layer write verified; **markers do not refresh** (see §"What is still open") |
| 3 | chromatic MIDI from an external controller | automatic while 16 LEVELS is lit | note path verified with simulated MIDI |
| 4 | shift overlay label | `SHIFT` held on the Sample page | **working** — third soft key overdrawn with `AutoTrim` |

```
SHIFT held : Loop Lock   Normalize   AutoTrim      <- AutoTrim is ours
released   : Trim        Tune        Filter
```

## Architecture

A shared object loaded into the app with `LD_PRELOAD`. On init it:

1. **Finds the app base** with `dl_iterate_phdr`, picking the `PT_LOAD` that
   contains file offset `0xb55b00` (the app is PIE, so runtime address =
   `base + file VA`).
2. **Maps two pages near the base** (`MAP_FIXED_NOREPLACE`, within ±128 MB so a
   `B` can reach them): one for relay slots, one for trampolines.
3. **Hooks a function** by overwriting its **first instruction** with
   `B slot`; the slot saves nothing and simply jumps to our C hook. The hook
   calls the trampoline, which holds a **copy of the function's prologue** and
   then branches into the body at the first instruction it did not copy:

   * the copy stops before any pc-relative instruction (`adrp`/`adr`/`B`/`ldr`
     literal), because those compute addresses from their own location — they
     must run in place;
   * the body then runs on its own frame and returns straight to the hook, so
     `x19`–`x28` are restored by the hook's normal epilogue.

   Two failure modes are guarded: a site whose first word is neither `paciasp`
   nor `bti c` is refused, and **both pages must be `mprotect`ed executable** —
   a missing `mprotect` on the trampoline page made every hook jump into a
   non-exec mapping and die with SIGSEGV at the trampoline address.
4. **Hooks the screen's paint slot** (a vtable swap, see "Shift overlay").
5. Refuses to hook the same address twice and logs every install
   (`minimod: hook 0x… (prologue N)`).

The constructor also installs a `SIGSEGV`/`SIGBUS`/`SIGILL`/`SIGABRT` handler
that logs `FAULT sig=… pc=… lr=… sp=…` — that is what pinned the trampoline bug
in one run.

Runtime switches: `MINIMOD_HOOKS` (bitmask), `MINIMOD_DEBUG=1`,
`MINIMOD_PROBE=1` with `MINIMOD_PROBES=<bits>`, `MINIMOD_TESTTRIM=0.09,0.45`,
`MINIMOD_MIDI=1`. Log: `/run/minimod.log`.

## Firmware anchors (validated)

| address | meaning |
| --- | --- |
| `0x1545a50` | 16-Levels constructor |
| `0x21472f0` | 16-Levels render `(self, pad, ctx)` |
| `0x27e4400` | 16-Levels pitch offset setter `(ctx, ?, ?, semitones)` |
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
| `0x1082810` / `0x102dca0` | `Font(height)` / `Graphics::setFont` |
| `0x15e70c0` | trim core `(float value, TrimParameter*)` |
| `0x13bb930` | value broadcaster `(property, float)`; keeps `this` in `x20` |
| `0x11bae00` | `Label::getText` — **not** used by the soft-key captions |
| `0x16b6060` | `AC50WaveformLocator` bounds: `x = round(pos@+0x168 · view->width@+0x28 − 2)` |
| `0x115f590` | `Component::setBounds(comp, x, y, w, h)` (3655 direct callers) |
| `0x3adf708` / `0x3492d10` | "Normalize" menu item built and added as id 5 (`0x45a28d0`) |

Object identities (vtable address points, i.e. what a live object stores):
`SampleModeScreen` `0x6e212a0`, `ui::Slider` `0x706e5d0`,
`ui::SampleModeWaveformViewport` `0x6e22408`,
`ui::SampleWaveformWithOverlays` `0x6e23300`,
`AC50WaveformLocator` `0x6e259e0`,
`TrimParameter` `0x6e1b240` (secondary vtables `0x6e1b290`, `0x6e1b2f0`,
`0x6e1b318`), and the page's per-parameter binding class `0x6e08a10` (property
pointer at `+0x20`). `SampleModeScreen`'s vtable slot **27** is
`Component::paint(Graphics&)`. Resolve any vptr with `tools/re/classof.py`.
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

## Feature 1 — pitch shift in 16 Levels

With 16 LEVELS active the pad row becomes a chromatic keyboard of 16 notes, and
`PLUS` / `MINUS` move the whole row by **8 semitones** — half the grid.

Two details matter, both learned the hard way on hardware:

1. **The keys must be swallowed.** `0x159f2f0` is the button dispatch; both keys
   arrive there as codes `0x36` / `0x37`. If the hook falls through to
   `orig_buttons(event)` the stock action still runs and the *selected knob*
   increments by one — which is exactly what the first hardware build did. The
   handler now returns immediately for those two codes:

   ```c
   if (code == 0x36 || code == 0x37) {
       if (down) bump_octave(code == 0x36 ? +1 : -1);
       return;
   }
   ```

2. **The shift belongs in `0x27e4400`, not in the render entry.** The 16-Levels
   renderer `0x21472f0(self, pad, ctx)` computes the semitone offset as
   `(pad & 0xf) - *(int *)(self + 0x228)` and hands it to
   `0x27e4400(ctx, ?, ?, offset)` — a *context object*, not a float. Writing
   through the renderer's third argument corrupts that object and moves no
   pitch. The hook is on the setter instead:

   ```c
   static void pitch_off(void *a0, uint64_t a1, uint64_t a2, int a3)
   {
       int off = a3 + 8 * g_octave;      /* half of the 16-note grid */
       orig_pitch_off(a0, a1, a2, off);
   }
   ```

   `0x21472f0` remains hooked but is a pure pass-through.

Note `0x21472f0` branches on `*(int *)(self + 0x88)` (modes 2/3/4/6/7); the
`0x27e4510` branch used by one of them is **not** hooked yet, so a mode that
takes it would shift nothing.

3. **Both keys at once reset the row.** Each key keeps a held flag; a press that
   arrives while the *other* key is already down clears the shift instead of
   stepping it:

   ```c
   if (plus ? g_minus_held : g_plus_held) {
       g_octave = 0;                      /* back to the original pitch */
   } else {
       bump_octave(plus ? +1 : -1);
   }
   ```

   Pressing them "together" therefore ends at zero even though the first key of
   the pair steps once before the second one clears it.

Verified on device 2026-11-03: `PLUS` / `MINUS` move the row by 8 semitones,
both together reset it, and neither changes the selected knob.

## Feature 2 — autotrim

`SHIFT` + `B3` on the Sample page trims the sample the selected pad plays to its
silent edges. The whole chain now works on a stock project.

### Pipeline

1. **Find the page's trim parameters.** Scan writable memory for the
   `TrimParameter` vtable (`base + 0x6e1b240`), then keep only the ones that are
   *wired*: the parameter's context must resolve to an instrument whose layer
   holds a sample whose frame count equals `[param + 0x1e8]`. This is what makes
   a parameter live rather than a stale copy.
2. **Bind Start/End.** Two wired parameters with the same length; prefer the
   ones that already carry a value (`+0x19c >= 0`), i.e. the page-bound ones.
3. **Resolve the sample and its file.** Layer chain:
   `[param+0xb0] → +0x60 → instrument → +0x22e8 → layers + pad*0xb30 + 0x4a8`.
   The `Sample` object's WAV path sits two indirections in
   (`sample+0x30 → +0x120 → juce::String`); a depth-2 pointer scan finds it.
   The frame count from the parameter validates the file.
4. **Compute the edges** from the file's own PCM, channel-aware, ~−40 dBFS:
   24-bit stereo is the normal case in the stock library.
5. **Write the state**: trim core `0x15e70c0(value, param)` (stores `+0x19c`),
   the boundary object (`+0x38`, `+0xa8`) and the layer's frame fields
   (`+0x4d8`, `+0x588`, `+0x530`).
6. **Refresh the page.** Broadcast through the parameter's property
   (`0x13bb930`) so the page's cached marker percentages follow. The property is
   learned from a real knob move (the broadcaster keeps `this` in `x20` while it
   calls the trim core) or found read-only: a candidate must be a heap object
   with a float at `+0x50`, a listener list whose first node points back at
   `P+0x10`, and a chain that references the parameter or its model.

Verified run (stock project, `BoomBap-Kik-BTB MD`, 15 129 frames):

```
autotrim: 3/3 params are wired (len=15129)
path from sample+0x30/+0x120 -> …/BoomBap-Kik-BTB MD.wav
autotrim: frames=15129 ch=2 edges=1..12433
autotrim frames=15129 -> 0.0001..0.8219
layer 0xffffe36f5d60 <- 1..12433 frames
commit 0.0001/0.8219
after layer=1/12434
```

### Why the obvious things did not work

* The trigger used to run `do_autotrim()` **on the UI thread**, and the edge
  scan read the tail **one `pread` per frame** — 15 k syscalls for a kick, a
  hang for a 500 k-frame sample. It reads 16 KB blocks now.
* The commit used to run **inside `repaint_hook`**, i.e. inside the paint
  machinery; calling the trim core and broadcasting from there hung the UI. It
  runs from the button handler now, on the message thread.
* Broadcasting a property that was guessed by "looks like a property" **aborts
  the app** (SIGABRT inside the broadcaster). Only a verified property may be
  broadcast; ambiguity means skip.

### What is still open in autotrim

The analysis and the layer write are correct and verified; what does not happen
is the **page repainting its Start/End locators**. Everything that was ruled out,
with the measurement that ruled it out:

| candidate | result |
| --- | --- |
| trim core + boundary + layer write | state lands, paint unchanged |
| property broadcast `0x13bb930` (after fixing the pre-write that suppressed it) | no visual change |
| observable setter `0x14a7580` | never called by the knobs |
| control-surface handler `0x23aff60` | never fires for panel knobs |
| native property command `0x1671650` on scanned `SamplePadControls` properties | no property in this firmware references the trim parameters |
| synthetic CC sweep through the button dispatch | reaches nothing |
| page cache fields `+0x1f4c/+0x288c/+0x72c8` | written, re-derived on the next paint |
| all 34 `ui::Slider` widgets (`+0x1b4`) | every write repaints to the *same* frame, i.e. the paint re-derives the widget from the model |
| hardware watchpoint (perf `HW_BREAKPOINT_W`) | never fires: QEMU does not virtualize the debug registers |
| QEMU gdbstub `Z2` watchpoint | rejected with `E22` under HVF |
| software watchpoint (page `PROT_READ` + fault handler) | armed, never faults on a knob turn |

Conclusion: the page paints from a model object that is not any of the
parameters, properties, caches or widgets tested above. The instrument that can
settle it is a `Z2` watchpoint under TCG (accepted by the stub, too slow to use
interactively on this host) or `gdbserver` on real hardware.

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

Holding `SHIFT` on the Sample page relabels the soft keys. Getting the third one
to read **AutoTrim** is done by **overdrawing**, the technique am-mod uses (see
[`AM-MOD-NOTES.md`](AM-MOD-NOTES.md)):

1. take `SampleModeScreen`'s vtable (`base + 0x6e212a0`), `mprotect` the
   `.data.rel.ro` page writable, save `vt[27]` (that slot is
   `Component::paint(Graphics&)`), and install our own paint;
2. our paint calls the stock one first, then — only while `SHIFT` is held and
   only outside Chop mode — fills the third cell (`x 213..320, y 0..29`) black
   and draws `AutoTrim` with
   the app's own `Graphics`: `setColour 0x102d5e0`, `fillRect 0x102e000`,
   `Font 0x1082810`, `setFont 0x102dca0`, `drawText 0x10817b0`;
3. `SHIFT` press/release forces a repaint (`0x1149880(screen, 0, screen[0x28])`).

Two earlier attempts are dead ends and are recorded as such: the captions are
**not** `juce::Label` (`Label::getText` `0x11bae00` is never called for them) and
they never pass through `String::operator=(char*)` (`0xf535c0`), so renaming was
never going to reach them.

## Build and try it

```sh
cd mod
./build.sh     # build/minimod.so  (aarch64 glibc, LD_PRELOAD)
./pack.sh      # build/minimod.zip (autorun package with a valid autorun.check)
```

Emulator:

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py run          # panel on :8865
MPCLAB_PORT_OFFSET=100 python3 lab.py preload \
  /Users/jakubkolcar/projects/customs/sample-converter/mod/build/minimod.so
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh 'cat /run/minimod.log'
```

`preload --off` removes it; a reboot also does, because the drop-in lives in
`/run`.

Device: unzip `build/minimod.zip` and copy `autorun/` to the card root (or
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

## How it was achieved

The order in which the blockers fell, with the evidence that settled each:

| # | symptom | root cause | fix |
| --- | --- | --- | --- |
| 1 | every trim attempt "did nothing" | the sample path only matched 16-bit WAVs, and Akai writes a large metadata chunk before `data` | chunk-walking parser, 16/24/32-bit, width-aware edge scan |
| 2 | sample never resolved | the path is two indirections inside the `Sample` object | depth-2 pointer scan (`sample+0x30 → +0x120`) |
| 3 | app aborted after the commit | broadcasting a **wrong** property | learn from `x20` at a real knob move, else verify read-only; ambiguity ⇒ skip |
| 4 | UI froze after the trim | tail scan did one `pread` per frame on the UI thread | 16 KB block scan |
| 5 | UI froze again | the commit ran inside `repaint_hook` | commit moved to the button handler |
| 6 | hooks silently missing | sites whose prologue holds a pc-relative instruction were refused outright | copy the prologue only up to that instruction and branch in place |
| 7 | every hook killed the app | the trampoline page was never `mprotect`ed executable (fault PC == trampoline) | map + `mprotect` both pages R-X |
| 8 | no "AutoTrim" label | captions are not `juce::Label` and never go through the char* assign | overdraw the screen's paint slot (am-mod technique) |

What made this tractable: a **fault handler inside the mod** (PC/LR/SP straight
into the log), a **log-first workflow** (every probe writes what it measured),
and `lab.py preload` + a restart per build so the device always ran exactly the
binary just built.

## Negative findings (kept so they are not retried)

| attempt | outcome |
| --- | --- |
| write sample struct `+0x3d4/+0x3dc` | succeeds, UI untouched (library metadata) |
| write `slider+0x20` | breaks the page render |
| hook `Label::getText` and return another string | no effect on paint; the hook also crashed the app when Sample Select opened |
| hook `Graphics::drawText` to detect the third label | it is not drawn through the global hook; overdrawing the screen paint works |
| synthetic control-surface event replay | dispatcher needs more state; no effect |
| `RotaryFixedConnection` (`0x15f5690`) | never called for the Trim knobs |
| assign at the `Filter` string site | changes tab row and shift row together (one shared widget) |
| write property objects with small-int values | triggered sample loading (selection parameters) |
| reuse `g_setter[]` addresses captured at startup | they are freed and reused by the app |
| `find_displayed_sample()` (screen child with `+0x130` → info struct) | no child matches this build; always logs "not found" |

## Repository layout

```
mod/src/minimod.c    the mod
mod/build.sh         zig cc -target aarch64-linux-gnu -shared
mod/pack.sh          builds the autorun zip (pins the firmware by first-4KB hash)
mod/build/           minimod.so, minimod.zip
docs/ECOSYSTEM.md    boot contract, boot-thingy, SampQOL/am-mod, apps, emulator
docs/MOD-MINI.md     this document
docs/TRIM-PAGE.md    the Sample/Trim page: layout, marker source, findings A–J
docs/AM-MOD-NOTES.md the overdraw technique taken from am-mod
```

## On-screen feedback

Both features confirm themselves with the same one-second toast: a `320x46` band
over the Start/End/Loop bar, near-black fill, amber 2 px rules top and bottom and
white 18 pt text, drawn by the paint hook and cleared by repainting through
`A_REPAINT`. It costs nothing when idle (a string plus a deadline check).

| action | toast |
|---|---|
| `SHIFT` + `B3`, first press | `Trim 3% of peak` |
| second press | `Trim 6% of peak` |
| third press | `Trim 9% of peak` |
| fourth press | `Trim cleared` |
| `PLUS` / `MINUS` | `Pitch +8 semitones`, `Pitch +16 semitones`, `Pitch -8 semitones`, … |
| `PLUS` + `MINUS` together | `Pitch reset` |

The label always states the resulting state of the sample or the row, not the
key that was pressed, so `Pitch +16 semitones` is what the pads now play.
