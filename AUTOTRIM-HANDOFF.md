# AutoTrim mod — handoff

Everything a new engineer needs to pick this up and finish it. Written after the
implementation, the code review, the release-hardening pass and the field
regressions, so it records what was measured, not what was assumed.

Companion documents (read in this order for depth):

| document | contents |
|---|---|
| `docs/MOD-REFERENCE.md` | precise reference: architecture, hook table, all RE findings |
| `docs/CODE-REVIEW.md` | 20 review findings + fix status + the regression bisect |
| `docs/TRIM-PAGE.md` | the long investigation log (§1–32), including dead ends |
| `docs/RE-GHIDRA.md` | Ghidra + MCP setup, the 12.2 extension rebuild |
| `docs/re/*.c` | decompiled reference functions |
| `HYPOTHESES.csv` | hypothesis log |

---

## 1. TL;DR status

**Works and ships:**

* AutoTrim on **SHIFT + B3** — analyses the selected pad's sample, trims to its
  silent *and low-level-noise* edges, writes the app's own trim parameters and
  property, and draws the result on the Trim page.
* Pitch shift in 16 Levels — `+` / `-` move the row by 8 semitones, PLUS+MINUS
  resets.
* Chromatic control from an external MIDI controller (opt-in).

**Verified in the emulator (numbers, not opinions):**

```
commit core done p0=0.0000 p1=0.8268 (want 0.0001/0.8254)
notified props 0xffffe576c718/0xffffe576c770
AutoTrim screen delta: 5196 px   (overlay: shade + edges)
pid unchanged across trims; 20 threads
```

**Known open items** — see §9. The main one: the app's *own* Start/End widgets
do not re-render from the trim value; the trim is drawn by the mod's overlay.
That is a cosmetic/architectural gap, not a functional one (parameters are exact
and the sample really is trimmed).

---

## 2. What the mod is

An `LD_PRELOAD` shared library (`mpcmini.so`) loaded into the stock app by an SD
`autorun` script. It never modifies the firmware on disk — it patches a few
instructions in memory at load time.

Target, **1.3.0.12 only**:

```
/usr/bin/MPC Sample
sha256 b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508
aarch64 PIE ELF, ~119 MB, stripped
```

The loader refuses to install anything if the digest does not match, so a wrong
firmware means "no mod", not "crash".

---

## 3. How it works

### 3.1 Loading and hooking

1. SD autorun sets `LD_PRELOAD=/path/mpcmini.so` and restarts the app.
2. `dl_iterate_phdr` finds the app base (the `PT_LOAD` covering file offset
   `0xb55b00`). **Runtime address = base + file VA**, base typically
   `0xaaaaaaaa0000`.
3. Each hook site gets a relay page and a trampoline page; the entry instruction
   is replaced by `B <relay>`.
4. **The site must start with `paciasp` or `bti`**, otherwise the installer
   refuses (logs `site %p has no paciasp/bti`). Functions with a plain prologue
   cannot be hooked this way — that is a hard constraint, not a bug.
5. `pc_relative()` decides how many prologue instructions may be copied into the
   trampoline: it stops at `B/BL/ADRP/ADR/LDR-literal/CBZ/CBNZ/TBZ/TBNZ/BR/BLR/RET`.

### 3.2 AutoTrim pipeline

1. **Key detection** — the button hook reads `code` at `event+8`, `down` at
   `event+9`. **SHIFT = `0x31`, B3 = `0x0e`.** While SHIFT is held the mod also
   rewrites the app's own *Filter* tab label to **"AutoTrim"** and restores it on
   release.
2. **Find the displayed sample** — `find_displayed_sample()` walks the Sample
   screen's child pointers for an info struct (`frames` at `+4`, `rate` at `+8`)
   and a holder that yields a `.wav` path.
3. **Analyse the WAV** — `wav_info()` parses RIFF; `wav_edges()` scans PCM for
   the first/last frame above a **peak-relative threshold** (`MPCMINI_TRIM_DB`,
   default **−40 dB**, clamped −90…−6) with an absolute −40 dBFS floor. That is
   what trims low-level noise, not just digital silence. `pcm_at()` handles
   16-, 24- and 32-bit PCM.
4. **Normalise** — `v0 = first/frames`, `v1 = (last+1)/frames`.
5. **Write the parameters** — `TrimParam_SetValue(value, param)` for Start and
   End (this is what the audio side uses).
