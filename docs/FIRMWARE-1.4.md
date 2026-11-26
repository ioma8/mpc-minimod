# Firmware 1.4.0 — what changed

**Status: static findings plus live MiniMod validation.** The rows below are
image-derived findings from file trees, strings, unit files and the boot FIT.
MiniMod's 1.4.0.8 integration is now exercised in the virtual lab; the current
20-case emulator suite passes. That does not prove every stock firmware feature
is reachable or working on physical hardware. Rows marked `[static]` still mean
string and filesystem evidence rather than decompilation.

MiniMod supports 1.3.0.12 and 1.4.0.8. The build profile and matching autorun
guard are selected together with `MINIMOD_FIRMWARE`; see the firmware profile
section in [`README.md`](../README.md).

## The target

| | 1.3.0.12 | 1.4.0 |
|---|---|---|
| update image | `MPCSAMPLE-1.3.0.12-update.img`, 169,207,872 B | `MPCSAMPLE-1.4.0-update.img`, 242,924,096 B |
| boot partition | 28,577,792 B | 24,756,224 B |
| rootfs partition | 427,117,568 B | **602,190,848 B** (+175 MB) |
| recovery partition | 28,303,360 B | 29,167,616 B |
| `/usr/bin/MPC Sample` | 119,243,440 B, sha256 `b6584cba…0193b508` | 128,398,464 B, sha256 `2271252f…7727ec2` |
| kernel modules | `6.12.60-imb-2025-12-03-rt13` | **`6.18.59-imb-2026-10-26-rt9`** |
| dm-verity | `dm-mod.create=` inside the kernel cmdline | `-- --table=` left for the bootloader |

The 1.3.0 app hash above is the one this repository already records in
[`README.md`](README.md), and the extraction below reproduces it byte for byte —
which is the check that the unpacking is right.

## How the update image unpacks

`update.img` is an `AZ0x` container:

```
0x00  "AZ0x", version 1
0x38  string pool ("SNAPSHOT-<timestamp>", "MPC Sample image", "AC50", …)
0x90  four 0x40-byte entries, one after another:

      +0x00  name[4]      "BOOT", "PARR", "PART", "PART"
      +0x04  u32 type     0 = raw, 1 = xz stream
      +0x08  u64 offset   inside update.img
      +0x10  u64 length   of the stream
      +0x18  u64 extra
      +0x20  digest[32]
```

`BOOT` is a raw FIT (kernel, initrd, two device trees) that the lab does not use.
The three xz streams map to the partitions that matter, and the mapping was
proven rather than assumed: decompressing **1.3.0.12**'s streams reproduces its
three published partition hashes exactly.

```sh
7z x "MPC Sample 1.4.0 Updater.exe" update.img          # 7z SFX payload
# then, per entry: dd the stream out of update.img and `xz -dc` it
#   PARR   -> extracted/parr    (recovery)
#   PART 1 -> extracted/part1   (boot, ext2 with kernel.fit)
#   PART 2 -> extracted/part2   (rootfs)
```

1.4.0 boots differently in one respect that the lab has to handle: it no longer
puts the verity mapping in the kernel command line. 1.3.0.12:

```
root=/dev/dm-0 rootwait=5 ro … dm-mod.waitfor=/dev/mmcblk0p10 dm-mod.create="rootfs,,,ro,0 808140 verity 1 …"
```

1.4.0:

```
ro rfkill.default_state=0 systemd.getty_auto=n fsck.repair=yes -- --table="1 /dev/mmcblk0p10 … 569697 569697 sha256 <root> <salt>"
```

Same table, same salt, no `root=` and no `dm-mod.create`. The device's bootloader
is expected to expand it. The lab boots the kernel directly, so it expands the
table itself — `scripts/verity_cmdline.py` in the virtual-MPC-sample repo, called
from `build-images.sh`.

## Scale of the diff

| | 1.3.0.12 | 1.4.0 |
|---|---|---|
| rootfs paths | 6,315 | 6,334 (489 added, 370 removed) |
| unique app strings (≥7 chars) | 85,252 | 92,168 (11,516 added, 4,600 removed) |

