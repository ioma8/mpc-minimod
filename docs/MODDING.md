# MPC Sample firmware modding: findings

Everything learned about running custom code on the MPC Sample from its SD card,
from the community mod **SampQOL-OS v0.1.4.9** and from the stock firmware image
shipped in the 1.3.0 updater.

Tags: **[VALIDATED]** read directly from the mod files or the firmware;
**[INFERRED]** follows from the code but was never executed on hardware.

---

## 1. The stock boot chain

**[VALIDATED]** From the rootfs (`ext4` inside the updater's `AZ0x` image):

```
systemd multi-user.target
  └─ ac50.service                         usr/lib/systemd/system/ac50.service
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

Two details make modding practical:

* **`setarch -R` disables ASLR**, so the app always maps at the same addresses.
  Fixed-address patching depends on this, and the stock launcher already does it.
* the 1 MiB stack and `MALLOC_ARENA_MAX=1` are set here rather than in the app —
  a mod that spawns threads inherits both.

`az01-script-runner.service` (`/usr/bin/az01-script-runner`) is a separate
service; it is **not** the autorun handler (its strings never mention autorun).

---

## 2. The autorun protocol

Autorun is a feature of the **application**, not a daemon.

**[VALIDATED]** The string table lives in `/usr/bin/MPC Sample` at file offset
`0x45c43b8`:

```
scripts  files  askQuestion  yesButtonText  noButtonText
.json    .check  autorun
"Script detected on %s - Do you want to run it?"
"About to run script: %s"
sh "%s"
```

**[VALIDATED]** The handler code sits around `0x2416cc0`–`0x2417f00`. It builds
`<folder>.json` and `<folder>.check` (`adrp/add` against `0x45c43f8` and
`0x45c4400`), reads both, and only then emits `Script detected on …` and runs the
script via `sh "<script>"`.

### 2.1 Placement

* Folder named exactly **`autorun`**, directly in the **root** of the SD card.
* Card must be **exFAT**.
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
        "message": "Start SampQOL OS?\nB1 = Start, B3 = Cancel",
        "yesButtonText": "Start",
        "noButtonText": "Cancel"
      }
    ],
    "files": []
  }
}
```

* `scripts[]` — shell scripts to offer or run, with an optional confirmation
  prompt and button labels. `askQuestion: false` runs without asking, which is
  how the mod starts silently.
* `files[]` — extra files the manifest declares (empty in this mod).
* Paths are relative to the `autorun` folder.
* Note the `{"value0": ...}` wrapper — the same serialisation convention as the
  `.xpj` project format (see `docs/FORMAT.md` §2).

### 2.3 `autorun.check`

**[VALIDATED]** A 64-character lowercase hex digest with no trailing newline:

```
304a89b27a79b49f51e006349c21e150267bd7a4304817124ed222233e45c1cd
```

**[DISPROVED]** It is not a plain hash of the obvious inputs. Fifteen
constructions were tried and none matched: `sha256` of `autorun.json` (raw,
stripped, re-serialised compact, sorted keys), of `autorun.sh`, double-hashing
either, concatenating the folder's file hashes, or `sha256sum`-formatted text.
The app links no OpenSSL, so it carries its own digest implementation.

**[INFERRED]** The device hashes the manifest (or the whole folder) and compares
it with `autorun.check`. The manual is explicit: *"the MPC checks them and
ignores a changed folder."*

### 2.4 What the shipped script does about it

`autorun.sh` is self-verifying — it pins its own payload:

```sh
PINS="<sha256>  qol-run.sh
<sha256>  qol-snap.sh
<sha256>  mpcqol.so
<sha256>  qol-splash.raw"
(cd "$DIR" && echo "$PINS" | sha256sum -c -s) || exit 0
```

So there are **two** integrity layers: the device's `autorun.check`, and the
script's own `sha256sum -c` on its payload.

---

## 3. The gate: can we run our own code?

To run our own script we must either

* **A.** pass the device's check with our own files — needs the `autorun.check`
  algorithm, or
* **B.** change only files the check does not cover.

**[INFERRED] B is plausible and costs one experiment.** The manifest names the
script by path; `.check` sits next to `.json`. If the device only verifies the
**manifest**, then:

1. keep `autorun.json` **byte-identical** (it already says
   `"path": "autorun.sh"`, `askQuestion: false`);
2. keep `autorun.check` untouched;
3. replace only `autorun.sh`.

The manifest still validates, and our script runs.

**The experiment** (one boot on hardware): replace `autorun.sh` with

```sh
#!/bin/sh
DIR="$(cd "$(dirname "$0")" && pwd)"
echo "$(date '+%F %T') custom autorun ran" >> "$DIR/custom.log"
```

Boot with the card, then look for `custom.log`. If it appears, arbitrary code
execution is available with no forgery. If not, the check covers the whole
folder and the next step is decompiling the comparison at `0x2416cc0`.

**This was never run** — it needs the device.

---

## 4. How the SampQOL mod works

Three layers, none of which touches flash. Remove the card and the device is
stock.

### 4.1 Launcher and supervisor

`qol-run.sh` (started detached from `autorun.sh`, pid in `/run/qol-run.pid`):

1. pins the firmware by hashing the **first 4096 bytes** of `/usr/bin/MPC Sample`
   against
   `44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f`;
2. `systemctl stop ac50.service`, `killall "MPC Sample"`;
3. paints a splash by writing `qol-splash.raw` straight to `/dev/fb0`;
4. relaunches the stock app with the mod injected:

```sh
( ulimit -S -s 1024
  export MALLOC_ARENA_MAX=1
  export LD_PRELOAD="$SO"
  setarch -R -- "$STOCK" )
```

5. supervises it: two exits inside 120 s each → fall back to stock
   (`systemctl start ac50.service`). Exit codes go to `autorun/logs/events.log`.

### 4.2 The payload

`mpcqol.so` — 828 KB, stripped ARM64 shared object, **625 exported functions**
with descriptive names (`trkname_shift`, `overlay_update`, `newfolder_label`,
`eq_current_params`, `faderstep_fader_on`, `submix_init`, …). Imports only
`libc`, `libm`, `libpthread`, `libdl` — no Qt, no app symbols.

### 4.3 The patching technique

**[VALIDATED]** from its own log strings, it is an **in-memory inline hook
installer**:

```
.text  .got  .got.plt  .rela.plt
/proc/self/maps  /proc/self/statm  /proc/self/task/%ld/comm
mprotect
"%s: 0x%lx is 0x%08x, not 0x%08x -- not patched"
"%s: 0x%lx not the expected BL (0x%08x) -- hook off"
"%s: call hook installed site=0x%lx veneer=0x%lx"
"%s: capture installed target=0x%lx veneer=0x%lx"
"%s: wrap installed target=0x%lx veneer=0x%lx"
"%s: veneer out of branch range"
"%s: 0x%lx already wrapped by this handler -- skip"
```

The loop:

1. find its own load base via `/proc/self/maps`;
2. parse the ELF (`.text`, `.got`, `.rela.plt`) to resolve patch sites;
3. `mprotect` the pages writable;
4. **verify the expected opcode** before writing anything;
5. write a `BL` to a **veneer** allocated within ±128 MB of the target
   (AArch64 `BL` range), and the veneer jumps to the mod;
6. "wrap" hooks call the original through the veneer, so stock behaviour can be
   preserved and extended.

This is why the mod is firmware-locked: every site is a fixed offset plus an
expected opcode, and it refuses to patch anything that does not match.

### 4.4 Resolving app internals

**[VALIDATED]** It calls app functions through a pointer table at
`0x171d000 + {0x3c0, 0x3c8, 0x3e0, 0x3f8, 0x408, 0x410, 0x418, 0x420}`, filled at
load. It never reimplements the renderer — it drives the stock one.

### 4.4.1 Reading the app's own DSP out of a stripped binary

**[VALIDATED]** The app is a 119,243,440 B stripped AArch64 PIE (SHA-256
`b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`), but it
keeps **C++ RTTI**, and the class names map the whole signal path: `FilterS8`
with `enum FILTER_TYPE` and `generateFrequencyResponseCurve(FILTER_TYPE, f, f,
vector<float>&, …)` (the per-type filter model), `AirCompressor`,
`FX_Compressor`, `XFX_Compressor`, `Qbq16CompressorEffect`,
`Qbq25SidechainCompressorEffect`, `AC50CompressorMeterConnector`, and the `H3`
plugin framework whose presets are literal XML (`<H3Part Type='Dyn3Compressor'
…>`).

Two small tools in `tools/re/` make this practical, because full auto-analysis of
a binary this size is too slow to iterate with:

```sh
# every reference to a rodata string or global, in seconds
python3 tools/re/fwxref.py "$FW" filterCutoff gainCoefficient
# RTTI name -> type_info -> vtable of function pointers
python3 tools/re/rtti.py  "$FW" 9FilterS8
# every BL call site to an address (e.g. after finding a helper)
python3 tools/re/calls.py "$FW" 0xb52e18
```

`fwxref.py` scans for `adrp`/`add`/`ldr` pairs, which is how essentially every
AArch64 reference to a fixed address is formed. `rtti.py` walks Itanium ABI
type_info objects, whose `name` pointer points at the mangled name string.
`calls.py` answers the inverse question — who calls this address — by decoding
`BL` immediates, which is how the JUCE IIR coefficient builders were found
(FINDINGS §73).

That route already paid off twice: it resolved the voice filter to
`juce::dsp::IIR::Coefficients<float>` and showed the device hardcodes Q = 1/√2,
which replaced an invented resonance→Q curve in the converter.

The extracted binary itself must stay **outside the repository** (it is 114 MiB);
`analysis/` is git-ignored. Re-extract with:

```sh
7z x firmware/extracted/part2 "usr/bin/MPC Sample"   # part2 = the ext4 rootfs
```

The `.xpj` insert blobs (`plugin.state`) are also readable now: plain base64 with
`/` written as `.`, length-prefixed inside the string. The decoded payload is a
bit-packed parameter stream (`0xff` = unset) that needs the app's bit widths to
unpack — see FINDINGS §72 and `docs/FORMAT.md` §15.

### 4.5 Project safety net

`qol-snap.sh` keeps a shadow copy of the open project under
`data/qol-snap/live` (hard links, so samples cost no extra space), plus the last
5 `.xpj` revisions, and re-links them if stock deleted files. This exists because
stock loading a mod-created project can drop samples that only the mod used.

---

## 5. Toolchain

The mod is an ordinary ARM64 Linux shared object. It can be built from macOS
with the system clang — no Linux machine needed:

```sh
clang -target aarch64-linux-gnu -shared -fPIC -nostdlib \
      -Wl,--unresolved-symbols=ignore-all -o ourmod.so ourmod.c
```

`-nostdlib` avoids needing an aarch64 sysroot; unresolved libc symbols resolve
from the running process at load. For anything beyond a trivial constructor a
proper sysroot or a `buildroot`/`crosstool-ng` toolchain is worth the setup.

---

## 6. Constraints and risks

| constraint | detail |
| --- | --- |
| Firmware lock | first 4 KB of the app pinned to 1.3.0 (`44507c7a…`); any other firmware needs every patch site re-derived |
| No flash | the mod exists only in RAM; power off restores stock |
| Card format | exFAT, folder named exactly `autorun` at the card root |
| Autorun gate | unproven (§3); everything depends on it |
| SSH is not a shortcut | `sshd` exists but login goes through `/usr/bin/az01-ssh-login`, a `ForceCommand` wrapper validating a product string from the key certificate; `debug-shell.service` only fires on a failed boot |
| ASLR | already disabled by the stock launcher, so fixed addresses are stable |

---

## 7. Design study: per-pad stem export (not built)

Recorded because the analysis is reusable if the mod is ever picked up again.

**Why:** the device can export a mixdown but not stems; per-pad audio is exactly
what `mpc2stems` reconstructs offline.

**How SampQOL does it (per track)** — **[VALIDATED]** from its strings:

```
export: render %d/%d %s seq %d bars %d..%d tail %d, %d track(s) muted -> %s
Mix + %d stem%s
submix: stem writer reached idx=%lu tid=%ld
Samples/User/Export   /Stems
```

It renders the sequence **once per track with the other tracks muted**, plus a
`Mix` file. Submixes instead **tap the submix bus**. Options exposed:
`Export Master FX`, `Export Tail`, `Export Stems Same`, `Export BPM in Name`,
`Export Separator`.

**Decisions reached before the study was parked:**

| question | decision |
| --- | --- |
| trigger | "Export stems" beside "Export mixdown" in song mode, reached by a `SHIFT` + confirm combo |
| stems | **dry** (no master insert chain) — the Color Compressor is non-linear and would not sum |
| mixdown file | not written by the stems export |
| tail | +3 s, always on (matches the stock mixdown window of 29.1818 s) |
| which pads | only pads that play in the song |
| output | `Samples/User/Export/<song>-stems/`, names `A01-<sample>.wav` |
| feedback | stock progress per render + a log file |

**Why it was parked:** the mute-and-re-render design needs one render per pad
(≈6 min for 12 pads). Hitting under 30 s requires a different architecture — a
**single pass that taps every pad** during one render. That tap was never found:
it is plausible (the engine sums per-pad voices) but not located, and if voices
are summed per *track* rather than per *pad* the tap yields tracks, which is
useless with a single drum track.

---

## 8. If this is resumed, start here

1. **Run the bypass experiment** (§3). Everything else is blocked on it.
2. If it fails, decompile the compare at `0x2416cc0` in the app — the digest
   input is decided in that function.
3. Pin the SD mount path from the rootfs rather than guessing it.
4. Decide the architecture before writing code: `mute+re-render` is days and
   ~6 min per song; `single-pass tap` is weeks and <30 s.

---

## 9. Glossary

| term | meaning |
| --- | --- |
| **autorun** | stock feature that runs a script from the `autorun` folder at boot |
| **`autorun.check`** | digest the device compares against to reject a modified folder |
| **veneer** | trampoline within branch range of a target, so a `BL` can be retargeted |
| **wrap hook** | hook that calls the original function through the veneer |
| **bypass** | running our own `autorun.sh` by changing nothing the device checks |
