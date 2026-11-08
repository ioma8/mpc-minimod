# The addin ecosystem — what other MPC Sample addins do, and what applies to us

In November 2026 inMusic published its open-source compliance dump for firmware
1.3.0, and Jacob Sabella published four MPC Sample addins. This document records
what those projects contain, which of it is usable for minimod, and which parts
we should not copy. Firmware facts they confirmed are in
[`MECHANISMS.md`](MECHANISMS.md); candidates are marked **implemented** once
adopted, otherwise **not implemented** — the doc is a study plus a record of what
was taken from it.

## Sources

| project | what it is | licence |
|---|---|---|
| [`inmusic-software/az0x-open-source`](https://github.com/inmusic-software/az0x-open-source/tree/mpc-sample/1.3.0) branch `mpc-sample/1.3.0` | licence-compliance dump of the patched third-party sources used by firmware 1.3.0 | per component |
| [`jacob-sabella/mpc-addin-sample-draw-handler`](https://github.com/jacob-sabella/mpc-addin-sample-draw-handler) | one addin that owns the screen on behalf of plugins/addins that draw | MIT |
| [`jacob-sabella/mpc-addin-sample-scalemode`](https://github.com/jacob-sabella/mpc-addin-sample-scalemode) | hold 16 Levels to play the 16 slots in a scale | MIT |
| [`jacob-sabella/mpc-addin-sample-rack`](https://github.com/jacob-sabella/mpc-addin-sample-rack) | add/remove/disable plugins in a project's inserts | MIT |
| [`jacob-sabella/mpc-addin-remote`](https://github.com/jacob-sabella/mpc-addin-remote) | screen and touchscreen in a browser, plus an MCP server | MIT |
| [`jacob-sabella/mpc-vst-plugins`](https://github.com/jacob-sabella/mpc-vst-plugins) | the shared addin installer and the addin protocol doc | MIT |

Three of the four ship a `tested.json` naming the **MPC Sample on firmware
1.3.0.12**: draw handler 1.0.1, scale mode 0.1.1–1.1.0, remote 1.1.0–1.2.0 (dates
2026-10-16 to 2026-11-06). The rack ships no `tested.json`; its README describes
the device behaviour it was built for. Treat "runs on the Sample" as their claim,
not as our verification.

### The compliance repository is not source

Every file on the `mpc-sample/1.3.0` branch is a ~130-byte Git LFS pointer
(`version https://git-lfs.github.com/spec/v1` + `oid` + `size`) to a patched
third-party tarball. There is **no MPC Sample application source** there, so it
cannot be used to verify an offset. What it does give us is the exact component
recipe for this firmware:

glibc **2.39**, alsa-lib **1.2.11**, BlueZ **5.72**, ConnMan **1.42**, systemd
**255.21**, kernel **6.12.60 (PREEMPT_RT)**, u-boot **2025.10**, libsndfile
**1.2.2**, flac 1.4.3, lame 3.100, LADSPA SDK 1.17, exfatprogs 1.2.2, libgpiod
2.1.3, ell 0.64, libconfig 1.7.3, plus the usual base packages.

Consequence for us: a glibc 2.39 baseline is safe, and the addin build's
`GLIBC_2.xx` symbol-version check is a cheap guard we could adopt.

## What an "addin" is

An addin is a shared library that MPC's process loads with `LD_PRELOAD`; it is
*not* a plugin loaded by the track host. It runs inside `/usr/bin/MPC Sample`,
in the same address space, before MPC finishes starting. That is exactly the
delivery technique minimod already uses, so most platform rules below apply to
us directly.

There is still **no persistent official loader**. The draw handler's README says
so outright: an addin is "loaded through a drop-in in `/run` and is gone after a
reboot until that is solved". The community answer is `/data/mpc-addins/<id>/`
plus a shared systemd drop-in, installed by a script the addin ships.

## The shared installer contract

Documented in [`mpc-vst-plugins` `docs/ADDINS.md`](https://github.com/jacob-sabella/mpc-vst-plugins/blob/catalog-sample-scalemode/docs/ADDINS.md)
(not on `main`). The parts that matter:

- **`Environment=LD_PRELOAD=` replaces, it does not append.** Every addin
  installs through one shared agent so the list is composed:
  if a unit or drop-in already sets `LD_PRELOAD`, the effective file is edited in
  place, the addin's `.so` is appended, and a `*.bak-mpc-addins` copy is kept.
  Otherwise a shared drop-in
  `/etc/systemd/system/<service>.service.d/90-mpc-addins.conf` holds a `# base:`
  line with the firmware's own list, then the addins. On a read-only root or a
  dm-verity Gen2 device the same file goes to `/run/systemd/system/...` and is
  rewritten after every reboot.
- **The service name differs per device**: `acvs` (MPC OS 3.x), `ac50` (MPC
  Sample), `inmusic-mpc` (modified firmware). Our mod is Sample-only and hard-codes
  `ac50`, which is correct here.
- **Files are staged** as `<file>.new` and renamed, because a running MPC keeps
  the old `.so` mapped. Then MPC is restarted.
- **The `.so` is checked before it touches `LD_PRELOAD`**: it must be a 32-bit
  little-endian ARM shared object, because anything else in `LD_PRELOAD` stops
  MPC from starting.
- **Settings are installed only when absent**, so user edits survive upgrades.
  `ADDIN_FILES` entries that are ELF or `#!` get mode 755, the rest 644.
- **A firmware update requires re-running the installer**: the drop-in's base
  list changes and the root filesystem is replaced.
- **MockbaMod**: when a card with `MockbaMod/env.sh` is found, the installer also
  writes `AddOns/run_<id>.sh` and appends to `/dev/shm/.LD_PRELOAD`.
- **`/etc` is mounted after systemd reads units on MPC OS 3.x.** A drop-in in
  `/etc` is missed on the first boot unless a one-shot
  `mpc-addins-reload.service` (after `etc.mount`, before MPC) runs
  `systemctl daemon-reload`. Our authoring path writes to `/run` and calls
  `daemon-reload` itself, so it does not depend on that unit.
- **DRM master is a trap**: whatever opens `/dev/dri/card*` first, while the card
  has no master, becomes master and MPC then fails with "Failed to initialise
  display". Any addin or tool that opens the card must call
  `DRM_IOCTL_DROP_MASTER` immediately. We do not open DRM today; this matters if
  we ever add screenshots.

### What minimod does today, and the gap

Our `autorun.sh` writes
`Environment=LD_PRELOAD=/run/minimod.so` into
`/run/systemd/system/ac50.service.d/minimod.conf`. That replaces the variable.
On a device with another addin installed (its shared drop-in is
`90-mpc-addins.conf`, which sorts before `minimod.conf`), our drop-in wins and
the other addin disappears; the same happens to any list the firmware itself
sets.

**Candidate change (not implemented):** compose in `autorun.sh` —
`systemctl show ac50 -p Environment`, extract the effective `LD_PRELOAD`, append
`/run/minimod.so`, write that list back. This is the one interoperability bug the
study found that is cheap to fix and has a clear failure mode.

## Coexistence rules

- An addin hooks libc by defining a same-named function and must find the next
  definition with `dlsym(RTLD_NEXT, name)`; it must fetch libc's `dlsym` with
  `dlvsym` in case another addin replaced it. Hook chains then work in either
  order (the rack's `tests/other_hook.c` tests exactly this).
- Build with `-fvisibility=hidden` and export only what is needed: two addins
  that define the same name are a catalog-visible conflict. The remote addin
  exports a single symbol, `mpc_remote_addin_start`.
- Addins run only in the MPC process: check the executable name. The remote
  addin also honours `MPC_REMOTE_ADDIN_DISABLE=1`, runs its threads with all
  signals blocked, nice 10, 256 KB stacks, no `signal()`/`SIGPIPE` changes,
  close-on-exec descriptors, bounded clients/timeouts, and retries a taken port.
- A broken addin crashes MPC at every start; SSH stays up because it is a
  separate service, and `enabled=0` in the settings file leaves the library
  loaded but idle.

## The four addins

### draw handler — screen ownership and a control surface

[`mpc-addin-sample-draw-handler`](https://github.com/jacob-sabella/mpc-addin-sample-draw-handler)
wraps `ioctl` inside MPC. MPC double-buffers and page-flips whenever its UI
changes; on each flip the addin saves MPC's new frame and paints the current
owner's 320×240 layer over it, then refreshes MPC's display fd (only the DRM
master may). This is *pixel composition*, not a UI-toolkit hook — the opposite
of minimod's approach, which draws from the app's own paint pass.

Its client API ([`src/mpc_sample_draw.h`](https://github.com/jacob-sabella/mpc-addin-sample-draw-handler/blob/main/src/mpc_sample_draw.h),
version 3) is looked up with `dlsym(RTLD_DEFAULT, "mpc_sample_draw_api")`:
`open`/`close`, `request`/`release`/`has_screen`, `pixels`/`present`, a focus
callback, `set_input` and `show(name)`. While a client owns the screen, the three
soft keys, the dial (steps and press) and K1–K3 are routed to it; pads, transport
and the page button stay with MPC. `KNOB_AT` delivers a knob's current position
when the screen arrives, for pickup.

**Usable for us:** the control interface is a ready-made lab/device harness.
On `127.0.0.1:6740` (default): `/status`, `/grant?id=N`, `/release`, `/cycle`,
`/test?on=1`, `/input?key=K|dial=N|knob=K&value=V`, and `/snap` returning the
screen as 320×240 pixels, 4 bytes each, `0x00RRGGBB` little-endian, row-major.
`/snap` plus `/input` is the device-side equivalent of the emulator panel's
`/screen.png` + `/control`. The addin itself is not useful to us: our overlay
already draws through JUCE at repaint time, which is cheaper and shape-correct.

Page cycling is a **long hold (800 ms) of PAD FX** (control-surface note 1),
which never reaches MPC; a short press does, on release. `page_home=RACK` makes
that hold open the rack page, and pages opened with `show()` return to it.

### remote — DRM capture, evdev touch injection, MCP

[`mpc-addin-remote`](https://github.com/jacob-sabella/mpc-addin-remote) is the
closest thing to a device-side test harness we have seen.

- **Screen:** DRM/KMS, not `/dev/mem`. Find the lit CRTC on `/dev/dri/card*`,
  `GETFB2` the framebuffer (needs `CAP_SYS_ADMIN`), map it read-only with
  `MAP_DUMB`, follow page flips. Size and format come from the device
  (XRGB/XBGR 8888, RGB565); tiled or compressed buffers are refused. The scanout
  may be portrait with the UI drawn on its side, so rotation is applied.
  The DRM ioctls are written out in `src/drm_min.h`.
- **Touch:** writes `struct input_event` records into the **touchscreen's own
  evdev node** — slot 0 of multitouch protocol B, plus `BTN_TOUCH` and
  `ABS_X`/`ABS_Y` — so MPC reads them as a real finger. The device is found by
  ABS_MT axes, skipping `BUS_VIRTUAL`, preferring `INPUT_PROP_DIRECT`, and its
  axis ranges are read from the device; coordinates are mapped from the upright
  picture back into the scanout. Open the node write-only (a reader would be sent
  events it never reads).
- **Buttons:** on the Sample there is no "…Private" ALSA port, so notes are
  injected through `/dev/midi_inject`, a custom kernel module (documented in
  mpc-vst-plugins; it is not part of the addin). Other devices accept notes on
  their private MIDI port.
- **MIDI:** an ALSA sequencer client "MPC Remote" with an output port for notes,
  CC, program, pitch bend, transport, clock and SysEx, and an input port for
  what MPC sends back. Note names put middle C (60) at C3.
- **MCP:** `POST /mcp` with `screenshot`, `tap`/`double_tap`/`long_press`/
  `drag`/`scroll`/`touch`, `wait_for_screen`, `get_screen_info`, `play_notes`,
  `send_midi`, `list_buttons`/`press_button`/`learn_button`, `midi_listen`,
  `midi_status`, `device_status`, and read-only `list_files`/`find_files`/
  `read_text_file`.

**Usable for us:** touch injection is the only known route to the touch-only
Preferences UI; DRM capture is the only known on-device screenshot route; the MCP
server is a ready-made way to drive a real device during hardware validation.
MCP is HTTP-only with no login, bound to loopback and reached over SSH, so it is
a development tool, not something to bundle.

**Not usable in our lab:** the emulator's tablet is `BUS_VIRTUAL`, which the
touch detector deliberately rejects, so browser touch would not work there even
if the addin were preloaded.

### scale mode — hold gestures, the sample's root note, pitch detection

[`mpc-addin-sample-scalemode`](https://github.com/jacob-sabella/mpc-addin-sample-scalemode)
is the most useful source of firmware facts.

- **Control surface without offsets.** MPC reads buttons from
  **`/dev/snd/midiC0D0`** in ALSA's timestamped framing: whole **32-byte
  frames**, MIDI bytes at **offset 16**, one button per frame, note on/off on
  channel 1. `src/hold.h` wraps `read()` on that fd, **holds back** the 16 Levels
  press, and either passes it to MPC on release within the threshold or swallows
  it and toggles Scale. Short presses pass through; Shift+16 Levels still cycles
  the Type. Two wrappers chain through `RTLD_NEXT`.
- **The sample's root note**, which the 16-Levels renderer uses: instrument
  vector at `+0x2808/+0x2810` (128 pointers, checked as `end - begin == 128*8`),
  `instrument + 0x22e8` → layer array, `+0x4a8` → sample, `sample + 0x130` →
  data, **`data + 0x3e4` = root note as int32 0–127**. Their code validates every
  pointer and falls back to a configured `pad_root` when reading fails.
- **In-memory sample audio:** `sample + 0x170` → holder object with RTTI
  **`"12MemorySample"`**, `+0x18` == the data object, `+0x28` → holder,
  `holder[0]` → the sample buffer; the data header is
  `{u16 channels, u32 frames, u32 rate, u32 bits}` with `bits == 32` (float32).
  `src/sample_scalemode.c` `sample_audio()` is the reference.
- **Pitch detection:** `src/pitch_detect.h` is YIN — 2048-frame windows,
  30 Hz–2 kHz, threshold 0.15, aperiodicity gate 0.10, windows taken after the
  loudest attack, returning a fractional MIDI note or -1. Used when MPC's root is
  60 because the WAV has no `smpl` chunk, and optionally for every sample.
- **Their hooks are ours.** They hook `0x21472f0` (16-Levels note map) and
  `0x27e4400` (pitch setter) — the same anchors we use — plus
  `0x1542030`/`0x1542260`, with PAC-aware trampolines installed at `PACIASP`.
  They call the setter with **semitone offset relative to the sample's root**,
  which confirms the arithmetic our chromatic MIDI should use.
- A `padctl` CLI (`ADDIN_FILES`) presses buttons/pads and reports what the
  detector heard, by queueing frames that MPC reads next — a model for a test
  seam that needs no host.

### rack — the project has plugin inserts

[`mpc-addin-sample-rack`](https://github.com/jacob-sabella/mpc-addin-sample-rack)
shows a capability we did not know about: the Sample's project has **inserts with
plugins** (four inserts per channel, one instance of each plugin per project;
MPC's own effects are displayed but never changed). There is no screen for
adding or removing them, so the addin edits the project file
(`/data/TempProjects/Temp-Project.xpj`) and restarts MPC for structural changes
(APPLY runs `sample-rack-apply` as its own systemd unit). It can toggle a running
plugin **live** by interposing `dlsym` for plugin entries and passing lookups on,
and it exposes `mpc_sample_rack_api/state/apply/live` to other addins via
`dlsym` — no socket.

**Relevance to us:** none of our features touch inserts, but a modded device can
have its project's insert graph changed by another addin, and a plugin's latency
or state can explain perceived behaviour. It is a platform fact for
[`DEVICE-NOTES.md`](DEVICE-NOTES.md), and a future "render with inserts" feature
would start here.

## Firmware facts confirmed by this study

Verified by me against our own firmware in the live lab (not just read from
their source), now recorded in [`MECHANISMS.md`](MECHANISMS.md):

| fact | value | how confirmed |
|---|---|---|
| sample root note | `sample+0x130 -> data; data+0x3e4` int32 0–127 | read live: A01 `RnB-Kik-Grove1 91V` = **88**, pads 1/9/13 = 60 |
| sample data header | `{u16 ch, u32 frames, u32 rate, u32 bits==32}` | read live: `(2, 10501, 44100, 32)` for A01 |
| instrument vector | `+0x2808/+0x2810`, 128 × 8 bytes | read live: `end - begin == 1024` |
| control-surface port | `/dev/snd/midiC0D0`, 32-byte frames, MIDI at +16 | their `src/hold.h`, matching our rawmidi observations |
| pitch arithmetic | setter argument = semitones from the sample's root | their `scale_map_hook` call, same anchor we use |

**Claimed by them, not yet verified here:** that the Sample's touch panel is
reachable by the evdev write, that `/dev/midi_inject` works as described, that
DRM capture works on the Sample's panel, and the insert limits above.

## What applies to minimod

| item | value | risk | status |
|---|---|---|---|
| Chromatic pitch measured from `data+0x3e4` instead of C3 | high — A01 was 28 semitones off; samples tagged 60 unaffected | low; one read per note, fallback 60 | **implemented** (FEATURES, chromatic MIDI) |
| Compose `LD_PRELOAD` instead of replacing it | high — otherwise minimod silently disables other addins and firmware preloads | low; shell only | **not implemented** |
| `MINIMOD_DISABLE=1` kill switch | medium — the addin-standard way out of a crash loop | very low | **not implemented** |
| Block signals on our worker threads (HPSS, toast timer) | medium — stops us stealing MPC's RT signals | very low | **not implemented** |
| `-fvisibility=hidden` for the `.so` | low/medium — we currently export 8 symbols (`hpss_split`, `spectral_onsets`, `bass_sample_render`, `h3m_effect_render`, `bass_preview_*`) | very low | **not implemented** |
| glibc symbol-version check in `build.sh` | low — catches an accidental newer baseline | very low | **implemented** (build.sh rejects > 2.39) |
| Touch injection (evdev write) to reach Preferences | high for discoverability, unproven on this panel | medium; needs a hardware session | experiment |
| DRM frame capture for on-device screenshots | medium — hardware verification without QMP | medium; `DROP_MASTER`, format/rotation handling | experiment |
| Hold gestures via control-surface `read()` wrapping | medium UX (hold instead of SHIFT chords) | medium; must chain with other wrappers, replaces part of our input model | idea |
| In-memory sample audio for AutoTrim/effects | medium — works for unsaved samples; identical results to the WAV path | low; float32 interleaved, WAV fallback kept | **implemented** (FEATURES, AutoTrim and effects) |
| YIN root detection for untagged samples | medium — makes "play in key" work for untagged WAVs | low; MIT code, but adds a worker and policy | idea |
| `-mcpu=cortex-a55` build | none proven | high — LSE atomics on a v8.0 lab CPU would SIGILL | **do not adopt** |
| Depend on `/dev/midi_inject` | none for us | high — kernel module, not shipped | **do not adopt** |
| Move to `/data/mpc-addins` + persistent systemd edits | none — our autorun is RAM-only and reversible by pulling the card | medium | **keep autorun** |

### Order if we implement

1. Root-aware chromatic pitch — implemented; live readout verified (A01 root 88
   → delta −21), hardware MIDI on a non-60 sample still pending.
2. glibc symbol-version check — implemented in `build.sh`.
3. In-memory sample audio — implemented for AutoTrim and the transforms;
   identical results to the WAV path (reverb RMS unchanged, same trim bounds).
4. `LD_PRELOAD` composition in `autorun.sh` — not implemented.
5. The remaining safety items: kill switch, signal-blocked workers, hidden
   visibility.
6. Hardware experiments (touch/Preferences, DRM capture) as separate work.

## What not to copy

- The draw handler's `ioctl` ownership model: two screen owners in one process is
  a conflict, and our overlay is cheaper and correct at the vector level.
- Their hook implementation: PAC-aware trampolines at `PACIASP` are sound, but we
  already have a validated relay/trampoline with prologue checks; rewriting it
  buys nothing.
- Their packaging: persistent `/data/mpc-addins` and systemd edits change the
  device; minimod's value is that pulling the card reverts everything.
- Any dependency on their installer, MCP server, or kernel module at runtime.

## References

- Addin protocol: `mpc-vst-plugins` `docs/ADDINS.md` (branch
  `catalog-sample-scalemode`).
- Draw API: `mpc-addin-sample-draw-handler/src/mpc_sample_draw.h`; settings
  `mpc_sample_draw_handler.conf`.
- Remote: `mpc-addin-remote/src/{capture.c,touch.c,buttons.c,midi.c,drm_min.h}`,
  `README.md`, `tested.json`.
- Scale mode: `mpc-addin-sample-scalemode/src/{hold.h,pitch_detect.h,sample_scalemode.c}`,
  `README.md`.
- Rack: `mpc-addin-sample-rack/README.md`, `src/{addin.c,live.c,bridge.c}`.
- Sample root note / `MemorySample`: `sample_scalemode.c` `mpc_root_of()`,
  `sample_audio()`.