Most churn is recompilation noise. The sections below are what survived
filtering by new classes, new unit files and new user-facing labels.

## 1. Scripting — the largest addition

Scripting strings go from 290 to **1,528**. The app gains a scripting layer with
two engines and a package format:

| evidence | what it shows |
|---|---|
| `Scripting::LuaRegistrar`, `Scripting::ThreadBasedRegistrar`, thread-wrapped functions | **Lua** bindings with per-thread marshalling |
| help text: *"NOTE: This should only be used from javascript. For lua scripts, the `x = propertyName.value` pattern is preferred"*, *"unimplemented in Javascript due to C++/JS binding limitations. Only available in Lua"* | **JavaScript** bindings too, with documented gaps |
| `*.scriptpkg`, *"… contains an autorun script. Do you want to run it now?"*, *"About to run unprotected script:"* | a distributable script package, with signed and unprotected variants |
| `ScriptablePadWrapper`, `ScriptableSamplePool`, `ScriptableCurrentTrack`, `ScriptableCurrentInstrument`, `ScriptableQLinkWrapper`, `ScriptableProject`, `ScriptableTrackPoolProperties`, `Scripting::ClipPropertiesMatrix`, `ScenePropertiesArray` | what scripts can reach: pads, samples, tracks, instruments, Q-Links, projects, clip/scene matrices |

This is the part that overlaps MiniMod's own territory: a supported way to drive
pads and samples, rather than hooking the app.

## 2. Remote Screen (beta)

| evidence | what it shows |
|---|---|
| `REMOTE SCREEN`, `REMOTE SCREEN (BETA)`, `Remote Screen Viewer on a …` | a screen-viewer feature, marked beta |
| `ALLOW REMOTE CONTROL`, `PASSWORD`, `NO AUTHENTICATION` | an optional control channel with or without authentication |
| `Enhancements.RemoteScreen`, `.RemoteScreenAllowControl`, `.RemoteScreenHighRefreshRate`, `.RemoteScreenPassword`, `.RemoteScreenShareAudio` | five settings, including audio |
| `remote_screen::FrameSource`, `AppFrameSource`, `MemoryFrameSource`, `InputSink` | frame producers and an input sink, i.e. it can feed and accept input |
| `az0x-webserver` moved `usr/bin` → `usr/libexec`, `libaz0x-webserver-client` 0.5.1 → 0.7.1, new `var/www/ac50/static/js/main.f298ad50.js` | the web side that serves it |

The shape is a browser-based viewer with optional remote control and audio,
authenticated by password.

## 3. Enhancements (beta)

`Enhancements` does not exist in 1.3.0.12 (0 strings); 1.4.0 has 199.

| evidence | what it shows |
|---|---|
| `ENHANCEMENTS (BETA)`, `DEV ENHANCEMENTS (BETA)`, `ENABLE ENHANCEMENTS`, `ENHANCEMENTS` | a user-facing menu and a developer variant |
| `Enhancements.Enabled`, `AC5xDevEnhancements` | one master switch plus a settings object |
| `Enhancements.AdvancedAudio`, `.AdvancedAudioBufferSize`, `.AdvancedAudioLatencyAdjust` | audio buffer size and latency trim |
| `Enhancements.Screenshots`, `.MousePointer`, `.DeveloperPanel` | developer conveniences |
| `Enhancements.UnlimitedTracks`, `.UnlimitedInstances`, `.VST2Support`, `.UntrustedOS` | host/plugin-era options |
| `Enhancements.RemoteScreen*` | see above |

`RENDER TIME`, `OVERRUNS`, `RESOURCES`, `ALMOST FULL`, `NO SPACE`, `RESTART`,
`REFRESH SPEED` appear alongside — a diagnostics panel.

## 4. MPE

MPE mentions roughly double (201 → 402), with new classes and settings:

| evidence | what it shows |
|---|---|
| `MPEChannelInfo::Listener`, `MPE Settings`, `MPE MIDI Settings`, `MPE Defaults` | MPE channel handling as a real subsystem |
| `MPE MIDI MONITOR`, `MPE Pitch Bend Settings`, `MPE Y Amount`, `MPE Y Mode`, `MPE Perf Param Lock`, `MPE Performance`, `MPE Lock` | per-channel bend, Y-axis and performance-parameter controls |
| `Convert MPCe to MPE` | conversion from the newer pad-expression format |

The device has no MPE hardware of its own, so the plausible reach is external
MIDI controllers.

## 5. Audio engine

| evidence | what it shows |
|---|---|
| `Air::AlgorithmicReverb` | a new reverb algorithm class |
| `Qbq::DiodeClipperEffect` | a diode-clipper distortion |
| `Air::Pumper` | a side-chain "pumper" |
| `AC5xFxCatalogue::ownEffects` (five overloads) | the effect catalogue grew |
| `CREATE BLOOM FILTER` | a new action label; not explained by the strings |

## 6. Entitlement-gated features

A licensing layer appears with four features behind flags. The machinery is new:
`softwareunlock::FeatureAssetsProduct`, `Checking for license…`, `Unlicensed`,
`Activation`, `This will deactivate`, and per-flag help strings of the shape
*"Whether the … feature is available"*.

| flag | adjacent evidence | reading |
|---|---|---|
| `SuperWarpFeatureEnabled` | warp/time-stretch family | an enhanced warp mode |
| `CVModulationFeatureEnabled` | `Property/Named/CVPorts`, `CvPortAndData`, `SerialisableCvPortAndData`, `SerialisableDrumCvProgramMappingData`, `AC5xMidiProperties.CVBase/CVDivision/CVSyncOut`, `MIDI/CV`, `AcvxHardwareIO::DriverOutput` | CV/gate control of external gear; the hardware has no CV jacks, so this is either for another SKU or for a companion box |
| `EnvelopeFollowersFeatureEnabled` | none beyond the flag | envelope-follower modulation sources |
| `EnhancedKeygroupsFeatureEnabled` | `KeygroupProgramConnector`, keygroup/zone vocabulary | multi-sample keygroup instruments |

Whether these are purchasable, SKU-limited or simply dormant here cannot be
determined from an image.

## 7. Programs, tracks, clips

The MPC-line project model is more present than in 1.3.0.12:

`ClipProgramConnector`, `InstrumentProgramConnector`, `KeygroupProgramConnector`,
`PROGRAMS`, `CREATE NEW TRACK`, `Save Current Program…`, `Error Saving Program`,
`Bounce track to sample`, `Bounce track to audio track`,
`Drum Track as MPC Sample file`, `Mode Track Mixer`, `Hide Project/Track/Program data folders`.
Track strings rise 2,006 → 2,480.

This may be shared code compiled in rather than a device UI.

## 8. Platform, services, security

| addition | evidence |
|---|---|
| verity trustpoints for LoadPin | `usr/lib/az0x/trusted-verity-digests`, `az0x-loadpin-trustpoints.service` → `/usr/sbin/az0x-loadpin-trustpoints`, runs `Before=local-fs-pre.target` |
| boot info logging | `az0x-info.service` → `/usr/bin/az0x-info`, oneshot, `RemainAfterExit=yes` |
| hardware control tool | `usr/bin/az0x-hwctl` |
| user access (SSH) | `usr/lib/az0x/user-access/{enable.sh,reset.sh,disclaimer,banner}`, `data-skel/ssh/authorized_keys`, `etc-overlay/ssh/sshd_config.d/00-user.conf`; `enable.sh` copies the skeleton to `/data`, overlays `/etc`, restarts `sshd` |
| test app launcher | `etc/test-app-launcher/test-app-launcher.conf` = `{"services-to-stop": ["ac50"]}` — stop the stock app and launch another |
| huge pages | `etc/sysctl.d/hugepages.conf` |
| license files | `usr/share/doc/inmusic/EULA.txt`, `common-licenses/{linux,u-boot}.txt` |

