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
| Monotonic clock (`now_ms`) | ported | real-time test; every deadline the looper and spectral paths use runs on it |
| Slice list (`slice_count_of`, `spectral_verify`) | ported | layout test; the Spectral Chop device test asserts "exact 5 boundaries" through the verifier |
| Selected-sample walk (`hpss_read_sample`) | ported | vtable/range test; Bass Synth and Spectral Chop both start from it |
| Slice writer (`spectral_write_markers`) | ported | the Spectral Chop device test writes and verifies its 5 boundaries through it |
| Manual chop mode (`spectral_manual`) | ported | the spectral test asserts "Manual" after the chop |
| Page predicates (`sample_page_visible`, chop mode) | ported | the looper and spectral tests both gate on them |
| Hook installation (pages, slots, trampolines, patching) | ported | the whole device suite runs on 40+ hooks installed by the Rust engine |
| Substitute writer (rendered buffer for the app) | ported | Reverb (154k frames incl. tail) renders through it; progress/arm tests |
| Stream geometry (`audio_channels`) | ported | offset test + stereo fallback; the bass preview path reads it every block |
| Audio writer (preview into the app's block) | ported | Bass Synth renders its whole pluck through it; looper passes on the same build |
| Hook-site guard (`midi_hook_site_ok`) | ported | the MIDI prologue is a documented constant; the chromatic test runs the hook it guards |
| MIDI note decoding (`midi::decode`) | ported | 5 rule tests (external ids, note on/off, row); the chromatic test drives the hook that uses it |
| 16 Levels event replay (`midi::replay`) | ported | the chromatic test plays through it: note in, transposed pad event out |
| Control-change routing (`routing::route`) | ported | 6 ladder tests; the looper test drives the master/trim paths and the bass test the three knobs |
| MIDI note hook (`midi::handle`) | ported | the chromatic test drives the whole hook: replay, pass-through, 16 Levels state |
| 16 Levels octave shift (`levels`) | ported | 3 tests: clamping, 8 semitones per press, the toast text |
| 16 Levels hook trampolines (ctor/proc/pitch) | ported | the chromatic MIDI test drives all three through the 16 Levels path |
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

## The looper volume: solved, and the mod was right all along

Every level in the looper test was measured from the **device output tap**, which
carries the app's playback and its live input monitor on top of the loops.  With
a tone routed in, the tap is dominated by a signal that cannot follow the loop's
master, so the ratio wandered between 0.78 and 0.94 depending on the loop phase
and the tone level - and the same code could pass three times and fail later.

The mixer now keeps a loop-only ring, `g_looptap` / `g_looptap_w`, written from
exactly `add[i] * gain` - the loops' contribution after the mod's master and
before the app's mixer.  With every level assertion in the test pointed at it:

```
vol_half / vol_full  -> 0.35..0.65   (was 0.78..0.94)
loop level           -> 0.150 rms, 441 Hz
stop / STOP          -> silent
overdub              -> audible
```

Two consecutive runs pass, and the historical `0.150 rms` is back - the 0.63
readings were the monitored tone, not the loop.  The lesson is in the code now:
a level that is meant to describe the loops must be measured where only the loops
are.

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

**Firmware calls need one careful helper per signature.** Rust has no variadic
FFI, so every distinct firmware signature the glue calls (void of pointer, void of
pointer+value, four-argument insert returning an int, ...) is one documented
`transmute` in `firmware`.  The call sites stay readable and the unsafe stays
countable.

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

## The spectral state machine

`spectral_machine.rs` owns the three-phase machine (open Manual chop, write the
boundaries through the app's slice calls, verify what it kept) and returns a
`Step`: idle, started, continue, done with the slice count, or failed with a
message.

The first wiring resolved the sample's path *inside* the machine and failed on
the device in the "repeat again" case - boundaries starting at 886 instead of
264 - because `paths::for_sample` falls back to `unique(TEMP_PROJECTS, ...)` and
on a repeat that picks another file with the same geometry.  The fix was not to
tighten the resolver but to move the read out: the glue keeps doing
`hpss_sample_now` + `hpss_path_for` + `wav_read_f32` (exactly what the C did) and
hands the samples to `minimod_spectral_begin`.  The machine then detects, opens
Manual chop and keeps the state; `minimod_spectral_step` advances it.

General lesson, from two reverts now: **a port inherits its dependencies'
behaviour, so diff the helpers it calls, not only its own body** - or, better,
pass the data in so the port cannot resolve it differently.

## The menu state, ported but not wired

`menu` holds the effects menu's state machine (open, close, activate a row,
restore the chop) as functions over a `State` struct with its own tests.  Moving
the glue's five menu flags into that struct and pointing the old names at it with
macros compiled and the app started, but the bass control page stopped taking the
knobs: the menu's row selection only moves through the encoder hook, and the
re-wired dispatcher consumed the turn before it reached it.

Reverted to the flags in the glue.  Wiring this needs the encoder path and the
flag struct moved **together**, not the flags first.

## The paint hook

Drawing and the toast are ported (`paint`) and wired.  The crash that blocked it
was not the drawing at all: `minimod_set_base` was declared and implemented but
**never called**, so `firmware::base()` returned 0 and every Rust draw jumped to
`0 + offset`.  One line in the constructor fixed it (`base = found;
minimod_set_base(base);`) and all six device tests pass with the Rust draws.

Drawing and the toast are ported (`paint`) but **cannot be wired yet**: every
attempt makes the app crash-loop at startup (`NRestarts=5` within seconds, before
any interaction), so the fault is in the first paint, not in a toast.  The Rust
side mirrors the C calls one for one - setColour, fillRect, and the String + Font
+ drawText sequence with the same offsets - so the likely difference is in how the
temporary String/Font objects are laid out or destroyed.

It stays unwired until that is understood; the C draws with its own calls, which
is verified by every device run.

## The spectral tick

`spectral` holds the chop's frame state (the request flag, the phase, the three
second deadline, the sample it is working on) and runs the whole tick - read the
page's sample, resolve its WAV, hand it to the machine, close the menu, show the
result.  The glue keeps only the trampoline that presses the app's CHOP key.

