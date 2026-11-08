# Rust port

The mod is being rewritten from C and C++ into Rust: idiomatic, clippy-clean with
no inline lint annotations, host-tested against the original C as the reference,
and validated on the emulator with the same acceptance tests as before.

This document is the working record: what is ported, how each piece is proven,
and what is left.

## Layout

```
rust/
  crates/dsp       audio DSP.  Pure computation over slices: no firmware
                   addresses, no globals, no FFI, host-testable.
  crates/minimod   the LD_PRELOAD library: C ABI, hook engine, feature glue.
  tests/golden     fixtures recorded from the shipped C implementation, plus
                   the generator that records them (generate.sh).
  scripts          the aarch64 cross linker driver (zig).
  check.sh         fmt + clippy + tests + cross build, one command.
```

The DSP crate is where the risk lives (sample-exact behaviour on a real device),
so it is validated first and independently of the firmware glue.

## Lint policy

`check.sh` runs `cargo clippy --all-targets --workspace -- -D warnings` with
`clippy::all`, `clippy::pedantic`, `clippy::cargo` and a few extra `rustc` lints
(`missing_docs`, `unsafe_op_in_unsafe_fn`, `unreachable_pub`,
`unused_qualifications`) denied.

There is **no `#[allow(clippy::…)]` or `#[expect(clippy::…)]` anywhere** in the
crates. Two lints are turned off once, in the workspace lint table, because they
describe the audio boundary rather than a style choice:

* `cast_possible_truncation` — the device hands over signed 32-bit PCM frames
  and every buffer inside is `f32`; narrowing is required;
* `cast_precision_loss` — buffer lengths and frame indices become `f32` for the
  analysis windows.

Both are funnelled through `crates/dsp/src/sample.rs` and `num.rs`, which document the exact
rounding. Every other cast is exact, and `cast_sign_loss` and
`cast_possible_wrap` stay denied, which is why conversions from `i64`/`f32` to
unsigned sizes go through explicit `try_from(...).unwrap_or(...)`.

Panics are not allowed to reach the firmware: `panic = "abort"` in the release
profile, and the FFI layer validates every pointer and length before it forms a
slice.

## Status

| part | state | proof |
| --- | --- | --- |
| FFT (radix-2, forward/inverse) | ported | round-trip and pure-tone tests |
| HPSS split | ported | golden fixture, peak < 1e-4 |
| Spectral Chop onsets | ported | golden fixture, positions identical |
| Live looper engine | ported | golden fixture, **bit-identical** over 122 542 frames |
| Sample conversion (S32 ↔ f32) | ported | golden fixture, bit-identical |
| Firmware core: load base, `/proc/self/mem` access, branch hooks | ported | unit tests for the encodings; the loader walk cross-checks the C base on the device |
| Firmware effects: reverb, vinyl, decimator | ported | all three emulator effect tests |
| Bass synth: Karplus string, SVF, DC blocker, crossfade, dust | ported | golden fixture, **bit-identical** over 132 300 samples; emulator bass test |
| Realtime bass preview (4 voices, 16 pads) | ported | unit test: a strike is audible, decays, both channels match |
| `fmbass_sample.c` (FM bass preset renderer) | removed | never compiled and never called; the Bass Synth action renders with `bass_sample_render` |
| WAV reading, source backup/restore | ported | 5 host tests over synthetic files; the effects and bass device tests run on it |
| Sample-path resolution (object graph, unique/newest file) | ported | 6 host tests; the effects and bass device tests run on it |
| Status line (action names, formatted messages) | ported | 3 host tests; the bass device test reads the string out of the process |
| `minimod.c` (hooks, features, UI glue) | C, shrinking | 7 683 -> 4 774 lines; every removal verified by an identical binary |

The looper, the effects and the whole bass synth are already what the device
runs: `mod/build.sh` links the Rust static library instead of the C engines, and
the emulator acceptance test for each passes against it.  HPSS and Spectral Chop
have Rust implementations validated by their fixtures, but the device still calls
the C ones — they share `minimod.c`'s worker and are wired in when that glue
moves.

## How the looper is proven

`tests/golden/generate.sh` compiles the **shipped C sources** and records a
scripted performance: record, stop, replay, overdub, sync pulse, a quantised
second loop, stop-all, half master volume, clear, close. It writes the input
frames, the rendered output, both loop buffers and the final `looper_view`.

`cargo test` replays exactly the same call sequence through the Rust engine and
compares:

* the loop buffers sample for sample,
* the whole output trace **bit for bit**,
* the `looper_view` struct field by field, and its size and offsets against the C
  struct (`332` bytes, pads at `76`), because the paint hook reads it across the
  C ABI.

That is the strongest check available without hardware: if the engine made a
different decision, mixed at a different level, captured one frame less of the
latency tail or quantised a length differently, the trace differs.

## How the glue moves

`minimod.c` is one translation unit with shared state, so it cannot be split
along module lines.  Instead, self-contained areas move out one at a time and the
C keeps calling them through a small header of C prototypes
(`src/wav_ffi.h` is the first): the call sites do not change, the C definitions
are deleted, and the header disappears with the file.  Each area is chosen so an
existing emulator test already covers it — the WAV layer, for instance, is on the
path of every sample action, so the Reverb and Bass Synth tests validate it on
the device.