6. **Write the model + notify** — `Property_Broadcast(prop, value)`.
   **Never pre-write `prop+0x50`**: the broadcaster early-outs when the stored
   value already equals the new one, which silently suppresses the notification.
   This cost several rounds; see `CODE-REVIEW.md` §H4 and `TRIM-PAGE.md` §31.2.
7. **Draw** — the overlay paints inside the app's paint pass.

### 3.3 Overlay

Takes the screen vtable's paint slot, keeps the original, draws on top:

* grey `(27,27,30)` shade over trimmed-away regions,
* white 1 px edges at `x = 10 + v*295`, `y = 88`, height `98`,
* the app's Filter tab label reading "AutoTrim".

It **must** draw inside the app's paint pass: the `Graphics` is a stack temporary
(`0xfffffffff648`), so caching it and painting later is a dangling pointer. Every
"force a repaint from a hook" attempt failed for this reason.

---

## 4. Key addresses and offsets

Convention: **Ghidra VA = file VA + 0x100000**; runtime = `base + file VA`.

| what | file VA | notes |
|---|---|---|
| button dispatch | `0x159f2f0` | `(event)`; code at `+8`, down at `+9` |
| trim core | `0x15e70c0` | `TrimParam_SetValue(value, param)` — value in `s0`, param in `x0` |
| `Property_Broadcast` | `0x13bb930` | `(prop, value)` — prop `x0`, value **`s0`**; store at `+0x50`, early-out when equal |
| repaint | `0x1149880` | `(comp, x, y)` |
| `fillRect` | `0x102e000` | `(g, x, y, w, h)` — used by the overlay |
| `setColour` | `0x102d5e0` | `(g, rgba)` |
| control handler | `0x2804e90` | getter; Start `0x462`, End `0x466` |
| pad→page copy | `0x280dca0` | `(page, pad)` — never seen firing |
| sample frames | `0x2941f80` | frame-count getter |

Structures:

* property: value `+0x50`, version `+0x18`, mutex `+0x20`;
* trim parameter: applied value `+0x19c`;
* `SamplePage`: `0x2468` bytes, vtable file `0x6ec3ca0`.

---

## 5. Build and deploy

```sh
cd mod
./build.sh     # zig cc -target aarch64-linux-gnu -shared -fPIC -O2 \
               #   -fno-omit-frame-pointer -mbranch-protection=standard -Wall -Wextra -s
./pack.sh      # -> mod/build/mpc-mini.zip (autorun/…)
```

Current artefacts: `mpcmini.so` ≈ 55.7 KB, `mpc-mini.zip` ≈ 29.5 KB.

**Debug build** (probes, watch thread, polling threads, TESTTRIM, signal
handlers): compile with `-DMPCMINI_DEBUG=1`. Release is the default and must stay
that way — see §9.1.

Deploy into the emulator:

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py panel          # web panel :8865 (keep running!)
MPCLAB_PORT_OFFSET=100 python3 lab.py preload <path>/mpcmini.so
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh '...'
```

**Trap:** `lab.py preload` rewrites the systemd drop-in *without* extra env vars,
so always rewrite it and restart afterwards:

```sh
printf "[Service]\nEnvironment=LD_PRELOAD=/run/mpclab-preload/mpcmini.so\n" \
  > /run/systemd/system/ac50.service.d/mpclab-preload.conf