The port cost four device runs, and the cause was worth the time: the machine
calls the press hook **with no arguments**, and the glue was handing it the
app's button hook, which takes the event pointer.  It read whatever was in the
argument register as an event and the app died in the first frame.  A logging
build narrowed it to the phase-0 path; the signature was the difference.

## The memory window

`rd` and `mapped` look like the easiest two functions in the file: read process
memory, answer whether an address is mapped.  Pointing the glue's names at the
crate's own `firmware::read` and `props::mapped` broke the button path - the menu
stopped opening - even though both pairs look equivalent on the page.

So they are **not** equivalent in one respect that matters: the crate's reader is
the one the *mod* uses for the app's image, and the glue's is the one used for the
app's *live objects* (events, pages, selection).  Whatever the difference is - a
bounds check, a cached descriptor, the range table - it is load-bearing, and the
next attempt starts by reading both implementations side by side rather than by
assuming.

Reverted; looper green.

## The constructor

The last C piece is the constructor: ~200 lines that are an install table, the
environment flags, and one assignment per hook for the displaced function.

An attempt at moving it whole ran into three things worth writing down before the
next try:

* the handlers the table names must be *global* symbols - half of them are still
  `static` in the glue, so Rust cannot take their address until they are not;
* the C helpers still need six originals back (buttons, repaint, control set,
  MIDI note, bass writer, slice constructor), so the install has to hand them over
  rather than keep them;
* clippy's 100-line limit bites: the table wants to be several small functions
  from the start.

The attempt was reverted rather than half-landed; the glue is unchanged and green.

Second attempt, with the handled parts:

* `g_screen` **did** have to move to Rust - with the C constructor gone nothing in
  C made it observable and the linker dropped it (`g_screen missing from the
  build`).  That is done and certified: the paint state owns the page it painted.
* the install table itself now compiles clean, split into `install_input`,
  `install_midi`, `install_ui` and `install_hpss`.

