# The MPC Sample mod ecosystem — how custom firmware actually boots

Study of the community custom-firmware/app ecosystem for the Akai MPC Sample
(firmware 1.3.0.12, product `AC50`), done to build `minimod`. Everything here
was read from the sources or reproduced in the virtual device; tags are
**[VALIDATED]** (ran/read it) and **[DOC]** (author's own claim).

Sources studied: `mpc-boot-thingy`, `SampQOL-OS` v0.1.5, `am-mod` r26/r27,
`mpc-ui-toolkit`, `mpc-furnace`, `mpc-cavestory`, `mpc-gen1recomp`, `mpc-tic80`,
and the `virtual-mpc-sample` emulator.

---

## 1. The one gate: autorun

The stock app runs scripts from an SD card folder named exactly `autorun` at the
card root. There is no `autoboot` folder in the firmware; "autoboot" in
boot-thingy's docs is just the name it uses for **whatever `autorun` package was
there before it**.

Layout:

```
/autorun/
  autorun.json     which script, and whether to ask
  autorun.sh       the script (runs as root)
  autorun.check    64 hex chars
  ...              payload
```

`autorun.json` **[VALIDATED]**:

```json
{"value0":{"scripts":[{"path":"autorun.sh","askQuestion":false,
  "message":"Start my mod?\nyesButtonText":"Start","noButtonText":"Cancel"}],
  "files":[]}}
```

`askQuestion:false` runs silently at power-on.

### 1.1 `autorun.check` — the gate is solved

**[VALIDATED]** the digest is

```
sha256( bytes(autorun.json) || bytes(autorun.sh) || "f7bed1c9-7e90-4731-9194-c996666b4331" )
```

verified against the shipped SampQOL package (`5e2803df…` reproduces exactly)
and against `mpc-ui-toolkit/tools/pack-autorun.sh`. So the older "unproven
gate" in `docs/MODDING.md` §2.3/§3 is closed: **any** script can be booted,
the folder just needs a matching check. Changing either file requires
recomputing it.

### 1.2 What a package may do

**[VALIDATED]** the script runs as root before/around the app; the card is
mounted `noexec`, so payloads are copied to `/run` (tmpfs) and executed there.
Everything is RAM-only: a reboot is stock.

---

## 2. mpc-boot-thingy — the multi-app launcher

Not a firmware patch. It is itself an `autorun` package that **replaces the
"one mod at a time" model with a menu**. It takes over the panel UART and the
screen while it runs, and hands them to the app it launches.

### 2.1 Install and layout **[VALIDATED]**

`setup.sh --SD <mount>` (or `--usb` / `--ssh --add pkg.zip`) writes only inside
`autorun/` and preserves:

```
/autorun/
  autorun.json, autorun.sh, autorun.check     ← boot-thingy itself
  system/mpc-boot-thingy                      ← the menu binary
  system/boot-default                         ← saved default id
  system/patches/, patch-catalog/             ← Boot Edit binary patch sets
  boot-menu/*.entry                           ← legacy menu rows
  apps/<name>/boot.entry                      ← an app
  apps/<name>/autorun/…                       ← or a nested autorun package
```

An app is either:

* an **ELF** named by `boot.entry` — launcher stops `ac50.service`, releases the
  panel/screen, `fork/exec`s it with cwd = its folder, waits, reopens the menu; or
* a **nested autorun package** (`apps/<name>/autorun/…`) — run through
  `systemd-run --unit=mpc-boot-autorun`; if any of its `.sh` files mention
  `ac50` or `LD_PRELOAD` in the first 64 KB it is treated as *replacing* the
  stock app, otherwise stock is restarted afterwards.

`boot.entry` **[VALIDATED]**:

```
title=My app
run=my-binary          # relative to the folder, or absolute
args=--foo             # optional, split on spaces
boot=ac50              # optional: this is a Boot Edit (patch) entry
patches=id1,id2        # with boot=ac50
```

Menu order is the folder name (`10-game` before `20-tool`); Stock is always
first; `system/boot-default` holds the id to auto-start.

### 2.2 Boot Edit = binary patches, not code

`patch-catalog/*.list` entries are `#target-sha256`, `#result-sha256` and
`offset base64(old) base64(new)` lines. At boot the launcher bind-mounts a
wrapper over `/usr/bin/launch-ac50` that preloads `loader.so` with
`MPC_PATCH_DIR`; the loader applies the patches **in memory** to the mapped
image (the file on disk is untouched). See `menu-patch/apply.sh`.

That is neat for byte-level tweaks (examples shipped: Shift+Pad 16 menu row,
a Reboot action) but it cannot express a new feature — real logic needs a
library. Which is what the big mods do.

---

## 3. The two real mods

### 3.1 SampQOL-OS (closed source, freeware) **[DOC]**

The template for "big mod". Its `autorun.sh` verifies its own payload pins, then
`qol-run.sh`:

1. pins firmware 1.3.0 by hashing the **first 4096 bytes** of `/usr/bin/MPC Sample`;
2. `systemctl stop ac50.service`, `killall "MPC Sample"`;
3. paints a splash to the framebuffer;
4. relaunches stock with `LD_PRELOAD=/tmp/mpcqol.so` under `ulimit -S -s 1024`,
   `MALLOC_ARENA_MAX=1`, `setarch -R` (ASLR already off);
5. supervises: two exits under 120 s → back to stock.

`mpcqol.so` is an **in-memory inline hook installer**: it finds its own base in
`/proc/self/maps`, parses `.text`/`.got`/`.rela.plt`, `mprotect`s pages,
**verifies the expected opcode**, and writes a `BL` to a veneer within ±128 MB.
It resolves app internals through a pointer table at `0x171d000 + {0x3c0…0x420}`
and never reimplements the renderer.

### 3.2 am-mod (open source) **[VALIDATED]**

The best-documented reference and the one our mod follows.

* SD `autorun` (prompt on), same `autorun.check` salt.
* Copies the stock binary to tmpfs, applies **static byte patches** to that copy
  (including growing the RX `PT_LOAD` at `0xD0`), then bind-mounts the copy over
  `/usr/bin/MPC Sample` in a private mount namespace and runs it with
  `LD_PRELOAD=phone-input.so` under `setarch -R`.
* Its preload installs **runtime BL hooks** (`sd-mod-session/src/takeover_endpoints.c`):
  keep `PACIASP` at the entry, replace `entry+4` with a `B` to a relay page
  mmapped **near the app base** (the branch range is ±128 MB), relay does
  `AUTIASP` then jumps to a C wrapper, and the wrapper calls a trampoline that
  replays the displaced instruction and branches back to `entry+8`.
* Base discovery: `dl_iterate_phdr`, pick the `PT_LOAD` containing a known VA.
  No `dlsym`, no ptrace.
* Pins: stock SHA-256, patch SHA, per-payload hashes, bridge magic, every
  `{entry,trampoline}` pair; any mismatch aborts **before** stock is stopped.

Address map it publishes (base-relative, our firmware) that matters here:

| address | meaning |
| --- | --- |
| `0x159ed30` | pad event dispatch |
| `0x159f2f0` | physical button dispatch (am-mod's `H_BUTTONS`) |
| `0x15be5e0` / `0x15be740` | OnPlus / OnMinus actions (name-editor path) |
| `0x1e27360` / `0x1e270d0` | pad note-on `(input,src,pad,props,vel)` / note-off |
| `0x21472f0` | 16-Levels processor `(props,pad,event)`; type `props+0x88` (1=Tune), enable `props+0x2f8&1`, original pad `props+0x228` |
| `0x27e4400` | packed note-field setter `(event+0x14,0,0,pitch)` |
| `0x27e54d0` / `0x211b090` | note-event factory / engine send-event |
| `0x1545a50` | `SixteenLevelLighting` ctor `(self,a,b,properties)` |
| `0x1545930` | `SixteenLevelLighting::RefreshPad` |
| `0x15e70c0` / `0x14d0890` | TrimParameter core / trim boundary write |
| `0x2941ff0` (+0x60 begin, +0x68 end) | slice/PCM data |
| `0x2803b90` | layer → shared sample |
| `0x286bbe0` | Program → Instrument |

---

## 4. The apps (for the record)

All five garrettjwilke apps are **independent programs**, not patches: a native
aarch64 binary plus a nested `autorun` package produced by
`mpc-ui-toolkit/tools/pack-autorun.sh`. They stop `ac50.service`, take the
panel and screen, and restart stock on exit.

* `mpc-ui-toolkit` — C platform layer: DRM (`/dev/dri/card0`, **not** `/dev/fb0`)
  with `DIRTYFB` diffs (~50 ms/full frame), rawmidi panel protocol, LED SysEx,
  file browser, widgets. **This is the way to build a *replacement* app.**
* `mpc-furnace` (tracker), `mpc-cavestory` (doukutsu-rs), `mpc-gen1recomp`
  (peanut_gb), `mpc-tic80` — the same contract, heavier stacks (CMake/Rust).

Note for MIDI work: the stock app's external MIDI goes through
**`/lib/librbinmidi.so`** and its ALSA sequencer ports; the app owns its raw MIDI
devices, so a mod must not open another competing raw-device reader. For
chromatic playback, the mod hooks the downstream native note dispatcher at
`0x211b090` and applies pitch with the firmware setter at `0x27e4400`. Details
and emulator evidence: [`MIDI-CHROMATIC.md`](MIDI-CHROMATIC.md).

A **mod** (patches the stock app and keeps it) and an **app** (replaces the
screen) are different products. `minimod` is a mod, because the three wanted
features are edits to stock behaviour, not a new UI.

---

## 5. The emulator (our test rig)

`~/projects/customs/virtual-mpc-sample` runs the **stock** rootfs in QEMU
(dm-verity checked) with the panel in a browser.

| thing | value |
| --- | --- |
| start | `python3 lab.py run` |
| panel | <http://127.0.0.1:8765/> (`/state`, `/screen.png`, `POST /control`) |
| root shell | `python3 lab.py ssh` |
| load a library | `python3 lab.py preload lib.so` |
| run a package | `python3 lab.py autorun pkg.zip` |
| ports | 5551–5557, 5910, 2022, 2222, 8765 |

The `/control` API takes `{"name":"PAD 1","action":"press","value":110}`;
pads need a `value` (velocity) or nothing sounds. `PLUS`/`MINUS`/`16 LEVELS`
are plain buttons. Screenshots are 320×240 PNGs from `/screen.png`.

**Autorun runner caveat:** in this checkout the lab's `lab.py autorun` reports
"the runner did not pick the package up" for *both* `minimod.zip` and the
reference `SampQOL-OS-v0.1.5.zip`, cold boot **and** `--no-reboot` (the card is
mounted, the app even announces the script question, but nothing executes).
Since the reference package behaves identically, the package format is not the
cause — the emulator's autorun path needs investigation (card mount racing the
runner, or a missing runner unit). Library injection (`lab.py preload`) is the
working development loop. Library injection (`lab.py preload`) works and is the loop used for
development.

**Port collision**: the lab's default QMP ports 5554/5555 clash with an Android
emulator. `lab.py` now accepts `MPCLAB_PORT_OFFSET=100` (a 4-line addition in the
sibling checkout; all lab ports shift, children inherit it).

---

## 6. What that means for a minimal mod

1. Ship one `autorun` package (or a boot-thingy app folder) — nothing else.
2. Pin the firmware, verify, stop `ac50`, start it again with `LD_PRELOAD`.
3. Do everything in the preload with base-relative offsets and verified
   prologues; keep the stock file and all of `/usr` untouched.
4. Keep a log under `/run`, and make every hook refuse (rather than guess) when
   the bytes do not match.
