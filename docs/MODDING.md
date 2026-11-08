# Modding the MPC Sample

How custom code gets into the stock app: the boot chain, the autorun protocol and
its digest, the `LD_PRELOAD` + branch-hook technique this mod uses, packaging,
and what the surrounding mod ecosystem does. Firmware **1.3.0.12** only.

The mechanisms this technique exposes are in [`MECHANISMS.md`](MECHANISMS.md);
the safety rules in [`RE-METHOD.md`](RE-METHOD.md) §5.

---

## 1. The stock boot chain [VALIDATED, from the rootfs]

```
systemd multi-user.target
  └─ ac50.service                    /usr/lib/systemd/system/ac50.service
       Wants=az01-script-runner.service   (After=)
       After=az0x-usb-gadget@ac50.service
       ExecStart=/usr/bin/launch-ac50
       Restart=always
```

`/usr/bin/launch-ac50` (489-byte shell script):

```sh
#!/bin/sh
FIRMWARE=/usr/share/Akai/AC50/Firmware/AC50/UpdateImage.rbin
/usr/bin/az0x-firmware-tool "$FIRMWARE"
ulimit -S -s 1024            # 1 MiB stack: thread stacks are preallocated + locked
export MALLOC_ARENA_MAX=1
exec setarch -R -- "/usr/bin/MPC Sample" "$@"
```

Three details make modding practical:

* **`setarch -R` disables ASLR** — the app always maps at the same addresses, and
  the stock launcher already does it. Fixed-address patching depends on this.
* The **1 MiB stack** and `MALLOC_ARENA_MAX=1` are set here, not in the app; a
  mod that spawns threads inherits both.
* `az01-script-runner.service` is a separate service, **not** the autorun
  handler (its strings never mention autorun).

---

## 2. The autorun protocol

Autorun is a feature of the **application**, not a daemon.

**[VALIDATED]** The string table is in `/usr/bin/MPC Sample` at file offset
`0x45c43b8`:

```
scripts  files  askQuestion  yesButtonText  noButtonText
.json    .check  autorun
"Script detected on %s - Do you want to run it?"
"About to run script: %s"
sh "%s"
```

The handler sits around `0x2416cc0`–`0x2417f00`: it builds `<folder>.json` and
`<folder>.check`, reads both, and only then emits `Script detected on …` and runs
the script via `sh "<script>"`.

### 2.1 Placement

* Folder named exactly **`autorun`**, directly in the **root** of the SD card.
* Card must be **exFAT**.
* The card is mounted **`noexec`**: payloads are copied to `/run` (tmpfs) and
  executed there. Everything is RAM-only; a reboot is stock.
* Renaming or moving the folder disables it; booting without the card is stock.

### 2.2 `autorun.json`

**[VALIDATED]** Schema, from the shipped mod:

```jsonc
{
  "value0": {
    "scripts": [
      {
        "path": "autorun.sh",
        "askQuestion": false,
        "message": "Start minimod?",
        "yesButtonText": "Start",
        "noButtonText": "Cancel"
      }
    ],
    "files": []
  }
}
```

* `scripts[]` — shell scripts to offer or run, with an optional confirmation
  prompt and button labels. `askQuestion: false` runs silently, which is how the
  mod starts.
* `files[]` — extra files the manifest declares (empty here).
* Paths are relative to the `autorun` folder.
* The `{"value0": …}` wrapper is the same serialisation convention as the `.xpj`
  project format (see [`../converter/docs/FORMAT.md`](../converter/docs/FORMAT.md) §2).

### 2.3 `autorun.check` — the gate is solved

**[VALIDATED]** A 64-character lowercase hex digest with no trailing newline:

```
sha256( bytes(autorun.json) || bytes(autorun.sh) || "f7bed1c9-7e90-4731-9194-c996666b4331" )
```

The two files followed by a fixed 36-byte salt. This reproduces the shipped
SampQOL package exactly and matches `mpc-ui-toolkit/tools/pack-autorun.sh`;
[`../mod/pack.sh`](../mod/pack.sh) regenerates it. The salt is why the earlier
"no obvious construction matches" result was a dead end — none of those attempts
included it. **Changing either file requires recomputing the check; nothing else
about the folder is covered.** The app builds the digest itself (it links no
OpenSSL).

### 2.4 What the shipped minimod script does

```sh
#!/bin/sh
DIR="$(cd "$(dirname "$0")" && pwd)"
STOCK="/usr/bin/MPC Sample"
FW=44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f   # first 4 KiB
[ "$(dd if="$STOCK" bs=4096 count=1 2>/dev/null | sha256sum | cut -d' ' -f1)" = "$FW" ] || exit 0
cp "$DIR/minimod.so" /run/minimod.so || exit 0
chmod 755 /run/minimod.so
mkdir -p /run/systemd/system/ac50.service.d
printf '[Service]\nEnvironment=LD_PRELOAD=/run/minimod.so\nEnvironment=MINIMOD_DEBUG=0\n' \
    > /run/systemd/system/ac50.service.d/minimod.conf
systemctl daemon-reload
systemctl restart ac50
```