## What was deleted rather than ported

Two kinds of code in the glue were never going to be Rust:

* the `#if MINIMOD_DEBUG` forensic scaffolding (probes, watch points, heap and
  broadcast dumps) - 1 050 lines that the release build already compiled out;
* the 63 functions and globals the compiler proved unreachable once that
  scaffolding was gone (~1 270 lines), which were the leftovers of the
  reverse-engineering sessions.

Both passes were checked by rebuilding: the resulting `.so` is **byte-identical**
(`b47b7971c7ee8ac2`) before and after, so 2 320 lines left the glue and not one
instruction changed. The device artifact is the same binary the round-6 tests ran
against.

## Findings from the port

**Fused multiply-add.** The C build contracts `add[i] += sample * gain` into an
FMA (`fmla`, verified in the object file), so mixing two loops rounds once, not
twice. The Rust engine uses `f32::mul_add` for the same expression; without it
the golden trace differs by one ulp on ~22 % of the frames in the two-loop
segment — inaudible, but a real behavioural difference. `mul_add` is also a
single instruction, so the port is faster.

**Globals the tests read.** The emulator test resolves symbol offsets in
`minimod.elf` and reads memory through `/proc/<pid>/mem`, so the engine keeps
exporting `g_pads`, `g_pad_stride`, `g_open`, `g_click`, `g_sync` and
`g_sync_frames` under their original names (`#[unsafe(export_name = …)]`). The
pad layout stays `#[repr(C)]` with `state`, `len`, `rec`, `lat` as the first four
words of each 40-byte entry.

**`char` is unsigned on aarch64 Linux.** `looper_notice(const char *)` takes
`*const c_char`, not `*const i8`; the host build hides the difference, the cross
build does not.

**A bug the port exposed.** `hpss_finalize` demanded a *source* restore after
every action, but only the Bass Synth path ever takes a source backup — the
effect path left the path empty, so `open("")` failed and every Reverb, Vinyl
and Lo-Fi action reported "source restore failed" and did nothing.  The restore
is now guarded by the backup's presence, the same way the bass path already was,
and all three effects pass again.

**A mask fix in the hook classifier.** The reference `pc_relative` masked only
bit 10 for `RET`, which matches `ret x0` and no real `ret` (the register field
sits in bits 5-9).  The port masks the register field, so `ret` and `ret xN` are
both refused as trampoline material.  That can only make the displacement window
shorter, never wrong.

**The note frequency is a bit pattern.** `bass_sample.cpp` tunes to
`65.4063913f`; writing `65.40639` in Rust is one ulp lower, and that one ulp
detunes the whole string (the delay is derived from it), which showed up as a
1e-6 offset in every sample of the golden trace.  `NOTE_HZ` is therefore
`f32::from_bits(0x4282_d013)` with a comment saying why.

**The reference has to be built without FMA contraction to be comparable.** The
Rust port never contracts `a * b + c`; clang does at `-O2`.  For the synth
fixture the reference is compiled with `-ffp-contract=off` so both sides evaluate
the same expression, which is what makes a bit-exact comparison meaningful.  (The
looper fixture goes the other way: its reference *is* contracted, and the Rust
mix uses `mul_add` to match it.)

**The unwinder is needed even with `panic = "abort"`.** The Rust standard library
object references `_Unwind_Resume` and friends from its panic hook's backtrace
path.  Without `-lunwind` the library links but the firmware refuses to start the
app (`undefined symbol: _Unwind_Resume`, exit 127).  Linking it costs ~20 KB
after `--gc-sections` and strip.

**A dead branch was hiding in the path lookup.** `holder_wav_path` asked for a
WAV with *zero* frames, which no file can satisfy, so the branch in
`find_displayed_sample` that used it could never fire; it is removed rather than
ported, and the function now reports only the frame count it can actually read.

**The sampler's own recordings are not at fixed offsets.** They carry a `JUNK`
chunk in front of `fmt `, so both the format and the data chunk have to be found
by walking the RIFF chunks, and the `fmt ` payload can be 16 or 40 bytes
(`WAVE_FORMAT_EXTENSIBLE` stores the real tag in its sub-format GUID).  The Rust
parser shares one walk between the header question and the decode, which is what
makes the two answers impossible to disagree.

**Linker noise.** rustc asks the linker for `-Wl,-O1`; lld has deprecated it and
zig forwards it verbatim, so every cross build printed a warning. The cross
driver in `scripts/` drops that argument — it changes nothing in the output —
which keeps the build silent without touching a lint.

## Working on it

```sh
rust/check.sh                       # fmt, clippy, tests, cross build
rust/tests/golden/generate.sh       # re-record the fixtures from the C sources
cd mod && ./build.sh && ./pack.sh   # the SD package
```

When a feature moves to Rust, `mod/build.sh` drops its C object and links the
Rust static library in its place, and the emulator test for that feature becomes
the acceptance gate. The C sources stay in `mod/src` until the whole mod is Rust
because they are the reference the golden fixtures are recorded from.