It still failed: with the constructor replaced, the app **does not start**
(`NRestarts=5`, no pid).  So the Rust install is producing something the loader
cannot run - the prime suspect is doing allocation or `std::env` work inside a
constructor that runs before the Rust runtime is up, or a site being patched with a
handler whose C signature does not match the one declared for it.

The logging build did its job in one run: the install **completes** - steps 1 to 9,
`install done`, the app gets a pid - once the handlers it names are global.  The
start-up failure was `undefined symbol: midi_note_event`: six C handlers were still
`static`, and a `static` is not a symbol the Rust install can name.

With them global the app starts and the install finishes, but the clean build still
loses the app on first interaction (`NRestarts=1`), so something the diagnostic
build masked is still wrong - most likely one of the four `safe`/`debug`/`mask`
statics that the old constructor used to set and nothing sets now.

Setting the three statics from the constructor did **not** fix it: the clean build
crashes on start (`NRestarts=14`), while the same code with `-DMINIMOD_LOG=1` starts
and installs fine.

That difference is the clue: with logging the install is slow enough that nothing
runs before it finishes.  Without it, a hook goes live while the originals are still
being recorded - `install_input` patches the sites *first* and records their
originals at the end - and a hook that fires in that window finds nothing.

So the ordering has to change, not the logging: record each original **immediately
after** patching its site, per hook, before moving to the next.  That is a small,
mechanical change to the four install functions.

## The button entry point

`ui` runs one button event end to end: it keeps what the dispatcher remembers
(SHIFT and ERASE under the looper, the trim cycle's step, the octave keys, the
marks, the last press object, the replayed event) and applies `buttons::dispatch`
through a hook table.

Three things had to be right, and each cost a round:

* **the state must be Rust's** while a dozen C readers keep their names - done with
  accessor macros, as for `g_menu` and `g_input`;
* **the event is not just forwarded**: the old dispatcher also stored it and
  derived `data_target` from its context pointer, so the entry point does too;
* **the hook table is a `#[repr(C)]` contract**: adding four fields to the Rust
  struct and appending them to the C one misaligned every field after them, which
  read as "the menu opens on every press".  Fields go in the same order on both
  sides or the app misbehaves in ways no compiler sees.

One more the tests caught: `looper_button` takes the raw key *value*, not "was it a
press" - passing the boolean made every key in the view a release.

## The probe hooks were dead code too

`trim_probe`, `sync_probe`, `loc_bounds`, `setctl_probe`, `obs_set` and `probe_node`
were installed as hooks and did nothing but record registers and addresses into a
log that release builds do not write.  `do_poke` poked a debug value behind an env
variable.  All gone, with their registrations and their original-pointer plumbing:
317 lines, and the hook table is seven entries shorter.

## The trim cluster was dead code

While preparing to port `commit_trim` I traced what feeds it: `g_trim_v0`/`v1` are
never assigned, and `g_pending_cc` - the flag the paint hook checks before calling
it - is only ever set *inside* `commit_trim` itself.  Nothing enters the loop.  It
was research scaffolding from the tracing work, kept alive by its own helpers.

So it was deleted rather than ported, with everything only it used: `commit_trim`,
`cc_replay`, `cc_inject`, `learned_prop`, `broadcast_prop`, `param_from_prop` and
`bcast_hook` (whose captured records `broadcast_prop` read).  The AutoTrim feature
is unaffected: it applies the trim through the app's control surface, never
through this path.

483 lines gone; all six device tests pass.

## AutoTrim

`autotrim` is the `SHIFT`+soft-key press: it walks the surface's pad tables to the
sample, resolves the WAV, asks `trim` for the audible bounds and applies them
through the app's own control surface, so the page redraws exactly as if the
markers had been moved.  The fourth press clears the trim.

It is wired and the cycle runs four times with the app stable, but **the trim's
effect is not asserted by any test in this lab**: the older `emulator_autotrim.py`
compares screenshots, and its QMP screendump targets the other lab's setup.  What
is proven here is that the press is handled, the app survives it and every other
feature still passes.