Nothing on disk changes; removing the folder and rebooting returns to stock. The
first-4-KiB check is a cheap version guard, not a security boundary — the
`lab.py preload` route (`LD_PRELOAD` by hand) has no such guard and would crash
the app on another build.

**Known interoperability gap.** `Environment=LD_PRELOAD=` *replaces* the
variable, so this script's fixed list drops any preload the firmware or another
addin set (the ecosystem's shared drop-in is `90-mpc-addins.conf`, which sorts
before ours and therefore loses). The community installer composes the effective
list instead of replacing it; see
[`ADDIN-ECOSYSTEM.md`](ADDIN-ECOSYSTEM.md#the-shared-installer-contract).

---

## 3. The hooking technique

An `LD_PRELOAD` aarch64 shared object; the constructor resolves the app base with
`dl_iterate_phdr` (the PIE base is constant because ASLR is off). Runtime address
= `base + mod offset`.

### 3.1 Relay and trampoline

* Two pages are mapped **within ±128 MB of `base`** (`MAP_FIXED_NOREPLACE`,
  scanned from `base+0x7900000`): one for the branch relay slots, one for
  trampolines. A `B` has ±128 MB range, and a slot must be reachable from its
  target.
* Each slot is 32 bytes: `ldr x16, #8 ; br x16 ; .quad <hook>`. The site's first
  instruction is replaced with `B <slot>`.
* The trampoline replays the displaced prologue (up to 4 instructions) and then
  branches back to `entry + n*4`. The copier **stops before any PC-relative
  instruction**; if the very first instruction is PC-relative the site is
  refused rather than guessed.
* The site's first instruction word and branch range are validated; the patched
  page is `mprotect`ed `R|X` afterwards and the instruction cache is cleared.
* `install_hook()` returns 0 on refusal. A refused hook is a missing feature,
  never a half-patched function.

### 3.2 Calling app code

`FN(offset, type)` casts `base + offset` to a function pointer; `rd`/`wr` read
and write target memory through `/proc/self/mem`. Hooks that need the original
behaviour keep the trampoline (`orig_*`) and call it.

### 3.3 What the hooks must check

* the **firmware hash** (autorun script) and the **prologue bytes** at each site;
* the **screen vtable** before any model access (`base + 0x6e212a0`);
* the **sample identity** before committing a transform (pointer and frame
  count), and the **chop type** after setting it;
* the **first bytes of the lazy MIDI hook target** before swapping it in.

A mismatch must turn into "no mod" or an explicit status, never a wrong write.

### 3.4 Threads

Input hooks only publish requests. Model work happens in the paint hook, one
phase per pass. DSP and file I/O run on a worker. The audio-thread hook copies
the planar block, mixes into private scratch, and calls the original writer. See
[`MECHANISMS.md`](MECHANISMS.md#14-threads-and-ownership).

---

## 4. Build and package

```sh
cd mod
./build.sh      # build/minimod.so  (zig cc -target aarch64-linux-gnu, -O2)
./pack.sh       # build/minimod.zip (autorun package with a valid autorun.check)
```

`build.sh` compiles `minimod.c`, `hpss.c`, `spectral.c`, `h3m_effect.c`,
`bass_sample.cpp` (DaisySP subset vendored under `mod/third_party/daisysp`), then
strips the `.so` with `llvm-objcopy --strip-all` **without shifting `g_screen`**
(the emulator tests read that symbol through `build/minimod.map`).
`fmbass_sample.c` is a compiled-out probe and is never linked.

`pack.sh` stages `autorun/{autorun.json, autorun.sh, minimod.so}`, recomputes
`autorun.check` with the salt, and zips the folder. That zip is the release
artifact.

The build is warning-free in the shipped configuration; probe code lives behind
`#if MINIMOD_DEBUG` / `#if MINIMOD_LOG` and is not in the release binary.

---

## 5. Constraints and risks

| constraint | detail |
|---|---|
| firmware lock | first 4 KiB of the app pinned to 1.3.0.12 (`44507c7a…`); any other firmware needs every patch site re-derived — the offsets are absolute |
| no flash | the mod exists only in RAM; power off restores stock |
| card format | exFAT, folder named exactly `autorun` at the card root |
| autorun gate | solved: `sha256(json + sh + salt)`; recompute on every change |
| noexec card | payloads must run from `/run` (tmpfs) |
| SSH is not a shortcut | `sshd` exists but login goes through `/usr/bin/az01-ssh-login`, a `ForceCommand` wrapper validating a product string from the key certificate; `debug-shell.service` only fires on a failed boot |
| ASLR | already disabled by the stock launcher, so fixed addresses are stable |
| hardware validation | emulator-verified is recorded as such; only pitch and external MIDI chromatic are hardware-verified |

---

## 6. The ecosystem

Studied to decide what a *minimal* mod should do. Sources: `mpc-boot-thingy`,
`SampQOL-OS` v0.1.5, `am-mod` r26/r27, `mpc-ui-toolkit`, `mpc-furnace`,
`mpc-cavestory`, `mpc-gen1recomp`, `mpc-tic80`, and the virtual lab. The 2026
addin wave (draw handler, remote, scale mode, rack) and its shared installer are
documented separately in [`ADDIN-ECOSYSTEM.md`](ADDIN-ECOSYSTEM.md).

### 6.1 mpc-boot-thingy — a multi-app launcher

Not a firmware patch: an `autorun` package that **replaces "one mod at a time"
with a menu**. It takes the panel UART and the screen while it runs and hands
them to the app it launches.

```
/autorun/
  autorun.json, autorun.sh, autorun.check     <- boot-thingy itself
  system/mpc-boot-thingy                      <- the menu binary
  system/boot-default                         <- saved default id
  system/patches/, patch-catalog/             <- Boot Edit binary patch sets
  apps/<name>/boot.entry                      <- an app
  apps/<name>/autorun/…                       <- or a nested autorun package
```

An app is either an ELF named by `boot.entry` (launcher stops `ac50.service`,
releases panel/screen, `fork/exec`s it with cwd = its folder, waits, reopens the
menu), or a nested autorun package run through
`systemd-run --unit=mpc-boot-autorun`. If any of its `.sh` files mention `ac50`
or `LD_PRELOAD` in the first 64 KB it is treated as *replacing* the stock app;
otherwise stock restarts afterwards. `boot.entry` is `title=`, `run=`, `args=`,
`boot=`; Boot Edit patches a copy of the app in RAM.

### 6.2 SampQOL OS — the full-featured reference

Layers, none of which touches flash:

* `qol-run.sh` started detached from `autorun.sh` (pid in `/run/qol-run.pid`),
  copies the payload to `/run`, stops `ac50.service`, restarts it with
  `LD_PRELOAD` and keeps a supervisor alive;
* `mpcqol.so` (880 KB, named `.dynsym` entries) patched in;
* features: step edit, chords, arp, mute, EQ, compressor, **Export Stems**
  (one WAV per track, 24-bit/44.1 kHz, submix effects, never master FX) and
  **Explode** (Step Edit → Track View → `SHIFT`+`B3` moves each pad to its own
  track, except pads sharing a mute group).

It was also the MIDI oracle: its log named the hooks (`keys-evin 0x211b090`,
`keys-nrsend 0x211a690`, `keys-prognote 0x286fcf0`) and measured chromatic
playback at correlation 0.997.

### 6.3 am-mod — the overlay technique

The technique that mattered: it **replaces the screen's paint vtable slot** and
draws over the stock painting with the app's own `juce::Graphics` entry points,
rather than renaming stock widgets.

```c
uintptr_t *vt = (uintptr_t *)(base + SCREEN_VT);
vt[27] = (uintptr_t)paint;      /* Component::paint(Graphics&), byte +0xD8 */
```

The vtable lives in `.data.rel.ro`, so the page must be `mprotect`ed writable for
the swap and read-only afterwards; keep the original slot and call it first to
draw on top of the stock UI. Useful entry points (same offsets in 1.3.0.12):

| offset | call |
|---|---|
| `0x102d5e0` | `Graphics::setColour(uint32 argb)` |
| `0x102e000` | `Graphics::fillRect(int x, int y, int w, int h)` |
| `0x1082810` | `Font::Font(int typeface, float height)` |
| `0x102dca0` | `Graphics::setFont(const Font&)` |
| `0x10817b0` | `Graphics::drawText(String&, x, y, w, h, justification, ellipses, minScale)` |
| `0xf535c0` | `juce::String::operator=(const char*)` |
| `0x1070d60` / `0xf0a290` | `Font` / `String` destructors |

am-mod identifies screens by vtable comparison (`*(uintptr_t*)p == base +
SCREEN_VT`), walks `Component+0x18` (parent) to find the owning screen, and keeps
a "ticket" (epoch + shift bit) so a queued control movement cannot land after the
screen changed. minimod uses the same paint-slot swap for the AutoTrim caption
and the SAMPLE OPTIONS overlay.

### 6.4 The rest

* **mpc-ui-toolkit** — UI components for other mods; its `pack-autorun.sh` is the
  second implementation that confirms the check formula.
* **mpc-furnace, mpc-cavestory, mpc-gen1recomp, mpc-tic80** — apps that boot
  through boot-thingy rather than patching the stock app.
* **virtual-mpc-sample** — the QEMU lab this project uses; it also documents how
  a library can be injected without an autorun package (`lab.py preload`).

### 6.5 What that means for a minimal mod

1. Ship one `autorun` package — nothing else.
2. Pin the firmware, verify it, stop `ac50`, restart it with `LD_PRELOAD`.
3. Do everything in the preload, with base-relative offsets and verified
   prologues; keep the stock file and all of `/usr` untouched.
4. Keep a log under `/run` (debug builds only) and make every hook refuse rather
   than guess when the bytes do not match.
5. Prefer the app's own commands and APIs to field writes; the app then keeps
   ownership of files, history and repaints.
