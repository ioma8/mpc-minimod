# Handoff: minimod on the Rust crates

**Branch** `feature/live-looper`
**State** the mod is Rust only (`rust/crates/dsp`, `rust/crates/minimod`). `main`'s
consolidated docs and its C fixes have been merged in (see §1). The merge commit is
not tagged and nothing is released.

## 1. What changed on this branch

- **The C is gone.** `mod/src`, the vendored DaisySP, the golden-fixture generators
  and the host tests that compiled C are deleted. Nothing compiles C for the mod.
- **`main` is merged.** `main` kept fixing the C while this branch removed it, so a
  textual merge was impossible. Each C fix was re-implemented in the crates; the table
  in [`docs/RUST-PORT.md`](docs/RUST-PORT.md#merging-main-into-the-port-november-2026)
  maps each one to its module. The highlights:
  - Bass preview: envelope, reset, output through the exported `snd_pcm_writei` tap,
    pads consumed at the control surface.
  - In-memory audio for AutoTrim, SAMPLE OPTIONS and Spectral (the WAV is the fallback).
  - Chromatic MIDI measured from the sample's root note.
  - Hook engine: `B.cond` and read-back. Arm lifetime and writer quiescence for Extract.
  - Effect object validation. Spectral deadline and off-page cancel.
  - Effects no longer back up or restore the source sample.
- **The hook table has 16 sites**, not 17. The planar writer hook was removed with the
  bass preview fix.
- **Exports:** the cdylib exports 24 symbols: the 4 ALSA taps and 20 `g_*` statics the
  device tests read (`g_surface` was added for the root-note test).
- **Docs:** `docs/` is main's consolidated set, plus `RUST-PORT.md` (final state and the
  merge table), `LIVE-LOOPER.md`, and the Rust notes in the other docs.
- **Build:** `mod/build.sh` builds the crate as a cdylib and cross-links it with
  `zig-cc-aarch64-linux-gnu`. It fails on unresolved non-glibc symbols and on glibc
  versions newer than 2.39.

## 2. What is verified, and what is not

- `rust/check.sh` passes: fmt, clippy `all`+`pedantic` `-D warnings` with no `#[allow]`,
  74 minimod tests, 36 dsp tests, aarch64 release build.
- **Emulator, on the merged build:**
  - pass: bass sample (now also checks the live preview: notes reach it, blocks are
    mixed, the selected sample does not change, and the preview stops after its envelope),
    looper, HPSS, MIDI chromatic (deltas -28/-27/-21 at root 88), reverb.
  - **not yet verified:** spectral and AutoTrim (the runner failed on argument
    handling, not on the mod), and the menu suite (it needs Pillow, which is not
    installed in the interpreter that ran it). Re-run them before relying on this build.
- Emulator-verified and hardware-verified are kept separate. Only the 16 Levels pitch
  shift and external MIDI chromatic playback were verified on real hardware.
- **Bass preview on device:** main's C concluded the unversioned `snd_pcm_writei` does
  not interpose and patched the PLT stub `0xb51ff8`. The emulator reaches the Rust tap,
  so the stub is not used. If the preview is silent on a real device, port the stub patch
  from `main`'s `mod/src/minimod.c` (`patch_writei_plt`).

## 3. The emulator

Panel <http://127.0.0.1:8765/> · ssh `-p 2022 root@127.0.0.1`

If something else holds 5554/5555 (an Android emulator does), start the lab with
`MPCLAB_PORT_OFFSET=100 python3 lab.py run`, export the same variable for the suites and
pass `--panel http://127.0.0.1:8865`. Load the mod with
`python3 lab.py preload mod/build/minimod.so`.

```sh
cd ~/projects/customs/sample-converter-looper
mod/build.sh
python3 ~/projects/customs/virtual-mpc-sample/lab.py preload mod/build/minimod.so
python3 mod/tests/emulator_looper.py --lab ~/projects/customs/virtual-mpc-sample
```

Things learned the hard way:

- The suites need **free pads** and a **plain PCM WAV** on A01.
- Stopping the app saves the project over whatever you pushed: push the `.xpj` *after*
  `systemctl stop ac50`, then start.
- The stock Extract re-encodes the **source WAV** in place. The suites compare decoded
  audio, not bytes. Restore the file before each suite.
- Never AutoTrim a pad the suites use: the project keeps the trim.
- The guest can wedge (400 % CPU, no ssh) after hours of restarts; `lab.py stop` kills it.
- Run each suite after `systemctl restart ac50`.

## 4. Where the history lives

[`docs/RUST-PORT.md`](docs/RUST-PORT.md) (the port, every round, and the merge),
[`docs/FEATURES.md`](docs/FEATURES.md), [`docs/MECHANISMS.md`](docs/MECHANISMS.md),
[`docs/HISTORY.md`](docs/HISTORY.md), [`docs/EMULATOR.md`](docs/EMULATOR.md).
