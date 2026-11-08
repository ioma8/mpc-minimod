# MPC Sample mod — reference

Everything in this document was established in this workspace, on firmware
**1.3.0.12** only, against the shipped binary:

```
/usr/bin/MPC Sample
sha256 b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508
aarch64 PIE ELF, ~119 MB, stripped
```

Current release behavior and anchors are in sections 1–3. Section 4 is
historical research from before native AutoTrim; use
[`AUTOTRIM-NATIVE.md`](AUTOTRIM-NATIVE.md) for current AutoTrim behavior.
Anything not measured is marked as such.

---

## 1. What the mod is

An `LD_PRELOAD` shared object (`minimod.so`) for the stock MPC Sample app. The
release build ships eight features: pitch shifting in 16 Levels, native-command
AutoTrim with its caption, the SAMPLE OPTIONS menu (HPSS, Spectral Chop,
Add Reverb, Vinyl, Lo-Fi, Bass Synth), and external chromatic MIDI in 16 Levels.
User-facing documentation is in [`../README.md`](../README.md) and
[`../mod/README.md`](../mod/README.md); this file is the firmware-side reference.

| feature | trigger | current state |
|---|---|---|
| Pitch shift | PLUS / MINUS in 16 Levels; both reset | verified on hardware; keys are swallowed only in 16 Levels |
| External chromatic MIDI | select a pad, enable Pad MIDI In, turn on 16 Levels | native MIDI event hook; C3 root; verified on hardware |
| AutoTrim | SHIFT + B3 on the Sample page | native control path and marker refresh verified in emulator |
| AutoTrim caption | hold SHIFT on the Sample page | drawn in the paint hook; hidden in Chop mode |
| SAMPLE OPTIONS → HPSS | SHIFT + DATA, choose HPSS, DATA | harmonic + percussive float32 WAVs on the next two free pads; emulator-verified |
| SAMPLE OPTIONS → Spectral Chop | SHIFT + DATA, choose Spectral Chop, DATA | in-place chop, ≤16 slices on detected frames; emulator-verified |
| SAMPLE OPTIONS → Add Reverb / Vinyl / Lo-Fi | SHIFT + DATA, choose, DATA | native H3 render on a free pad, Reverb with an appended tail; emulator-verified |
| SAMPLE OPTIONS → Bass Synth | SHIFT + DATA, choose, DATA | DaisySP C2 pluck on the next free pad, K1–K3 shape it; emulator-verified |

AutoTrim uses the most recently triggered pad index (A01 before the first pad
note). Real hardware, save/reload, non-first layers, selection by other routes,
and sequence playback still need validation. See
[`AUTOTRIM-NATIVE.md`](AUTOTRIM-NATIVE.md).

## 2. How it works

### 2.1 Loading and hooking

1. The SD autorun script sets `LD_PRELOAD` for the app.
2. `dl_iterate_phdr` finds the app base; runtime addresses are `base + file_va`.
3. A relay and trampoline are allocated for each hook. The installer checks the
   target's PAC/BTI prologue, branch range, memory protection, and instruction
   cache before patching.
4. The autorun package checks the target app version. Do not manually preload
   this build against a different firmware.

### 2.2 Relevant firmware anchors (file VAs, 1.3.0.12)

| address | purpose |
|---|---|
| `0x159f2f0` | panel button dispatch; SHIFT, B3, 16 Levels, PLUS/MINUS |
| `0x1968020` | native control dispatcher used to write trim positions |
| `0x28100c0` | Sample page constructor probe (debug) |
| `0x2941f80` | sample frame-count getter |
| `0x1149880` | component repaint, used for toast clearing |
| `0x102e000` / `0x102d5e0` | Graphics fillRect / setColour |
| `0x10817b0` | Graphics drawText, used for the caption and toast |
| `0x21472f0` / `0x27e4400` | 16-Levels render / pitch-offset setter |

The native trim command uses a live dispatcher context and frame positions.
It sets Start to zero, End to `last + 1`, then Start to `first`. It does not
call `TrimParam_SetValue`, broadcast a guessed property, or draw trim edges.

### 2.3 AutoTrim analysis

The button hook detects a fresh B3 press while SHIFT is held and while Chop is
off. The sample is resolved from the last triggered pad through the surface pad
vector (`+0x2808/+0x2810`), pad state (`+0x22e8`), layer array, and sample
(`+0x4a8`). The WAV path is read from the sample object first, then located by a
pointer scan or by matching frame count in TempProjects.

The RIFF parser handles PCM 16/24/32-bit and IEEE float32, including extensible
headers. The default first-step threshold is 3% of peak (about −30.46 dB) with a
90-count absolute floor. `MINIMOD_TRIM_DB` overrides the relative threshold for
the first step. Successive B3 presses while SHIFT stays down use 3%, 6%, 9%,
then clear trim. Releasing SHIFT resets the cycle. Each step raises a one-second
toast. B3 repeats while held, so only its down edge is processed.

### 2.4 Paint hook

The SampleModeScreen paint hook calls the stock paint and draws the temporary
AutoTrim caption while SHIFT is held, plus the confirmation toast. It does not
draw trim edges; the native command updates the app's own waveform markers and
controls. The caption is hidden when the Chop group is visible.

## 3. Build and deploy

```sh
cd mod
./build.sh     # zig cc -target aarch64-linux-gnu -O2 -fno-omit-frame-pointer
./pack.sh      # -> mod/build/minimod.zip
```

The package contains `autorun/minimod.so`, `autorun.sh`, `autorun.check`,
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
printf "[Service]\nEnvironment=LD_PRELOAD=/run/mpclab-preload/minimod.so\n" \
  > /run/systemd/system/ac50.service.d/mpclab-preload.conf
systemctl daemon-reload; systemctl reset-failed ac50; systemctl restart ac50
```

Ports: panel `8865`, ssh `2122`, QMP `5654`, VNC `6010`.

---

## 4. Historical reverse-engineering findings (pre-native AutoTrim)

The property, trim-core, heap-scan, and overlay conclusions in this section
document the superseded implementation. They remain useful as investigation
history, but are not the current AutoTrim write or render path.

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

## 5. Superseded renderer findings (pre-native AutoTrim)

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

## 7. Historical verified measurements (pre-native AutoTrim)

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
| `mod/src/minimod.c` | the mod (~6 400 lines; ~1 700 of them are `#if MINIMOD_DEBUG` probe code that a release build does not compile) |
| `mod/build.sh`, `mod/pack.sh` | build and package |
| `mod/build/minimod.zip` | **the deliverable** |
| `docs/TRIM-PAGE.md` | the long-form investigation log (§1–32) |
| `docs/RE-GHIDRA.md` | Ghidra/MCP setup and the 12.2 rebuild |
| `docs/re/*.c` | decompiled reference functions |
| `HYPOTHESES.csv` | hypothesis log |
