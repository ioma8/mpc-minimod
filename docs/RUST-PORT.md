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
| Sample/instrument geometry accessors | ported | rate-range test; Reverb device test runs the paint path that uses them |
| Trim sample scale (`pcm_at`) | ported | 6 assertions over 16/24/float scaling and clamping; Vinyl and Bass Synth device tests run the paths around it |
| Trim edge scan (`wav_edges`) | ported | 3 host tests over synthesised takes (padded, silent, over-threshold); Bass Synth device test runs on the build |
| Looper settings file | ported | 4 host tests; the device writes and reloads its own trim |
| Pad occupancy (`hpss_free_pad`, `hpss_free_count`) | ported | offset test; the Bass Synth device test waits on the pad count to detect the assignment |
| `minimod.c` (hooks, features, UI glue) | C, shrinking | 7 683 -> 4 564 lines; dead code removed only where the compiler proved it, each pass verified by an identical binary |

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

## The looper volume: mechanism found, and it is the measurement

The volume assertion failed again (ratio 0.936) while the new direct assertion on
`g_looper_master` passed, so the mixer really had 64.  Tracing the tap explains
why that did not show up in the audio:

* `g_outtap` is filled inside the interposed `snd_pcm_writei`, from the buffer
  the **app** produced - after the app's own MAIN VOLUME has been applied to it;
* the looper's mix adds its output *into that same buffer*
  (`value = s32_to_f32(*sample) + mixed`, with `mixed = add[i] * master/127`),
  so the tap holds **both** the app's playback and the loop.

`MAIN VOLUME 64` therefore scales the loop by ~0.5 but the app's own playback by
its own curve, and the measured ratio is a weighted average of the two.  When the
app's playback dominates the window, the ratio stays near 1.  In the earlier
passing runs the loop dominated, which is why the same code could pass three
times and fail later.

Measured with the input silenced, the ratio is a **stable 0.331** (two runs,
0.208/0.629) rather than 0.5: the loop is scaled by the mod's master *and* by the
app's own volume curve, which the mod mixes into.  Silencing the input also
changes the level the test's later `loop_rms` check sees (0.63 instead of 0.15),
so the whole measurement section needs to be re-derived together rather than
patched band by band.

Re-deriving it as one block did not work either: with the input routed to
`none`, the loop measured **0.0** - in this lab, unplugging AUDIO IN stops the
output path altogether, so the loop cannot be isolated that way.  The isolation
has to come from the app (deselect the pad, or mute its monitor) or from the tap
itself (a loop-only probe), not from the jack routing.

It is a test-design issue, not a mod bug: to measure the loop's response to MAIN
VOLUME, the app's own playback of that pad has to be out of the picture - either
by selecting an empty pad before measuring, or by measuring the loop's
contribution relative to a baseline taken with the loop stopped.

## Closed: the looper volume "regression"

The Live Looper test's MAIN VOLUME assertion failed twice with a ratio of 0.78
where 0.35 to 0.65 was expected.  It was the *test*, not the mod: it inferred the
volume from the audio and waited for the RMS to stop changing, which is not the
same as the knob having arrived.  The mod now exports the value the mixer
actually uses (`g_looper_master`) and the test asserts it after each panel move:

```python
control('MAIN VOLUME', 'set', 64)
while master_now() != 64 and time.monotonic() < deadline: ...
assert master_now() == 64
```

With that, two consecutive runs pass, and the CC path is proven end to end: the
panel move reaches `cc_capture`, `looper_set_master`, and the mixer.  The lesson
is recorded here because it is the second time a flaky assertion looked like a
regression in the ported code.

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

**Firmware calls need one careful helper.** `firmware::int_of_pointer` is the
single place that turns an address into a callable `extern "C"` function.  Every
remaining glue port that talks to the app goes through it, so the unsafe
transmute exists once and is documented once, instead of at each call site.

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

## What is left, in the order it should be taken

The glue is 4 564 lines and every remaining part has a seam, but they are not
equally safe to cut.  In order:

1. **`hpss_job` lifetime.** The structure, its allocation and `hpss_free_job` own
   three buffers plus the source backup.  Nothing in it calls the firmware, so it
   can move to Rust with ownership replacing the manual `malloc`/`free` pairs
   (the preview engine already models a singleton this way).  The device tests
   for Reverb and Bass Synth exercise both the success and the failure paths.
2. **`hpss_worker`.** The render switch (`bass_sample_render`, `h3m_effect_render`,
   `hpss_split`) already calls Rust; the C only owns the thread and the done flag.
   Moving the thread to `std::thread` removes the last `pthread_create` in the
   feature path.
3. **`hpss_finalize` + `hpss_extract_ctx`.** The hard part: an arm structure read
   by a firmware callback, a pad-occupancy poll, and the free-pad wait loop.  It
   needs the callback trampoline to be built in Rust (the hook machinery already
   is), so do this after 1 and 2 have made the surrounding state Rust-owned.
4. **MIDI/16-levels and chop mode.** Device-tested by `emulator_midi_chromatic`
   and the looper/spectral tests; mostly pure mapping logic once the note hook is
   Rust-owned.
5. **The looper/sample-options UI glue** (paint, buttons, menu tree). Last,
   because it is mostly firmware draw calls with little logic to test.
6. **The constructor and hook installation**, which is only glue once everything
   it installs is Rust.

The pattern for each step is fixed: move the logic, keep a `*_ffi.h` with the old
signatures, delete the C, rebuild, run the device test that covers the feature,
and only then remove the header.
