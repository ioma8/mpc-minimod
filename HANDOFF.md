# Handoff: minimod on the Rust crates

**Branch** `main`
**State** the mod is Rust only (`rust/crates/dsp` plus the public [`mpc-sample-sdk`](https://github.com/ioma8/mpc-sample-sdk),
`rust/crates/minimod`). The
port is merged; no C or C++ is compiled for the mod. Nothing at `HEAD` is tagged
(the last release tag is `v0.7.0`).

## 1. What the tree is

- **The C is gone.** `mod/src`, the vendored DaisySP, the golden-fixture generators
  and the host tests that compiled C are deleted. `mod/build.sh` is a thin wrapper around the public SDK's
  `build-mod.sh`, which builds the `minimod` crate as a `cdylib`, cross-links
  it, and fails on unresolved non-glibc symbols and glibc newer than 2.39.
- **The crates.** The public [`mpc-sample-sdk`](https://github.com/ioma8/mpc-sample-sdk) is the universal SDK: firmware image and
  the 36-word check, the hook engine and all 16 sites, the `Mod` trait and event
  decoding, the ALSA taps, drawing, and typed access to the app's page, sample,
  pad, slice, WAV, control-surface, effect and Extract objects.  `crates/minimod`
  is the `LD_PRELOAD` library and only the features, built on the SDK through
  `impl Mod` and `export_mod!(Minimod)`.  `crates/dsp` is pure audio DSP over
  slices: looper, HPSS, onset detection, FFT, the Karplus-Strong bass synth, and
  the S32↔f32 conversion.
- **The hook table has 16 sites.** `mpc_sample_sdk::sites` installs them and keeps
  every displaced original; `mpc_sample_sdk::loader` checks 36 words of the loaded
  image first, so a different firmware is refused rather than hooked at the wrong
  offsets.
- **Exports:** the cdylib exports 24 symbols: the 4 ALSA taps and 20 `g_*` statics
  the device tests read.
- **Docs:** `docs/` is main's consolidated set, plus `RUST-PORT.md`,
  `LIVE-LOOPER.md`, and the Rust notes in the other docs.

## 2. What is verified, and what is not

- `rust/check.sh` passes: fmt, clippy `all` + `pedantic` + `cargo` with
  `-D warnings`, a rustdoc build with warnings denied, a guard that
  `crates/minimod/src` contains no `unsafe` token, **39 minimod tests** and
  **41 dsp tests** in this workspace (including the golden fixtures), and the
  aarch64 release build. The standalone `mpc-sample-sdk` repo has its own
  **59 SDK tests**.
- **Golden fixtures:** the DSP is compared with the original C where the
  arithmetic is deterministic: the looper trace and loop buffers are
  bit-identical, the bass render is bit-identical, HPSS stays under 1e-4, and the
  spectral onsets match exactly.
- **Emulator, recorded green on the Rust-only build:** bass sample, looper, HPSS,
  MIDI chromatic, reverb, spectral. AutoTrim and the menu have their own scripts
  (`mod/tests/emulator_autotrim.py`, `mod/tests/emulator_menu.py`); see
  [`docs/FEATURES.md`](docs/FEATURES.md) for what each asserts.
- Emulator-verified and hardware-verified are kept separate. Only the 16 Levels
  pitch shift and external MIDI chromatic playback were verified on real hardware.
- **Bass preview on device:** main's C concluded the unversioned
  `snd_pcm_writei` does not interpose and patched the PLT stub `0xb51ff8`. The
  emulator reaches the Rust tap, so the stub is not used. If the preview is silent
  on a real device, port the stub patch from `main`'s `mod/src/minimod.c`
  (`patch_writei_plt`).

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
