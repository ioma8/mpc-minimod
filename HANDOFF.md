# Handoff: the Rust port of minimod

**Branch** `feature/live-looper` · **last commit** `3430e6c`
**State** behaviour fully ported and device-verified; a C glue layer remains.

This file is the entry point for whoever picks the port up next. Everything below is
either measured on the emulator or read from the tree at `3430e6c`. Nothing here is a
plan that has not been tried unless it says so.

---

## 1. What the mod is, in one paragraph

`minimod` is an `LD_PRELOAD` library for the Akai MPC Sample firmware 1.3.0.12. It finds
the app's load address, patches sixteen entry points with `bldr x16 / br x16`
trampolines, and adds: a live looper, HPSS stem extraction, spectral chop, a flexible
trim, a bass synth, MIDI chord/16-Levels handling, and an overlay menu drawn on top of
the stock UI. The Rust port replaced the C implementation piece by piece while the mod
stayed shippable at every step.

---

## 2. Current numbers

| | |
|---|---|
| C in `mod/src/minimod.c` | **1 139 lines, 534 of code, 25 functions** |
| Rust in `rust/crates/minimod/src/` | **45 modules, 627 functions** |
| Unit tests | 120, all passing |
| Device suites | 6 (`mod/tests/emulator_*.py`), **5 green**; spectral blocked, see §7 |
| Lint gate | clippy `all` + `pedantic` + `cargo` with `-D warnings`, **no `#[allow]` anywhere** |
| `docs/RUST-PORT.md` | 983 lines, the full history and every elimination |

---

## 3. What is Rust now

Everything with behaviour:

- **install**: `glue::minimod_load` finds the app, checks the image against the firmware
  digest, reads the saved calibration, walks the site table, patches each site with
  `minimod_install_hook`, records each displaced function, installs the paint hook
- **all four hook tables**: job (`jobflow::glue_hooks`), menu, buttons (`ui`), overlay
- **every app entry point**: buttons, page paint, MIDI note event, the slice command's
  constructor and method, both ALSA taps, the channel negotiation
- **the behaviour**: looper, HPSS, spectral chop, trim, bass synth, menu and its rows,
  overlay drawing, jobs and their flow, config, memory window, firmware checks

The C file no longer contains a single handler or table. Its 534 code lines are the
accessors Rust calls plus four small helpers.

---

## 4. What is still C, and what each piece needs

25 functions. None is behaviour; every one is a join between a C-shaped call and a Rust
module.

### 4.1 Accessors the crate calls (11)

| function | lines | what blocks it |
|---|---|---|
| `minimod_glue_set_originals` | 12 | assigns six C globals from a struct; the globals are read by C callers that are themselves mostly gone |
| `minimod_glue_cc_context` | 15 | builds a C struct from the app's object graph |
| `minimod_glue_context` | 5 | same, for the input path |
| `minimod_glue_finished` | 3 | one `mlog` call |
| `minimod_glue_save`, `_selection`, `_set_base`, `_cc_notice` | 3 each | one-line forwards |
| `looper_cfg_save` | 3 | writes two values through `minimod_cfg_save` |
| `bass_preview_set_active`, `hpss_restore_chop`, `hpss_repaint`, `hpss_chop_mode`, `hpss_finished`, `looper_pump`, `sample_page_visible`, `bass_preview_off`, `do_autotrim` | 2–3 each | one-line forwards; **most have no caller left and can simply be deleted** |

### 4.2 Real helpers (4)

| function | lines | notes |
|---|---|---|
| `mlog` | 27 | variadic; cannot be Rust as written. 44 call sites in the C file only. Leaves with the file |
| `input_context` | 11 | fills a C struct from the app's objects |
| `hpss_close` | 9 | menu close; its Rust twin (`menu::minimod_menu_close_self`) already exists and is used |
| `set_filter_text` | 6 | renames a soft key by finding a live `juce::String` |

### 4.3 The honest estimate

At the pace that works (see §6), the remaining 25 are **12–18 rounds**. Fourteen of them
are one-line forwards or already-dead functions and could go in two rounds by sweeping
with the reference audit in §6.3.

---

## 5. The emulator, and how to run it

**Panel: <http://127.0.0.1:8765/>** · ssh `-p 2022 root@127.0.0.1`

```sh
cd ~/projects/customs/sample-converter-looper
mod/build.sh                                    # builds build/minimod.so
scp -P 2022 mod/build/minimod.so root@127.0.0.1:/run/mpclab-preload/minimod.so
ssh -p 2022 root@127.0.0.1 'systemctl restart ac50' && sleep 33
python3 mod/tests/emulator_looper.py --lab ~/projects/customs/virtual-mpc-sample
```

### The lab lives in `/run` and dies on reboot

After any reboot the preload is gone and the app runs **unmodded** — every test then
fails for the wrong reason. Re-create it:

```sh
ssh -p 2022 root@127.0.0.1 'mkdir -p /run/systemd/system/ac50.service.d /run/mpclab-preload &&
  printf "[Service]\nEnvironment=LD_PRELOAD=/run/mpclab-preload/minimod.so\n" \
    > /run/systemd/system/ac50.service.d/mpclab-preload.conf &&
  systemctl daemon-reload'
```

### The sequencer wedges after a failure

