# MPC Sample mod — reference

Everything in this document was established in this workspace, on firmware
**1.3.0.12** only, against the shipped binary:

```
/usr/bin/MPC Sample
sha256 b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508
aarch64 PIE ELF, ~119 MB, stripped
```

Anything not measured is marked as such. Sections 1–3 describe what ships;
sections 4–7 describe how it was found and what does *not* work.

---

## 1. What the mod is

An `LD_PRELOAD` shared library (`mpcmini.so`) that is loaded into the stock app by
an SD-card `autorun` script. It never modifies the firmware on disk — it patches
a handful of instructions in the app's memory at load time, the same technique
`am-mod` uses.

Features as shipped:

| feature | trigger | state |
|---|---|---|
| AutoTrim | SHIFT + B3 on the Sample/Trim page | **works**, verified in the emulator |
| Pitch shift | `+` / `-` in 16 Levels, PLUS+MINUS resets | works |
| Chromatic control | external MIDI | works |

The trim is computed, written into the app's own trim parameters *and* into its
property model (through the app's own setter, so listeners fire), and drawn on
the Trim page by the mod's overlay.

---

## 2. How it works

### 2.1 Loading and hooking

1. The SD autorun script sets `LD_PRELOAD=/path/mpcmini.so` for the app and
   restarts it.
2. On load, the library walks `dl_iterate_phdr` to find the app's load base
   (the `PT_LOAD` containing file offset `0xb55b00`). Runtime address of any
   symbol = `base + file_va`, with `base` typically `0xaaaaaaaa0000`.
3. Each hook site gets two anonymous pages: a **relay** (the patched entry
   jumps here) and a **trampoline** (the original first instruction plus a jump
   back). The entry instruction is replaced with `B <slot>`.
4. **The site must begin with `paciasp` or `bti`** — the installer refuses
   otherwise and logs `site %p has no paciasp/bti`. This is a hard constraint:
   functions with a plain prologue cannot be hooked with this scheme.
5. The library refuses to install anything if the binary digest does not match,
   so a wrong firmware means "no mod" rather than "crash".

### 2.2 The hook table (file VAs, 1.3.0.12)

| constant | file VA | what it is |
|---|---|---|
| `A_BUTTONS` | `0x159f2f0` | physical button dispatch — where SHIFT+B3 is detected |
| `A_TRIM_CORE` | `0x15e70c0` | `TrimParam_SetValue(value, param)` — writes the applied trim |
| `A_BROADCAST` | `0x13bb930` | `Property_Broadcast(prop, value)` — stores + notifies listeners |
| `A_REPAINT` | `0x1149880` | `Component::repaint(comp, x, y)` |
| `A_SAMPLE_FRAMES` | `0x2941f80` | sample frame-count getter |
| — | `0x102e000` | `fillRect(g, x, y, w, h)` (used by the overlay) |
| — | `0x102d5e0` | `setColour(g, rgba)` (used by the overlay) |

`A_BROADCAST` and `A_TRIM_CORE` are used both as hooks (to learn object
addresses) and as call targets.

### 2.3 AutoTrim pipeline

1. **Key detection.** The button dispatcher hook reads `code` at `event+8` and
   `down` at `event+9`. **SHIFT is code `0x31`, B3 is code `0x0e`.**
   SHIFT press/release sets `g_shift`; SHIFT + B3 calls `do_autotrim`.
   While SHIFT is held, the mod also rewrites the app's own **Filter tab label to
   "AutoTrim"** (`set_filter_text`) and restores it to `"Filter"` on release —
   that is the "AutoTrim" label the mod shows, using the app's own label widget
   rather than drawing text.
2. **Find the displayed sample.** `find_displayed_sample()` scans the Sample
   screen's child pointers (offsets `0..0x800`, step 8) for a child whose
   `+0x130` points at an info struct holding `frames` (`info+4`) and `rate`
   (`info+8`), then scans that child for a holder that yields a `.wav` path.
   The path is logged as `sample obj=… info=… frames=… path='…'`.
3. **Analyse the WAV.** `wav_info()` parses the RIFF header; `wav_edges()` scans
   the PCM data for the first and last frame above a threshold.
   The threshold is **peak-relative** (`MPCMINI_TRIM_DB`, default **−40 dB**,
   clamped to −90…−6) with an absolute fallback of `328` (−40 dBFS) — this is
   what makes low-level noise at the edges get trimmed, not just digital
   silence.
4. **Convert to normalised positions.** `v0 = first/frames`, `v1 = last/frames`.
5. **Write the app's parameters.** The values are pushed through
   `TrimParam_SetValue(value, param)` for both the Start and End parameters,
   so the audio side uses exactly what the knobs would produce.
6. **Write the app's model and notify.** `Property_Broadcast(prop, value)` is
   called for both properties. **It must be called instead of pre-writing
   `prop+0x50`** — see 4.4; the broadcaster early-outs when the stored value
   already equals the new one, which silently suppresses the notification.
7. **Draw it.** The overlay paints during the app's own paint pass.

Measured result on a real pad:

```
commit core done p0=0.0000 p1=0.8268 (want 0.0001/0.8254)
```

and the property read back through the debugger afterwards: `0x388a9e36` = `0.0001`.

### 2.4 The overlay

The mod takes the screen vtable's paint slot, keeps the original, and draws on
top of it with the app's own `setColour`/`fillRect`:

* grey `(27,27,30)` shading over the trimmed-away regions,
* white edge lines at `x = 10 + v * 295`, `y = 88`, height `98` (i.e. 88…186),
  plus a 3×1 tick at the top of each edge,
* the app's Filter tab label reading **"AutoTrim"** while SHIFT is held.

Because it draws **inside** the app's paint pass, it uses the real `Graphics`
object. That matters: the Graphics is a **stack temporary** — caching it and
trying to paint later is a dangling pointer, which is why every "force a
repaint from a hook" attempt failed (measured: 0 px, and the cached pointer was
`0xfffffffff648`, i.e. a stack address).