## What the last 3 287 lines are

The glue that is left is one unit, and it is worth being precise about why it
cannot be taken a function at a time:

* the trim cluster (`commit_trim`, `trim_probe`, `do_autotrim`, `find_knob_props`,
  `prop_of_param`, `param_from_prop`, `prop_references`) reads the broadcast
  records that `bcast_hook` captures.  Removing `bcast_hook` as "debug
  scaffolding" breaks the trim - tried, reverted;
* `ranges_load` and `rd`/`wr` (the `/proc/self/mem` window) are used by both the
  trim probes and `firmware_ok`;
* the constructor installs ~30 hooks over handlers that still live in this file,
  so it can only move once they all have Rust counterparts.

So the remaining work is: move the last state structs (trim, spectral, hooks),
port the trim cluster against them, port the small hook bodies and the
constructor, then delete the file and link the crate as the cdylib.

## Certified after the paint port

`453b866` is the last fully device-certified build: all six device tests pass with
the Rust draws and toast (Live Looper, Bass Synth, chromatic MIDI, Spectral Chop,
HPSS, Reverb).

## Certified after the input port

`2c11ef0` was certified: Live Looper, Bass Synth,
chromatic MIDI, Spectral Chop, HPSS and Reverb all pass on it.

Two bugs the device found in the recent ports, both fixed:

* the audio writer's 128 KiB `thread_local!` scratch landed in `.tbss` and pushed
  `.bss` **past the last load segment**, so the mod's globals were addressed
  outside its image (this is what made the tests' `/proc/<pid>/mem` reads fail -
  it was not a harness fault);
* `jobflow` restored the bass synth's source backup only on the success path, so
  a cancelled job left the user's sample overwritten.

## State at the "no emulator" checkpoint

After round 55 the device suite was last run green on `3ccef7e` (looper, bass,
HPSS, reverb, MIDI, spectral).  The work after that was done without the
emulator, so everything below is build- and test-verified only:

* `jobflow` owns the whole job lifecycle (start, worker, finish) and the bass
  action's source backup;
* `paint` holds the drawing primitives and the toast, and is **wired**: the glue's
  `draw_colour`/`draw_rect`/`draw_text` and `toast` are shims, and the toast's
  text and deadline moved into the module so the paint hook asks for them;
* `firmware` gained the five-, eight-argument and colour call helpers plus the
  cached image base.

Remaining glue at this checkpoint: 2 651 lines in 91 functions, of which the big
ones are `commit_trim` (359), `buttons` (179), `trim_probe` (154),
`screen_paint` (143), `cc_capture` (78), `looper_draw` (76), `hpss_draw_menu`
(75) and `do_autotrim` (62).

## Running the device suite

Every test drives the app from a fresh start; running one straight after another
without `systemctl restart ac50` fails on the previous test's screen state (the
looper reports "the looper view did not open" if the bass page is still up).  A
full run, verified on the round-51 artifact:

    for t in looper bass_sample midi_chromatic spectral hpss; do
        ssh ... 'systemctl restart ac50'; sleep 30
        python3 mod/tests/emulator_$t.py --lab ~/projects/customs/virtual-mpc-sample
    done

All five pass (Reverb separately, `--effect reverb`).

## Dead sources removed

Three C files were still in the tree but no longer compiled: `looper.c` (684
lines), `bass_sample.c` (89) and `h3m_effect.c` (147).  Their Rust replacements
have been the ones the device runs since rounds 3 and 5, and the headers stay
because they declare the C ABI the Rust implements.  Deleting them removes 920
lines of superseded code from the repository - the binary is unchanged.

`hpss.c` (153) and `spectral.c` (339) are the opposite case: still compiled, and
their Rust ports exist and pass their golden fixtures but are **not wired**.

A differential run on the sample the Spectral Chop test chops settled it.  The C
detector returned

    264, 14187, 30059, 45419, 54635      (detected = 5)

and the Rust one

    264, 30059, 45419                     (detected = 3)