Any crashing run leaves the ALSA MIDI port stuck
(`Device not found on port 'Control Surface: Control Surface 20:0'`) and **every later
probe fails identically**. Reboot before believing a failure. Check the mod is actually
mapped before concluding anything:

```sh
ssh -p 2022 root@127.0.0.1 'p=$(pidof "MPC Sample"); echo pid=$p maps=$(grep -c minimod /proc/$p/maps)'
```

`maps` must be > 0. If the app is not running at all, read
`journalctl -u ac50 -n 20` for `symbol lookup error` — that is a missing symbol, not a
crash, and it is always a `static` C function that Rust names or a stale `extern`.

---

## 6. How this session actually made progress

Recorded because it cost a dozen rounds to learn and it is the most reusable thing here.

### 6.1 Slice, then verify — never bundle

| rounds | approach | outcome |
|---|---|---|
| 69, 70, 77, 85 | 4–20 functions in one build | **all four reverted** |
| 71–76, 78–84, 86–88 | one function, one build, one device run | **all landed** |

The code was the same quality. The difference is that a bundle leaves no budget to bisect
when the device says no, and the device always finds something.

### 6.2 Read constants from their source

Nine of the sixteen site offsets were reconstructed from the Ghidra addresses by
subtracting the image base instead of read from the `#define`s. Every constructor attempt
for thirteen rounds patched nine wrong addresses and died silently. One `grep` would have
prevented all of it. **Never infer a constant that exists in the tree.**

### 6.3 Check for duplicate statics before moving one

Three silent failures came from a static that existed in both languages: `base` (round
82, paint install returned 0), `g_lat_extra` (round 85), and it was only avoided for
`g_menu`/`g_spectral`/`g_bass_knob` by moving them early. When the crate takes a static,
the glue's copy must become `extern`. Before starting:

```sh
grep -rn 'g_your_static' mod/src/minimod.c rust/crates/minimod/src/
```

### 6.4 Sweep for dead C with a reference audit

After each table moves to Rust, its C counterparts become dead but still compile. Find
them:

```sh
python3 - <<'PY'
import re, glob
s = open('mod/src/minimod.c').read()
rust = ''.join(open(f).read() for f in glob.glob('rust/crates/minimod/src/*.rs'))
for m in re.finditer(r'^(static )?[a-z][^\n;=]*?\b([a-z_0-9]+)\([^\n]*\)\n\{', s, re.M):
    name = m.group(2)
    calls = len(re.findall(r'\b' + re.escape(name) + r'\s*\(', s)) - 1
    addr = len(re.findall(r'\b' + re.escape(name) + r'\b', s)) - 1
    if calls == 0 and addr == 0 and name not in rust:
        print(name)
PY
```

Round 80 removed eighteen functions this way with **no change in the built `.so` size** —
proof they were already compiled out, and proof the sweep is free.

### 6.5 Instrument before trusting

The constructor had failed fourteen times. One run with a log file answered it in a
minute: the patch loop was fine all along, and `install_screen_paint` was returning 0
because the glue's `base` was never set. Write the intermediate values to a file; do not
reason about them.

---

## 7. Open items

### 7.1 Spectral is unverified

`emulator_spectral` asserts that pad **A13** holds `GuitPh-Grove3` with exactly **62 259**
frames. The sample file is intact in the project
(`/data/TempProjects/Temp-Project_[ProjectData]/GuitPh-Grove3 E85 91V.wav`), but the pad
binding was overwritten by an earlier test run that rendered into a free pad. **Load that
sample on A13, then run the suite.** Until then the suite is not evidence either way.

### 7.2 `mlog` cannot be ported as-is

Variadic, 27 lines, 44 call sites, all inside the C file. It disappears when the file
does. If the C file must go first, the call sites have to be replaced by a macro that
expands to nothing in release.

### 7.3 The remaining `minimod_glue_*` accessors are the real work

Everything else is deletion. Those eleven expose C's view of state the crate already
owns; each one goes by moving its state to Rust and making the glue's copy `extern` —
the method in §6.3, one per round.

---

## 8. Verification commands

```sh
# lint + unit tests
cd rust && ./check.sh            # clippy all/pedantic -D warnings, then cargo test

# one device suite
python3 mod/tests/emulator_looper.py   --lab ~/projects/customs/virtual-mpc-sample
python3 mod/tests/emulator_bass_sample.py --lab …
python3 mod/tests/emulator_hpss.py     --lab …
python3 mod/tests/emulator_midi_chromatic.py --lab …
python3 mod/tests/emulator_spectral.py --lab …      # needs the A13 fixture
python3 mod/tests/emulator_reverb.py   --lab … --effect reverb

# the SD package
mod/pack.sh
```

A suite takes ~40 s plus a 33 s restart. Reboot between failures — see §5.

---

## 9. Where the history lives

- **`docs/RUST-PORT.md`** — 983 lines: every round, every elimination, every dead end,
  including the twelve constructor attempts and what each one ruled out.
- `docs/MINIMOD.md`, `docs/MOD-REFERENCE.md` — what the mod does and the offsets it uses.
- `docs/HPSS-RE-FINDINGS.md`, `docs/SPECTRAL-CHOP.md` — the reverse engineering behind
  the two heaviest features.
- `docs/EMULATOR.md` — the lab itself.

If you read one thing before touching the port, read `docs/RUST-PORT.md` §"A serious
finding: the table's offsets were invented".