---

## 3. Build and deploy

```sh
cd mod
./build.sh     # zig cc -target aarch64-linux-gnu -O2 -fno-omit-frame-pointer
./pack.sh      # -> mod/build/mpc-mini.zip
```

The package contains `autorun/mpcmini.so`, `autorun.sh`, `autorun.check`,
`autorun.json` (~34 KB total). Copy it to the SD card root; the stock app runs
the autorun script on boot.

Emulator workflow used throughout (port offset 100):

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py panel            # web panel on :8865
MPCLAB_PORT_OFFSET=100 python3 lab.py preload <so>     # copy + install drop-in
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh '...'        # root shell
```

**Trap:** `lab.py preload` rewrites the systemd drop-in *without* extra
environment variables. Always rewrite the drop-in and restart afterwards:

```sh
printf "[Service]\nEnvironment=LD_PRELOAD=/run/mpclab-preload/mpcmini.so\n" \
  > /run/systemd/system/ac50.service.d/mpclab-preload.conf
systemctl daemon-reload; systemctl reset-failed ac50; systemctl restart ac50
```

Ports: panel `8865`, ssh `2122`, QMP `5654`, VNC `6010`.

---

## 4. Reverse-engineering findings

Address convention: **Ghidra VA = file VA + 0x100000**;
**runtime = 0xaaaaaaaa0000 + file VA**.

### 4.1 The property object

A value holder used by every knob:

| offset | meaning |
|---|---|
| `+0x10` | listener list head |
| `+0x18` | **version** (16-bit, incremented on every change) |
| `+0x20` | `pthread_mutex_t` |
| `+0x50` | **the value** (32-bit; floats are stored as their bit pattern) |

`Property_Broadcast(prop, value)` — `prop` in `x0`, **value in `s0`**:

```
14bb94c  mov  x20, x0          ; prop
14bb954  fmov w21, s0          ; value
14bb988  ldr  w0, [x20, #0x50]
14bb98c  cmp  w0, w21          ; equal -> return, notify nobody
14bb9f4  str  w21, [x20, #0x50]
```

Confirmed with a hardware write watchpoint on a live property:

```
Z2(0xffffe4ecc768) -> OK
stop: T05thread:01;watch:ffffe4ecc768;
WRITER pc=0xaaaaabe5b9f4   (file 0x13bb9f4 = Ghidra 0x14bb9f4)
```

### 4.2 The trim parameter

* `param + 0x19c` — applied value (what the audio side uses).
* The trim core `TrimParam_SetValue(value, param)` at file `0x15e70c0` writes it.

### 4.3 The control surface (knobs)

* `FUN_029c6510(surface, mode, controlId)` is the dispatch funnel; it calls the
  surface's virtual method at **`vtable + 0x160`**.
* Observed control ids with the debugger: **Start = `0x462`, End = `0x466`**
  (four apart, same knob group).
* The handler `FUN_02904e90` (file `0x2804e90`) is a **getter**: for the `0xc0x`
  ranges it reads `*(surface+0x22e8) + idx*0xb30` and converts ints with
  `/127.0` (`+0x1a8`, `+0x220`, `+0x298`).
* The value setter path is `FUN_01a68020`, whose register use is
  `x0 = arg0`, `x1 = surface`, `w2 = mode`, `w3 = controlId`, `s0 = float value`
  (from the disassembly at `0x1a68020`: `mov x19,x1` / `mov w20,w2` /
  `mov w21,w3` / `fmov s8,s0`, and the dispatch `(*(*x1 + 0x160))()`).

### 4.4 The bug that cost the most time

`commit_trim` originally did:

```c
wr(pr0 + 0x50, &v0, 4);   /* write the value directly      */
orig_bcast(pr0, v0);      /* broadcaster sees "unchanged"  */
```

Because `Property_Broadcast` early-outs when the stored value already equals the
new one, the notification — the part that tells listeners the value changed —
was **always suppressed**. Removing the direct writes (letting the broadcaster do
the store) is the fix, and it is verified: the property now reads `0.0001` after
a trim.

### 4.5 What the page object is *not*

Measured with a full memory snapshot through the debugger:

* a `0x2400`-byte snapshot of the page (vtable verified `0xaaaab1963ca0`) is
  **bit-identical** across a knob turn — zero changed bytes;
* the property is read **only** by `Property_Broadcast` itself (`0x14bb988`) on a
  knob turn — no listener and no paint-side code reads it;
* the trim parameter's `+0x19c` is not read during a page switch;
* the pad→page copy `FUN_0290dca0` (file `0x280dca0`) never fires on a pad or
  Sample press.

So the app's own trim widgets (waveform edges and the bottom Start/End bars) do
**not** re-render from any of these. This is the one open item.

---

## 5. The renderer limitation (honest statement)

The mod's overlay draws the trim; the **app's own** widgets do not move.

An early conclusion — that writing `prop+0x50` plus a redraw moved the app's Start
edge to 50 % — was **wrong** and is retracted in this workspace: re-tested with
the current tooling, writing the property and forcing a repaint leaves the Trim
page with no edge markers at all. What was seen in that early screenshot was the
**End** marker moving because of the knob nudge used to force the repaint.

Not identified, despite extensive work: the data the page renderer reads for its
Start/End. Candidates eliminated are listed in 4.5; the techniques used are in
section 6.

---

## 6. Debugging techniques developed here

### 6.1 Ghidra + MCP

`ghidra-mcp v7.0.0` (bridge `/Users/jakubkolcar/.local/bin/bridge-mcp-ghidra`,
backend `http://127.0.0.1:8089`) exposes ~216 tools over HTTP. The extension
**must be built for the exact Ghidra version**; the shipped 12.1.4 build failed
against Ghidra 12.2 with a `SleighLanguageDescription` error and was rebuilt from
source and installed to
`~/Library/ghidra/ghidra_12.2_PUBLIC/Extensions/GhidraMCP`.

Practical notes:

* two programs with the same name make every call ambiguous — import a uniquely
  named copy (`/tmp/knob.bin`);
* `force_decompile` is the fastest way to get C for a function; several functions
  decompile to nothing because they are stubs or jump tables.

### 6.2 ptrace tracers (in-guest)

Cross-compiled static tools with `zig cc -target aarch64-linux-musl`:

* `knobtrace` — attach, plant `BRK`, log `pc/x0..x3/lr`, single-step, re-arm;
* `st` — single-step and decode `STR/STP` immediates to log heap-destined
  stores.

**Hazard:** never breakpoint an address the mod has already hooked. The mod's
`B` instruction gets overwritten, and the restore wrote back a mismatched value,
corrupting the entry and crashing the app.

### 6.3 QEMU gdbstub (hardware watchpoints)

The guest CPU exposes **no debug feature** (`/proc/cpuinfo` has neither `debug`
nor `breakpoint`), so in-guest hardware watchpoints are impossible:
`PTRACE_SETREGSET` with `NT_ARM_HW_WATCH` fails with `ENOSPC` even though
`GETREGSET` reports slots.

QEMU's own gdbstub can be started on the **running** VM:

```
{"execute":"human-monitor-command","arguments":{"command-line":"gdbserver tcp::1234"}}
```

Two clients were written/used:

* a ~120-line RSP client speaking `Z2` (write), `Z3` (read), `Z4` (access),
  `m`/`M` (memory), `Z0` (software breakpoint) — **virtual** addresses work,
  QEMU translates them;
* **LLDB** on macOS, which provides what the raw client lacks:
  **conditional breakpoints**, e.g.
  `break set -a 0xaaaaabace000 -c "$x2 >= 86 && $x2 <= 188 && $x3 <= 8"`.

Operational lessons (all learned the hard way):

* connecting **stops the VM**; a killed client leaves it paused;
* `QMP {"execute":"cont"}` resumes it even when the stub is unresponsive — this
  is the reliable recovery;
* a client killed mid-session **wedges the stub** (later connects fail with
  `failed to get reply to handshake packet`), and QEMU refuses to create a second
  stub (`duplicate yank instance`) — only a VM restart clears it;
* memory reads/writes through the stub are **MMU-context dependent**: `m`/`M`
  can return `E14` unless the CPU is stopped inside the app, so use the mod's
  in-process `set`/`read` commands for anything decisive;
* breaking on a hot function without conditions is impractical: `fillRect`
  produced **64 870 stops in 40 s**.

### 6.4 Emulator specifics

* screendump via QMP `human-monitor-command` → `screendump /tmp/fb.ppm`
  (320×240 RGB);
* panel control via HTTP:
  `curl -X POST -d '{"name":"K1","action":"set","value":60}' http://127.0.0.1:8865/control`
  (also `press`/`release` for keys such as `SHIFT`, `SCREEN 1/2/3`, `PAD 1`,
  `SAMPLE`);
* the panel service must be running (`lab.py panel`) or no key reaches the app;
* `lab.py stop` may need 60 s and then quits QEMU, and the next boot uses a
  **fresh eMMC** (`/data` and `/factory` reset).

---

## 7. Verified measurements

| what | value |
|---|---|
| trim parameters after AutoTrim | `p0=0.0000 p1=0.8268` (knob-equivalent `0.0001/0.8254`) |
| property value read back | `0x388a9e36` = `0.0001` |
| overlay drawing | 384 marker pixels on the Trim page |
| app stability | pid unchanged across a trim |
| page size / object | `SamplePage` = `0x2468` bytes, vtable file `0x6ec3ca0` |
| property slot | `prop+0x50`, version `prop+0x18` |

---

## 8. Files

| path | contents |
|---|---|
| `mod/src/mpcmini.c` | the mod (~3 900 lines) |
| `mod/build.sh`, `mod/pack.sh` | build and package |
| `mod/build/mpc-mini.zip` | **the deliverable** |
| `docs/TRIM-PAGE.md` | the long-form investigation log (§1–32) |
| `docs/RE-GHIDRA.md` | Ghidra/MCP setup and the 12.2 rebuild |
| `docs/re/*.c` | decompiled reference functions |
| `HYPOTHESES.csv` | hypothesis log |