Library moves: `libaz01-prog` **1.3 → 1.8.1**, `libaz0x-info` 0.7.1 → 0.10.1,
`libaz0x-splashctl` 1.0.4 → 1.5.1, `libaz0x-webserver-client` 0.5.1 → 0.7.1,
`libexpat` 1.10.0 → 1.12.3.

## 9. Content and library

The factory project is new: `usr/share/Akai/AC50/TempProjects/AC50D/` with a
different sample set from 1.3.0.12 (BoomBap, HipHop, Keys, Synth, FullPh,
Players, DontQuit, Electric, Dilla, YBoy …). The demo library ships with the
firmware, so a factory-reset device no longer carries the old loop pack.

The web UI bundle was regenerated (`main.d096576e.js` → `main.f298ad50.js`) and
`icon512.png` changed.

## 10. Removed or relocated

| gone in 1.4.0 | note |
|---|---|
| `usr/bin/az0x-webserver` | now `usr/libexec/az0x-webserver` |
| `libidn2.so.0*` (+ its CA entries) | dependency dropped |
| systemd presets `10-acvs`, `10-sme` | services no longer shipped |
| `libaz01-prog.so.1.3`, `libaz0x-info.so.0.7.1`, `libaz0x-splashctl.so.1.0.4`, `libaz0x-webserver-client.so.0.5.1` | replaced by the newer sonames |
| module tree `6.12.60-imb-2025-12-03-rt13` | replaced by `6.18.59-imb-2026-10-26-rt9` |

## What is not established

* **Reachability.** Strings prove code, not UI. Programs, tracks, clips, VST2,
  unlimited tracks, CV ports and MPCe→MPE conversion all look like shared
  MPC-line code; a 16-pad sampler may expose none of it.
* **Behaviour.** No feature above has been exercised. Nothing here is `[rt]`.
* **The entitlement semantics.** Flags, a licence check and an activation flow
  exist; what unlocks what, and whether anything is for sale, is unknown.
* **What `CREATE BLOOM FILTER`, `Aperture`, `Air::Pumper` are for** beyond the
  labels and class names.

## The `CREATE NEW TRACK` dialog — RE map (incomplete)

An under-the-hood look at whether the track/program UI can be *opened* on this
device, for whoever picks this up. Addresses are 1.4.0 virtual addresses (the
first `PT_LOAD` maps file offset to VA identically; the second is offset+0x100000).

| item | address |
|---|---|
| `"CREATE NEW TRACK"` string | `0x4bed368` (in a dialog label block: `TRACK NAME`, `SEQUENCE NAME`, `SELECT TRACK...`, `STEP EDIT`, `ADD NOTE`) |
| its only code reference | `adrp/add` at `0x3ff6b40`/`0x3ff6b44` |
| dialog constructor | **`0x3ff69e0`** (`paciasp`, builds a `0x1ae8`-byte object) |
| its only caller | **`0x3fb93f8`**, inside the screen builder `0x3fb7480` |
| registration | the object is inserted into an id-keyed container (`0x3083`/`0x3084`) right after construction |

What is established:

* The dialog is **constructed at startup and registered**, not built on demand.
* The device **does** present modals: the power-key confirmation
  (`Shutdown / Do you want to shut down?`) renders correctly at 320×240. Its
  builder is `0x1bbbf70`–`0x1bbc670`.
* Dialog builders are **invoked indirectly** (`0x1bbbf70` has zero direct `bl`
  callers); their pointers are stored through `R_AARCH64_RELATIVE` entries of the
  form `(0x403, target, location)` — e.g. `0x1bbbf70` → slot `0x76e8670`,
  relocation record at `0x26d328`. Reconstructing that slot table reconstructs
  the class's vtable.

What is *not* established, and what blocked it:

* The overlay host's present call. The obvious candidates are too common to be
  it: `0x1e048b0` has 293 callers (property layer), `0x154a970` has 394 and
  `0x10305b0` has 308 (generic utilities). The calls inside the shutdown builder
  are `std::function` `blr` invocations, so there is no single
  `present(host, component)` symbol to copy.
* Two earlier readings were wrong and are recorded to save time: a `[x0, 0x250]`
  load in that builder is the **stack canary** (global at `0x7a14xxx`), not a
  host pointer; and screen-hash comparisons are unreliable because the display
  **animates** — three no-input captures differ (`2c0a8db`, `06c6ec7`, `c16c69b`).

### Runtime findings (gdb over QEMU's gdbstub)

A working link was established: `LAB_GDB=1 lab.py run` exposes QEMU's gdbstub on
`:1234`, and `gdb-multiarch` (installed into a copy of the lab tools image) sets
breakpoints in the guest. The app maps at `0xaaaaaaaa0000`, so runtime address =
`0xaaaaaaaa0000 + va` (it is launched through `setarch -R`, so the base is stable
across boots). Attaching to the gdbstub **pauses the VM** — the console and the
panel freeze until `continue`/QMP `cont`.

With a breakpoint on the shutdown dialog builder (`0x1bbbf70` + base), pressing
the power key hits it, and:

* `x0` is an **`AC50Engine`** instance (RTTI: vtable `…1885b0`, typeinfo name
  `"10AC50Engine"`), so the dialog builder is an **engine virtual method** and
  dialogs are engine-level overlays.
* the builder makes six calls through a per-item callback (`blr x2` at
  `0x1bbc164`, `x2` a code pointer) — i.e. it builds a list of entries, matching
  the multi-entry label block around `CREATE NEW TRACK`.
* the pointer to the *next* step is still missing: no call in that builder takes
  an overlay host explicitly, so the present likely happens through an engine
  vtable slot or in the dispatcher that invoked the method.

Next step that would converge: attach a debugger to the guest. The lab runs QEMU
on the host, so `-gdb tcp::1234` plus a remote client with breakpoints at
`0x3fb93f8` and `0x1bbbf70` would show exactly which vtable slot attaches a
dialog — no guessing. Note there is **no aarch64 gdb** on this machine (macOS has
lldb only; the lab container ships neither), so that toolchain has to be
provided first.

Verification note: patching `/usr/bin/MPC Sample` in the rootfs is impossible
(dm-verity), but the lab layer (`lowerdir=/layer:/lower`) shadows the stock file
while verity still validates the original, so a patched binary can be dropped in
the layer and rebuilt with `REBUILD_EMMC=1 lab.py build images`.

## What this means for MiniMod

* The app is 9 MB bigger (`2271252f…`). The SDK now carries a separately
  fingerprinted 1.4 profile and the lab suite exercises the selected MiniMod
  features on it. The 1.4 autorun guard is the first-4-KiB hash
  `2b1ce171…`; 1.3 retains `44507c7a…`.
* **Scripting may make some of MiniMod redundant** — pad and sample actions are
  exactly what a `*.scriptpkg` appears to reach. Worth evaluating before porting
  hooks.
* The Tune page still exposes `Semi Tune` / `Fine Tune`; the emulator pitch test
  confirms five fine steps produce about a half semitone on both supported
  builds.
* The lab's 1.4.0 setup (partitions, hashes, cmdline translation) is recorded in
  the virtual-MPC-sample repository rather than here; the 1.3.0 partitions are
  kept beside it so a port can compare both.

## Reproducing the diff

```sh
# 1. unpack the installer and its AZ0x container (see above)
# 2. file trees
7z l -slt extracted/part2 | sed -n 's/^Path = //p' | tail -n +2 | sort -u
# 3. app strings
7z e -y extracted/part2 "usr/bin/MPC Sample" -o.  &&  strings -a -n 7 "MPC Sample" | sort -u
# 4. classify
comm -13 s130.txt s140.txt | grep -E '^[A-Z][A-Za-z0-9 &/+().,:!?#-]{2,40}$'
```
