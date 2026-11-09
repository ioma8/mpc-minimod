# Handoff: the Rust port of minimod

**Branch** `feature/live-looper`
**State** the port is complete and **all six device suites pass**: `mod/src/minimod.c` and
every `*_ffi.h` are gone and the shipped `.so` is the Rust `cdylib`. See §2.

## 1. What changed in the last rounds

- The C glue is gone: `minimod.c`, every `*_ffi.h`, and then the rest of the C and C++
  (`mod/src`, the vendored DaisySP, the golden-fixture generators, four host tests that
  compiled C). Nothing in the repo compiles C for the mod; `tools/` and `docs/re/` still
  hold unrelated emulator and decompilation C.
- `mod/build.sh` builds the `minimod` crate as a `cdylib` and cross-links it. It fails on
  any unresolved symbol of ours and requires the symbols the tests read.
- **Exports were cut from ~250 to 23**: the four ALSA taps and the 19 `g_*` statics the
  device tests read. Everything else is an ordinary Rust item; dead items found by making
  the modules private for a build (about 120 functions, constants and two modules,
  `props` and `status_ffi`) were deleted, along with the tests that only tested them.
  The `.so` shrank from 562 KB to 487 KB.
- Docs: `docs/RUST-PORT.md` opens with the final state and a C-to-Rust module map; the
  docs that describe the removed C carry a "Historical" banner.
- Four real regressions were found and fixed on the way, all invisible to the unit tests:
  AutoTrim's control-surface capture (commit `317a9bd`), the B3 release routing, the
  missing recorder for the button dispatcher (site 8), and **the slice command's execute
  hook returning nothing** (see below). See §2.

## 2. What is verified, and what is not

- `rust/check.sh` passes: fmt, clippy `all`+`pedantic` `-D warnings` (no `#[allow]`),
  74 unit tests, aarch64 release build.
- **Device: looper, bass, hpss, midi, reverb and spectral all pass** on the final Rust-only
  `.so`.
- **AutoTrim verified by eye** (A06 and A13): the toasts read "Trim 3 % / 6 % / 9 % of
  peak" then "Trim cleared", the End marker moves, SHIFT release restores the soft-key
  labels. No automated test (`emulator_autotrim.py` is stale).
- **Third port regression found and fixed, the one that blocked spectral:** after commit
  `999007b` nothing recorded the app's original button dispatcher (site 8), so the hook had
  nothing to forward to and **every button the mod does not handle itself was silently
  dropped** (CHOP, SHIFT, SCREEN, ...). `record_site` now handles site 8, reports whether it
  handled a site, and `every_site_has_a_recorder_for_its_displaced_function` fails if a
  site is added without one.
- **Fourth: HPSS (and every Extract) showed "Command failed / Extract slice failed", and OK
  then left the panel stuck.** The app's slice `execute` (`0x143f0b0`) returns an int status
  that its caller checks; `hpss_exec_entry` was typed to return nothing, so the caller read
  leftover register garbage and reported failure *after the work was done*. Worse, the failed
  status made the app abort its in-place re-encode of the source sample halfway, which left
  WAVs with the `data` chunk tag overwritten by `atem` (the "Sample not found" dialogs and
  never-loading waveforms seen earlier in the lab). Present since the C glue (it was also a
  `void` hook there). Fixed: the hook returns the app's own status. `set_control` (site 16),
  which tail-calls the app's setter, now forwards the result too. Every other hooked function
  was checked for a returned value by disassembly: only `execute` and the 16 Levels
  constructor (already returned) set one.
  The suites missed it because they read the mod's own status string, which said "created".
  `mod/tests/panel_screen.py` now lets hpss, bass and reverb assert that no app dialog is on
  the panel afterwards; with the hook forced to fail, `emulator_hpss.py` fails on it.
- The app re-encodes a source sample it extracts from (24-bit PCM -> 32-bit float, plus a JSON
  metadata chunk) when the command succeeds, so `emulator_bass_sample.py` compares the decoded
  audio of the source, not its bytes. Restoring the source file before each suite is still
  the safe way to start.

## 3. The emulator

Panel <http://127.0.0.1:8765/> · ssh `-p 2022 root@127.0.0.1`

If something else holds 5554/5555 (an Android emulator does), start the lab with
`MPCLAB_PORT_OFFSET=100 python3 lab.py run`, export the same variable for the suites and
pass `--panel http://127.0.0.1:8865`. Load the mod with
`python3 lab.py preload mod/build/minimod.so` (`preload -- --off` removes it).

```sh
cd ~/projects/customs/sample-converter-looper
mod/build.sh
python3 ~/projects/customs/virtual-mpc-sample/lab.py preload mod/build/minimod.so
python3 mod/tests/emulator_looper.py --lab ~/projects/customs/virtual-mpc-sample
```

### The lab project (rebuilt after a reset)

The original eMMC's project is gone. The current one is the factory `Temp-Project` with
`Temp-Project.xpj` edited (gzip + ACVS header + JSON): pads 17-128 blanked, the sample
lists pruned to files that exist, A01 = `RnB-Kik-Grove1 91V.wav` and A13 =
`GuitPh-Grove3 E85 91V.wav` with their instruments copied from
`converter/project1/bapap1.xpj`. Things learned the hard way:

- The suites need **free pads** and a **plain PCM WAV** on A01 (the factory samples have a
  header the mod's "exact WAV" check rejects).
- Stopping the app saves the project over whatever you pushed: push the `.xpj` *after*
  `systemctl stop ac50`, then start.
- The stock app's Extract/assign re-encodes the **source WAV** in place (see §2; before the
  execute-status fix it left the file corrupted instead). Restore the file before each suite.
- Never AutoTrim a pad the suites use: the project keeps the trim.
- The guest can wedge (400 % CPU, no ssh) after hours of restarts; `lab.py stop` kills it.
- A failed spectral run can leave the mod's chop state armed: restart `ac50`.
- Run each suite after `systemctl restart ac50`; the lab is a long-lived VM and can wedge.

## 4. Where the history lives

`docs/RUST-PORT.md` (every round and dead end), `docs/MINIMOD.md`,
`docs/MOD-REFERENCE.md`, `docs/HPSS-RE-FINDINGS.md`, `docs/SPECTRAL-CHOP.md`,
`docs/EMULATOR.md`.