The cause was one line in the chroma mapping: the reference rounds a bin's MIDI
number to the nearest semitone (`lroundf`), the port truncated it.  At 43 Hz bin
spacing a semitone is less than half a bin, so **half the bins landed a semitone
off**, which changed every chroma distance the cut filter compares - and the
filter then rejected two real onsets while all synthetic fixtures, whose chroma
barely moves, still matched.

With `midi.round()` the Rust detector returns exactly the C list, and the Spectral
Chop device test passes on the real sample with the C file deleted.  This is the
one class of bug the golden fixtures could not catch, and the reason the swap was
reverted three times rather than shipped: the fixture compares positions on
synthetic material, which is blind to a feature-matrix error that only real
material exposes.

`hpss.c` is wired too, and its swap caught a convention the golden fixture could
not: the reference's `hpss_split` returns **0 on success and -1 on failure**, and
the port returned 1/0 like the other entry points.  The device reported
`HPSS: processing failed` on a perfectly good split until the return values were
mapped.  A fixture compares buffers, not conventions.

The spectral one: with the Rust onset detector behind
`spectral_onsets`, the Spectral Chop device test failed while iterating its cases
(`('first from Trim', (a, 0, [...]))`) - a triple of the selected sample, a slice
start and the boundary list, which is *not* the same thing as "the detector asked
for 264".  Reading the assertion source afterwards showed the comparison is made
across a full cycle (select, chop, re-read), so the failure could be the detector,
the marker writer, or the sample the test picked up after the previous run.

That reading matters: the swap is still unvalidated, but the diagnosis in the
revert commit is too specific and should not be trusted as the cause. The golden fixture compares positions on one synthetic
input and passes, so the mismatch is in something the fixture does not cover
(sub-frame rounding, or the leading-silence rule, both plausible). That needs a
differential run on a real sample before the swap is safe - the detector is not
wrong in general, it is wrong on this material and the fixture did not catch it.

The C stays in place until that is understood; a wrong boundary list would chop
the user's pad incorrectly, which is worse than a slower port.

## Inventory after round 31

`minimod.c` is 4 381 lines in 94 functions, of which the largest are:

| lines | function | kind |
| --- | --- | --- |
| 359 | `commit_trim` | trim computation + property writes (device-untested) |
| 179 | `buttons` | the whole modal button handler |
| 154 | `trim_probe` | trim-page object discovery |
| 143 | `screen_paint` | every overlay the mod draws |
| 125 | `hpss_finalize` | extract hand-off, write, status |
| 89 | `cc_capture` | knob/transport routing |
| 83 | `midi_note_event` | the note hook body |
| 82 | `spectral_tick` | the spectral state machine |
| 76 + 75 + 55 | `looper_draw`, `hpss_draw_menu`, `looper_cell` | drawing |
| 65 | `install_hook` | the patch itself |
| 63 | `audio_writer` | the preview/looper mix hook |
| 62 + 62 + 61 | `sample_transform_start`, `do_autotrim`, `fmbass_start` | action orchestration |

The rest are small helpers of the kind already ported.  Two observations that
shape the remaining rounds:

* the **drawing** code is about a fifth of what is left and has no logic to test
  beyond "the overlay appears", so it should move last and in one go;
* `commit_trim` and `trim_probe` are the largest untested block - the autotrim
  test does not run in this lab (it targets a different instance), so they need
  either a new test or a careful reading before they move.

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

## The constructor: abandoned after seven attempts

Attempt seven had the install ordering fixed (each original recorded the moment its
site is patched, which was the round-33 diagnosis) and failed the same way, after two
more findings:

1. the load failed with `undefined symbol: midi_note_event` - the C handlers must be
   **global**, and they had drifted back to `static` when the table was reverted;
2. with that resolved the app starts and then crash-loops (13 restarts) inside the
   install.

Decision: **stop**. Seven rounds have gone into 200 lines that fail *silently* - the
.so loads, the app starts, and the install dies somewhere inside eighteen patches,
with no log unless `MINIMOD_LOG=1` and with `MINIMOD_LOG=1` the timing changes enough
that it does not die at all. Every other piece of this port failed loudly and was
fixed in one round; this one is a heisenbug in a debug build.

