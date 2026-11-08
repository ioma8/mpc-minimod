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
| `h3m_effect.c` (firmware effect glue) | C | — |
| `bass_sample.cpp`, `fmbass_sample.c` | C | — |
| DaisySP (SVF, Karplus, StringVoice, DC block, crossfade) | C++ | — |
| `minimod.c` (hooks, features, UI glue) | C | — |

The looper is already the one the device runs: `mod/build.sh` links the Rust
static library instead of `looper.c`, and the emulator acceptance test passes
against it.

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