systemctl daemon-reload; systemctl reset-failed ac50; systemctl restart ac50
```

Ports with `MPCLAB_PORT_OFFSET=100`: panel `8865`, ssh `2122`, QMP `5654`,
VNC `6010`. `lab.py preload --off` is **not** supported by the CLI; to remove the
mod, delete the drop-in file and restart.

---

## 6. How it is tested

All verification is **pixel-diffed screenshots plus the mod's own log** — no
"looks right" claims.

```python
# panel control
POST http://127.0.0.1:8865/control  {"name":"K1","action":"set","value":60}
POST ... {"name":"SHIFT","action":"press"} / {"action":"release"}
# screen
QMP -> human-monitor-command {"command-line":"screendump /tmp/fb.ppm"}   # 320x240
```

The canonical trim test:

1. `SAMPLE` press/release (open the sample page), wait ~2.5 s.
2. `K1 = 0`, `K2 = 127` (full sample).
3. `PAD 1` press/release (select the pad).
4. screenshot **before**.
5. `SHIFT` press, `SCREEN 3` press, wait ~6 s, release both.
6. screenshot **after**; count changed pixels in `y ∈ [84,190]`, `x ∈ [4,312]`.
7. Check `/run/mpcmini.log` for `commit core done`.

Expected (release build, overlay on):

| signal | expected |
|---|---|
| `commit core done p0=… p1=…` | `0.0000 / 0.8268` (knob-equivalent `0.0001/0.8254`) |
| changed pixels | ≈ 5196 |
| `notified props` | two non-nil property addresses |
| pid | unchanged across the trim |

Sanity checks for the UI being alive (a frozen UI was a real regression, §9.2):
`SCREEN 2` should change ≈ 5300 px; a knob move should change ≫ 0 px.

---

## 7. Tooling used (and how to reuse it)

### Ghidra + MCP
`ghidra-mcp v7.0.0`, bridge `/Users/jakubkolcar/.local/bin/bridge-mcp-ghidra`,
backend `http://127.0.0.1:8089`. **The extension must be built for the exact
Ghidra version** — the shipped 12.1.4 build fails against 12.2 with a
`SleighLanguageDescription` error; the rebuild recipe is in `docs/RE-GHIDRA.md`.
Import a uniquely named copy of the binary (two programs with the same name make
every call ambiguous).

### In-guest ptrace tracers
Cross-compiled static with `zig cc -target aarch64-linux-musl`:
* `knobtrace` — attach, plant `BRK`, log `pc/x0..x3/lr`, single-step, re-arm;
* store tracer — single-steps and decodes `STR/STP` immediates.

**Hazard:** never breakpoint an address the mod already hooked — the mod's `B`
gets overwritten and the restore corrupted the entry, crashing the app.

**Hazard:** in-guest hardware watchpoints are impossible here. The guest CPU
exposes no `debug`/`breakpoint` feature, so `PTRACE_SETREGSET` with
`NT_ARM_HW_WATCH` returns `ENOSPC`.

### QEMU gdbstub (the tool that actually worked)
Start it on the **running** VM:

```
QMP {"execute":"human-monitor-command","arguments":{"command-line":"gdbserver tcp::1234"}}
```

Two clients:
* a ~120-line RSP client (`Z2` write / `Z3` read / `Z4` access watchpoints,
  `m`/`M`, `Z0` software breakpoints) — **virtual addresses work**, QEMU
  translates them;
* **LLDB** (`lldb --batch -s cmdfile` → `gdb-remote 1234`), which provides the
  **conditional breakpoints** the raw client lacks:
  `break set -a 0x... -c "$x2 >= 86 && $x2 <= 188 && $x3 <= 8"`.

Operational rules learned the hard way:
* connecting **stops the VM**; a killed client leaves it paused;
* **`QMP {"execute":"cont"}` always recovers it**, even when the stub is wedged;
* a client killed mid-session **wedges the stub** (later connects fail with
  `failed to get reply to handshake packet`) and QEMU refuses to create a second
  one (`duplicate yank instance`) — only a VM restart clears it;
* stub memory access is **MMU-context dependent**: `m`/`M` can return `E14`
  unless the CPU is stopped inside the app, so prefer the mod's in-process
  read/write for anything decisive;
* breaking on a hot function without a condition is impractical — `fillRect`
  produced **64 870 stops in 40 s**.

### Emulator specifics
`lab.py stop` can take 60 s and then quits QEMU; **the next boot uses a fresh
eMMC** (`/data` and `/factory` reset). `lab.py reset` only works after a stop.

---

## 8. Debug switches (release: all off)

| env | effect |
|---|---|
| `MPCMINI_DEBUG` (build) | compiles the probes, watch thread, polling threads, signal handlers, `/run` channels, `MPCMINI_TESTTRIM` |
| `MPCMINI_TRIM_DB` | threshold in dB (default −40) |
| `MPCMINI_MIDI` | enable the external-MIDI feature (off by default: it steals the rawmidi node) |
| `MPCMINI_NOOVERLAY` | disable the overlay (used in every "does the app itself render it?" test) |

Debug builds also expose `/run/mpcmini.heap` (`set`, `findval`, dumps) and
`/run/mpcmini.watch`. **These must never ship.**