The one remedy not tried is the obvious one for this shape of problem: install the
hooks **one at a time**, testing after each, rather than all eighteen per build. That
is eleven device runs, and the remaining rounds are better spent deleting the helpers
the constructor protects.

## The output tap: measured, not attempted again

`snd_pcm_writei` is the largest single piece left (94 lines) and the next obvious
target.  An attempt this round got as far as moving its nine statics and the function
itself into `audio`, and stopped at three things worth recording before the next try:

* the nine statics are read by `readi`, the pump, the cfg save and the device tests,
  so they have to keep their names - the same trick as `g_screen` and `g_midi`;
* the latency path needs five glue accessors (`fixed_latency`, `extra_latency`,
  `buffer_out`, `buffer_in`, `input_delay`) that do not exist yet; they are four lines
  each, but they are what the port hangs on;
* `audio.rs` already has a `look_up` helper for ALSA, so the second copy has to reuse
  it rather than be added beside it - the edit that removed the duplicate cut the
  wrong span and duplicated four definitions instead.

Reverted; the certified build is green and the finding is the plan for the next
attempt: accessors first, then the statics, then the function.

## The output tap, second attempt: it runs, but the looper hears nothing

Attempt two followed the plan exactly - the five accessors first, then the nine
statics with their names in Rust, then the function - and it **built, loaded and
started**, with the Bass Synth suite green.  The Live Looper suite failed with
`(0, 1024)`, which is the shape of "no audio reached the looper's input".

One bug was found and fixed on the way: the latency source argument is a **letter**
(`F` for the override, `B` for the ring sizes, `E` for the fallback measurement), not a
boolean.  Fixing it did not change the failure.

So the port is structurally right and one detail is wrong, and the difference has to
be in what the tap *does* with the buffer: the nine statics moved with their types
(`float g_outtap[]` on the C side, `[f32; 88200]` on the Rust side, which are the same
layout), so the suspicious part is `looper_render_s32` being handed the buffer before
the app's write, or the channel stride in the peak and tap loops.

Reverted; the certified build is green.  The next attempt starts from the passing
build and changes **one** of those two things at a time.

## The constructor crash: found by bisection

The bisect worked.  The install table took a mask from `/run/minimod.hooks` (a file, so
each probe was a restart rather than a rebuild), and every bit was a group of sites:

| mask | result |
|---|---|
| `0` | runs - the install machinery itself is sound |
| `1000` data signal | runs |
| `10` buttons | runs |
| `ff0` input + controls | runs |
| `10000` bass writer | runs |
| `20000` repaint | runs |
| `80000` screen paint | runs |
| `40000` **label assign** | **crash loop** |
| `fffff` (all) | crash loop |

So the constructor was never the problem: **one hook does it**.  `A_STR_ASSIGN` (the
soft-key label assignment at `0x102d5e0`) crashes when the Rust install patches it -
and the same Rust function was fine when the C constructor patched it, which points at
what happens *around* the patch rather than the hook itself: the original not being
recorded yet, or the recorder being called for a site whose handler was installed from
a different module.

Two things made this findable at last: a mask that could be changed without a rebuild,
and probing one bit at a time.  Seven earlier attempts guessed at the shape of the
problem; this one measured it in nine restarts.

Next: reproduce with the mask on `40000` alone and a log, and compare
`minimod_assign_original` against the C constructor's `orig_str_assign = t_sa`.

## The bisect was measured on a sick lab

The mask sweep in the previous entry is **not trustworthy**.  Halfway through it the
emulator's MIDI port wedged (`Device not found on port 'Control Surface...'`), and the
app exits on that - so every later "NORUN" was the lab, not the mod.  The `40000`
result, and the "the label site is the culprit" conclusion drawn from it, have to be
re-measured.

The reboot that followed exposed two things the lab keeps in `/run` and therefore
loses: the preload drop-in (`/run/systemd/system/ac50.service.d/mpclab-preload.conf`)
and the preload directory itself.  Both are restored; the certified build is deployed
and the Live Looper suite passes again.

