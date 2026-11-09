# Documentation

Everything about the MPC Sample firmware 1.3.0.12 and the MiniMod, in one place.
The converter (`mpc2stems`) is a separate product with its own docs in
[`../converter/docs/`](../converter/docs/) and
[`../converter/CONVERTER-HANDOFF.md`](../converter/CONVERTER-HANDOFF.md).

## Reading order

* **New to the project** — [`../README.md`](../README.md) for what the mod does,
  then [`FEATURES.md`](FEATURES.md) for exact behavior.
* **Modding or porting** — [`MODDING.md`](MODDING.md) (boot chain, autorun, hook
  technique, packaging), then [`MECHANISMS.md`](MECHANISMS.md) for the addresses.
* **Investigating a new mechanism** — [`RE-METHOD.md`](RE-METHOD.md) (how claims
  are ranked and what has already failed), [`TOOLING.md`](TOOLING.md) (exact
  commands), [`HISTORY.md`](HISTORY.md) (dead ends).
* **Setting up the lab** — [`EMULATOR.md`](EMULATOR.md).
* **Platform questions** — [`DEVICE-NOTES.md`](DEVICE-NOTES.md) (hardware,
  Bluetooth, WiFi, the hidden touch Preferences UI).
* **The Rust code** — [`RUST-PORT.md`](RUST-PORT.md) (crate layout, lint policy,
  the C-to-Rust module map, what is proven how) and
  [`../rust/README.md`](../rust/README.md).
* **The Live Looper** — [`LIVE-LOOPER.md`](LIVE-LOOPER.md).
* **Interop with other mods/addins** — [`ADDIN-ECOSYSTEM.md`](ADDIN-ECOSYSTEM.md)
  (how other addins load, the shared installer rules, and what we can reuse).

## Ownership — one home per topic

| document | owns | does not contain |
|---|---|---|
| [`FEATURES.md`](FEATURES.md) | what each feature does, how it is implemented, measured validation, known limits | firmware internals, history |
| [`MECHANISMS.md`](MECHANISMS.md) | firmware mechanisms, object layouts, addresses, evidence level | feature behavior, narrative |
| [`RE-METHOD.md`](RE-METHOD.md) | ground-truth hierarchy, the investigation loop, what paid off, blind paths, hardening rules, checklist | specific addresses (those live in MECHANISMS) |
| [`TOOLING.md`](TOOLING.md) | every tool with invocations: Ghidra + MCP + Java scripts, kuna/r2, lab, probes, harnesses, device probes | results (those live in MECHANISMS/HISTORY) |
| [`MODDING.md`](MODDING.md) | boot chain, autorun protocol and its digest, hooking technique, packaging, constraints, ecosystem survey | feature internals |
| [`EMULATOR.md`](EMULATOR.md) | the QEMU lab: setup, ports, panel protocol, lifecycle | RE method |
| [`DEVICE-NOTES.md`](DEVICE-NOTES.md) | hardware, kernel modules, Bluetooth, WiFi, Preferences/touch, what the lab lacks | mod features |
| [`ADDIN-ECOSYSTEM.md`](ADDIN-ECOSYSTEM.md) | the 2026 addin projects, their loading/install contract, and what applies to minimod | our own mechanisms (those live in MECHANISMS) |
| [`RUST-PORT.md`](RUST-PORT.md) | the Rust rewrite: layout, lint policy, module map, every round and dead end, how each part is proven | firmware mechanisms |
| [`LIVE-LOOPER.md`](LIVE-LOOPER.md) | the Live Looper feature: state machine, latency, sync, validation | other features |
| [`HISTORY.md`](HISTORY.md) | chronology, superseded approaches, disproved leads, open investigations, the code review | current contracts (those live in MECHANISMS/FEATURES) |
| [`re/`](re/README.md) | raw evidence: Ghidra output, decompiled fragments, screenshots | prose claims |
| [`tasks/`](tasks/README.md) | backlog of unstarted feature ideas | current behavior |

`RELEASE-NOTES.md` at the repository root is the user-facing changelog, not a
mechanism reference.

## Conventions

* **Addresses.** A *mod offset* is what `FN()`, `install_hook()` and `mk` take.
  **Ghidra VA = mod offset + `0x100000`.** Runtime address = `base + mod offset`,
  where `base` is read from `/proc/<pid>/maps`. Every table states which it uses.
* **Evidence levels.** **[rt]** measured at runtime (emulator or device),
  **[hook]** observed by a live hook, **[static]** decompiled only, **[doc]**
  established elsewhere in this repository.
* **The target.** `/usr/bin/MPC Sample`, 119,243,440 B, sha256
  `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`, firmware
  1.3.0.12 only.
* **Verification.** A claim about behavior needs a measurement, not a log line
  from the mod. The test suite and its commands are listed in
  [`FEATURES.md`](FEATURES.md#build-and-verification).
* **Hardware vs emulator.** Emulator-verified and hardware-verified are always
  stated separately; only the 16-Levels pitch shift and external MIDI chromatic
  playback were verified on real hardware.