---

## 9. Current issues

### 9.1 The app's own widgets do not re-render from the trim (main open item)
The trim is drawn by the **overlay**. The app's own waveform edges and the bottom
Start/End bars do not move. Measured, not assumed:

* a read watchpoint on the property's value slot shows exactly one reader —
  `Property_Broadcast`'s own comparison at file `0x13bb988`; no paint-side PC;
* a `0x2400`-byte snapshot of the page object (vtable verified) is
  **bit-identical** across a knob turn — zero changed bytes;
* the trim parameter's `+0x19c` is not read during a page switch;
* the pad→page copy `0x280dca0` never fires on a pad or Sample press;
* writing the getter object's `+0x78`/`+0xd0` (the pair the handler reads for
  control ids `0x462`/`0x466`) **removes the page's markers** — those fields are
  not trim floats.

Retracted claim: an early round concluded that writing the property plus a
redraw moved the app's Start edge to 50 %. Re-tested with the current tooling it
does **not**; what was seen was the *End* marker moving because of the knob nudge
used to force the repaint (`TRIM-PAGE.md` §31.7).

**Next step that has the best chance:** with a fresh gdbstub, use LLDB to set a
conditional breakpoint on `fillRect` (`base + 0x102e000`) filtered to the
waveform band (`$x2 >= 86 && $x2 <= 188 && $x3 <= 8`), press a page key, and read
`bt` — that names the paint function. Then watch what that paint reads.

### 9.2 UI freeze regression (fixed — record it so it is not reintroduced)
The hardening pass cached `/proc/self/mem` with **`O_RDWR`**. On this kernel a
**read through an `O_RDWR` descriptor fails with `EIO`**, so every `rd()` returned
0, the mod's object walking broke, and the UI wedged while audio kept playing.
Fix: one descriptor per direction (`O_RDONLY` for reads, `O_WRONLY` for writes),
never `O_RDWR`. Bisect table is in `CODE-REVIEW.md` §H4.

### 9.3 Pad presses do not select / repaint (emulator vs hardware)
The panel sends pads as **channel-10 MIDI notes** (`99 {id} {vel}` in
`panel/controls.json`), not as a physical pad-matrix event. Measured on a
**factory-fresh VM with no mod at all**: `PAD 1..5` give 0 px, the header sample
name does not change, and it still does not change after forcing a full repaint.
So the note plays audio but never changes the selection.

The user's device behavior (pressing a pad switches the sample) therefore cannot
be reproduced through the panel as it stands. **To finish this**, the emulator's
pad path would have to emit whatever the hardware pad matrix emits (not a MIDI
note) — that is a `panel/`/`controls.json` change, not a mod change. Verify by
pressing a pad and checking whether the displayed sample path changes (the mod
logs it on every trim: `autotrim: sample=… path='…'`; it was identical for
`PAD 1`, `PAD 3`, `PAD 7`).

### 9.4 Property learning depends on a value change
If a knob's value never changes, the app never broadcasts, so the mod never
learns that property and falls back to `prop_of_param()` — a **full heap scan on
the UI thread**. It self-corrects as soon as the knob moves. Worth bounding or
replacing with a structural lookup.

### 9.5 Accepted / open review items
* M8 (compound state in `g_pm[]`) — accepted: 32-bit aligned stores are atomic
  on aarch64 and the pair is written once at learn time.
* L1–L5 from `CODE-REVIEW.md` are open: overlay targets a vtable slot that is an
  empty stub (no verification), raw hex instead of named constants for several
  hooks, `snprintf` truncation not checked, comment drift.

---

## 10. Repo map

```
mod/src/mpcmini.c        the mod (~4.4k lines)
mod/build.sh             build the .so
mod/pack.sh              build the SD autorun zip
mod/build/mpc-mini.zip   the deliverable
docs/MOD-REFERENCE.md    architecture + all RE findings
docs/CODE-REVIEW.md      review, fix status, regression bisect
docs/TRIM-PAGE.md        investigation log (§1–32)
docs/RE-GHIDRA.md        Ghidra/MCP setup
docs/re/                 decompiled reference functions
```

Git: tag `v1.3.0.12-mpc-mini.1`; private remote `https://github.com/ioma8/mpc-sample-mod`.