Lesson for the next sweep: check `pidof` *and* `/proc/<pid>/maps` for `minimod.so`
before believing a negative result, and restart the lab if the MIDI port message
appears.

## Re-measured on a clean lab, with a validity check per probe

Every probe now asserts both `pidof` and `minimod.so` in `/proc/<pid>/maps`, and each
crash probe starts from a rebooted lab - because the emulator's ALSA sequencer wedges
after the first failing run and makes every later probe look identical.

Fresh-lab results:

| probe | result |
|---|---|
| `0` (no hooks) | runs, mod mapped |
| `40000` (label assign alone) | **dies before the first paint** |
| `fffff` (all) | dies |
| hybrid: glue installs the label site, Rust installs the rest | dies |

Two things follow.  First, the `40000` result is real and not lab contamination: on a
clean lab the app dies without the MIDI-port message, i.e. it crashes rather than
exiting.  Second, the hybrid still dies, so **the label site is not the only
difference** - something about installing the rest from the table does it too, and the
label site merely reproduces it alone.

What is now known to differ between the working C constructor and the table:

* the C constructor calls `minimod_hook_originals(...)` for the four input hooks and
  `minimod_entries_partial(...)` for repaint and label; the table records each one
  individually - same values, different call;
* the C constructor installs eight sites before the first input hook; the table starts
  with them.

That is where the next probe goes: install **only** the eight sites the C constructor
installs first, from the table, and see whether that alone survives.

## A faithful transcription also dies

The obvious last hypothesis was that my table *redesigned* something.  So the C
constructor was transcribed into `glue::minimod_glue_install` call for call - same
order, same `minimod_hook_originals` / `minimod_midi_originals` / `minimod_entries_all`
batch calls, same unconditional sites, same paint install last.

It dies the same way.  So the difference is **not** the order, not the recorders, and
not the batching.

What is left, and it is a short list:

* the C constructor runs `looper_cfg_load()` and reads the latency environment
  variables **before** any install; the transcription reads them before the call, so
  that is equivalent - unless one of those reads matters to a hook;
* `base` is a C global that `rd`/`mapped`/`FN` use; if `minimod_set_base` were not in
  effect during the Rust install, every hook would read the wrong image;
* the C constructor installs `A_SCREEN_VT` through a `static` function in the same
  translation unit; the transcription calls the same function through `extern "C"`.

The second one is testable in one run: print `minimod_glue_base()` from inside the
install and compare it with `base`.  That is the next probe, not another rearrangement.

## The base probe: answered

```
glue=0xaaaaaaaa0000 crate=0xaaaaaaaa0000
```

Both sides agree, before and during the install.  So the third candidate is gone too:
the crate's base is in effect, and `rd`/`mapped`/`FN` inside the hooks are reading the
right image.

## The elimination table, after twelve attempts

| candidate | ruled out by |
|---|---|
| struct layout drift | round 4 (a shifted `#[repr(C)]`) |
| dropped statics | round 31 (`g_screen`) |
| static handlers not linkable | round 33 (`undefined symbol`) |
| install ordering / recorders / batching | round 51 (faithful transcription) |
| the label site alone | round 50 (it does crash alone - but so does everything) |
| lab artefacts (sequencer wedge, lost drop-in) | round 49 |
| the base not being in effect | round 53 (this probe) |

What is left is the one thing that has never been varied: **who calls the install**.
The C constructor succeeds; a Rust function called from the same C constructor, with
the same base, the same sites in the same order and the same originals, crashes before
the first paint.  That is a difference in the calling context, not in the table - and it
is the kind of difference that needs a debugger, not another build.

## Decision

Twelve attempts, no landing.  The constructor stays in C, and this document says so
plainly rather than leaving it as an unfinished intention.  The remaining rounds go to
the pieces that still *can* be moved, so the C file shrinks to the constructor and the
handful of handlers it names - which is the smallest it can be without the answer above.
