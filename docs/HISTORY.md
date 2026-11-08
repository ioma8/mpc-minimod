# History — investigations, reversals, open work

The chronological record: what was tried, what was believed and then disproved,
and what replaced it. Findings live in [`MECHANISMS.md`](MECHANISMS.md),
features in [`FEATURES.md`](FEATURES.md), the method in
[`RE-METHOD.md`](RE-METHOD.md). Detailed captures remain in `docs/re/` and in
[`HYPOTHESES.csv`](../HYPOTHESES.csv).

This file exists so that no one repeats a dead end that is already paid for.

---

## 1. AutoTrim and the Trim page

### The problem

Trim a sample to its silent edges without touching files, using the app's own
state so the page and the renderer agree.

### What was believed first, and why it was wrong

1. **"Write the model, the page follows."** Writing the captured `TrimParameter`
   value and its applied copy changed nothing on screen. Byte-level screen
   snapshots then showed the page paints from its **own cached percentage**
   (`screen+0x1f6c`, mirrored at `+0x28bc`), not from the parameter. A screen diff
   across a 50 → 75 % knob move changes exactly four words.
2. **"The page never repaints on a knob move."** Wrong: a knob move repaints the
   waveform. The original 0-px measurement had not registered the knob change — a
   dirty measurement, not a firmware fact.
3. **"The marker is drawn by a locator we can move."** `AC50WaveformLocator`
   caches a normalised `pos` at `+0x168` and never reads the trim property at
   paint time; who writes `pos` on this page was never found. The stock locator
   route was abandoned.
4. **"Broadcast through the property and the app will repaint."** Broadcasting
   through `0x13bb930` stores the value and notifies the listeners, and a learned
   Start property *did* move the painted edge to 50 %. But a subsequent repaint
   call changed 0 px, and a nudge of the End knob was needed to see it. Calling
   the app's setter directly crashed (needs the exact surface and mode).
5. **"The encoder is a float property."** The wheel arrives as CC `0x64` on the
   rawmidi stream and reaches a **signed integer event emitter**
   (`0x140dbc0(surface+0x2da8, delta)`), which has no de-duplication. Inferring a
   detent from a changing float property selected a silent source and dropped
   turns; 200 repeated events are 200 events.
6. **"The CC value is the encoder."** The wheel is CC `0x64`, but the page
   does not consume the CC value directly: the firmware decodes it into a
   *signed delta* and delivers it to a separate integer signal object
   (`0x140dbc0`, object `surface+0x2da8`), which has no de-duplication. Reading
   the CC value (or a float property that happened to change alongside) loses
   turns; 200 repeated deltas are 200 events.

### What settled it

The native control path: the app's own handler (`0x1968020`) takes the live
dispatcher context, the pad index and **frame positions**. The mod opens Start to
zero, sets End to `last + 1`, then Start to `first` — that order lets the bounds
expand when clearing a trim. The command updates the model and renderer, so no
property, layer field or overlay is written. The earlier approach (property
writes plus a hand-drawn marker overlay) was removed.

Two corrections came from listening and measuring rather than from code:

* IEEE **float32** WAVs were being decoded as integer PCM32 (quiet tails looked
  loud), and a negative full-scale sample overflowed a 16-bit `abs`. The parser
  now distinguishes the format tag and handles full-scale negatives.
* The default threshold moved from −40 dBFS to **3 % of peak** (≈ −30.46 dB), and
  SHIFT-held cycling (3 % / 6 % / 9 % / clear) replaced a single shot.

### Model offsets, finally

The native command writes **frame** bounds at `layer+0x7b8` (start),
`layer+0x810` (end) and a start mirror at `+0x868`. This was found by diffing the
layer around a full cycle and is now asserted by
`mod/tests/emulator_autotrim.py`. The older `+0x4d8/+0x588/+0x530` words belong
to the removed property-based path; the native command does not touch them.

### Disproved, do not retry

* writing the parameter value, its applied copy, the property cache, the layer
  frames or the slider value;
* moving the stock locators (`+0x168` is written by code that was never found);
* using the CC dispatcher (`0x159f230`) as the panel path;
* setting the trim through the app's setter `0x01a68020` without the exact
  surface/mode (crashes);
* guessing the sample by on-screen name (the name is not the pad's sample);
* reusing `g_setter[]`-style pointers captured at startup (freed objects).

Still open: hardware, save/reload, non-first layers, selection routes other than
a pad press, and selection during sequence playback (the selected pad is the last
note-on pad, default A01).

---

## 2. HPSS

### Three routes, one worked

1. **Sample-pool load request + assignment command.** Decompiling the command
   vtables identified the load execute (`FUN_01a97bd0` → `FUN_01ee0620`), the
   assignment execute (`FUN_01a9a3b0`), the per-layer change
   (`FUN_01a9a290` → `FUN_029c6790`) and the import request constructor. The
   request carries five service pointers; a fabricated request with null
   pointers is unsafe, and the services were never identified. Dead end.
2. **Chop Extract reuse.** The stock Sample/Chop Extract handler
   (`0x17580d0`) already creates, names, registers and assigns a sample. The
   mod only has to (a) make the range cover the whole source and (b) substitute
   the audio the native writer receives. **This is the shipped route.**
3. **Direct pool writes.** Writing a pointer into a layer skips ownership and
   project state. Rejected on principle and never attempted.

### The pieces that made Extract work

* The extraction settings object is at `screen+0x9ba0` with the range at
  `+0x78`/`+0xd0`. Patching it around the handler call does **not** work: the
  handler re-syncs from the UI before constructing the command. The range must
  be patched inside the execute hook, immediately before calling the original,
  and restored immediately after. Proven with a live chop/trim range of
  `0..5083` while HPSS wrote all 63,906 frames.
* The writer callback (`0x1e8aff0`) carries frames at `+0` and a float32 pointer
  at `+8`; substituting the pointer gives scoped audio substitution, first
  proven bit-exact with `source × 0.5` over 63,906 frames.
* Two empty pads are preflighted with the native occupancy predicate; names are
  `HPSS Harmonic`/`HPSS Percussive` and the app appends a numeral.
* A destructive rollback was implemented and **removed**: clearing a pad outside
  the native command/history path made the app delete the file and report a
  missing sample on reload. The shipped path never clears pads.

### Input: four attempts to read one wheel

| attempt | result |
|---|---|
| rawmidi `read()` interposer | works mechanically (32-byte records, one MIDI byte at `+16`) but dropping the three CC bytes was fragile: active-sensing and "unused" CCs still moved the page |
| rewrite the CC value | insufficient; the app acted on the rewritten value |
| float-property inference | wrong source, dropped turns (see §1) |
| **native integer signal** (`0x140dbc0`, object `surface+0x2da8`) | one navigation source, no de-duplication, exact object match — shipped |

The rawmidi knowledge was not wasted: it explains the transport, and the
`/dev/snd/midiC1D0` framing is recorded in
[`TOOLING.md`](TOOLING.md#panel-protocol) for probe work.

### Reverb tail

Requesting a longer extraction range was clipped by the native reader
(`FUN_021c6e40` stops at the source length). The fix exploits the **writer
boundary**: the writer callback receives a mutable block frame count, so the mod
substitutes the final source block with the remaining rendered buffer, raises
that block's frame count for the call, and restores it afterwards. The concrete
float WAV writer (`0x1e93570`, reader loop `0x21c6e40`) then finalises the longer
WAV and native Extract still registers and assigns it.

### The source-restore mistake (2026-11-08)

`hpss_finalize` restored a sample backup for every effect, but only the Bass path
ever took one — so Reverb/Vinyl/Lo-Fi always ended with
`<name>: source restore failed` *after* writing a valid copy. Taking the backup
everywhere made it worse: the restore wrote at remembered offsets, and Extract
makes the app re-serialise its project working copy (the `JUNK` chunk shrinks a
few bytes) and re-dither the PCM. The stale offset corrupted the source (peak
0.891 → 1.000, ~21 k bytes shifted).

The whole backup/restore pair was removed. The mod writes no sample file; the
source check in `emulator_bass_sample.py` now allows the app's dither (max
2 LSB) and still rejects real corruption.

---

## 3. Spectral Chop

### Six passes

1. **The wrong detector.** The first implementation selected the stock Chop
   view's **Threshold** type and reported its count as its own. It looked right
   because markers appeared — they were the firmware's positions. Reduced to a
   fail-safe (`Spectral: not applied`) until there was a real write path.
2. **Chasing the command API.** Static work found Insert/Split/Remove/Remove All
   and the UI wrapper; all need an "editor" object carrying the sample at
   `+0x4c8`. Three heap scans failed to find it, and calling Insert on the best
   candidate faulted — read, wrongly, as "the command wants a different sample".
3. **The address-space mistake.** The probe used Ghidra VAs as mod offsets, so
   every hook sat `0x100000` past its target and "the hook never fires" was an
   artifact. After subtracting `0x100000`: hooks fired immediately,
   `isSampleValid` on `layer+0x4a8` returned 1, and Split/Merge were seen to fire
   **no command** at all — only a marker resolver, slice-list accessor and slice
   publisher. That redirected the work from the command API to the model.
4. **The human in the loop.** Two user remarks ("K1/K2 move the selected
   boundary; you select a slice by pressing a pad" and "Split works only in
   Manual") became one experiment: snapshot every word of every slice on every
   event and diff. Result: `slice0 +0xd0 2465 → 4906 (+ mirrors on slice1)`,
   `slice1 +0xd0 4907 → 5143 → 5295 → 6051`; `+0x78`/`+0x128` are start and
   mirror; a pad press chooses which slice the knobs edit.
5. **The first working chop** (direct writes + vector shortening + Chop
   re-entry, commit `9b299b9`). It worked because a rebuild was forced: the Chop
   view caches its layout and raw writes broadcast nothing.
6. **The native API** replaced the writes: `reset`, `insert-at-frame`,
   `reconcile`, `select`, Manual through the binding's model callback. The
   action moved to the UI thread as a phase machine and got an emulator
   acceptance test that reads the model independently.

### Disproved along the way

* the "rebuild" lives in the function containing `0x1a601d0` — it is an unwind
  cleanup;
* the editor is findable by scanning for `*(p+0x4c8) == sample` — never observed;
* a pad press inserts a slice through `0x21a3b10` — it only moves the selection;
* slices are allocated with libc `malloc` — the app has its own allocator;
* `layer+0xb0…+0xf8` hold `SlicePoints` — inline `std::string` data;
* `+0xd0` is a start frame — it is the end;
* K1/K2 are zoom controls — false, the test had selected start/end markers;
* direct writes plus a rebuild are the way — they work, but the native API is
  correct and needs no rebuild.

---

## 4. External MIDI → 16 Levels

Five steps, in the order they happened:

1. **A reference proved it possible.** SampQOL v0.1.5 in the same emulator:
   MIDI 60 vs selected pad correlation **0.997**, 1.000 on another pad;
   stock/minimod max 0.05. Its log listed the hook map.
2. **Byte-level event capture at the dispatcher.** Diffing a physical pad event
   against a synthesised note exposed `event+0x14` (property mask, u16),
   `+0x16` (property 0), `+0x30` (pad/slot), one field at a time.
3. **Capturing the app's own call.** Instrumenting the existing `pitch_off`
   wrapper logged the app calling the property setter with
   `w3 = level − originalPad` and `event+0x30` = selected pad. Reproducing that
   call with `w3 = note − 60` is what made full-range chromatic work. The hint
   came from the user: the octave feature already pitched outside the 16-pad row.
4. **A controlled sine** turned pitch into arithmetic (48 → 110.4, 60 → 220.7,
   65 → 293.4 Hz on 220 Hz). An earlier "48 = 247.6 Hz" was PAD 6's tail, because
   the app's row is offset by `originalPad` while the MIDI path uses absolute
   `note − 60`.
5. **The contract, then the full-range check.** Routing is keyed by source;
   rebuild the note for the selected pad, call the setter, dispatch with the
   physical-pad sentinel `0x7ffffffa`. Every note 36–96 played the selected
   sample; 16 Levels off restores the stock dispatcher bytes (`d503233f`).

An earlier raw-ALSA experiment opened a device already owned by the firmware and
called the pad `note_on` entry directly, creating two competing input paths. It
was removed in favour of transforming in the native event path.

Still open: the root is fixed at C3; Pad MIDI In must be re-enabled each boot.

---

## 5. Bass Synth and the native FMBass probe

The native instrument was attempted seriously before falling back.

* The first discovery: the effect context does **not** satisfy the H3 instrument
  host interface. Constructing FMBass with it crashes; `H3pp` is queried through
  host vtable slot 4.
* The documented `create → configure → setSampleRate → prepare` sequence then
  crashed elsewhere. Ghidra showed the trap: `FUN_03dabed0` is
  `H3M::StereoReverb::Process`, which calls its own host `Audio::H3M::Part::Process`
  through vtable slot `+0x20`; a null host faults.
* The graph-wrapper fix (2-arg constructor, type string `"FMBass"` via
  `FUN_03cada70`, prepare `FUN_03d1c200`) made **create, configure and prepare
  all succeed**. Render still crashed because the graph had no host instance.
* A **standalone part-event queue** was then targeted: queue callback closure in
  `0x4551c4`, invoker `0x3f5b88`, active callback `0x4e3f2c`; `FUN_03e0c1a0` drains
  when a counter at `+8` is non-zero and dispatches through the callback after
  reading the callback object's `+0xa8/+0xb0/+0xa9/+0xaa`. A corrected headless
  host (vtable = accept_note) made the pump accept one note-on and advance the
  queue, so the voice manager's active index reached 1.
* **Render stayed silent**: the generic H3 graph has no FMBass render node
  (`graph+0x88` is null after prepare), and the voice's DSP countdown at `+0x3dc`
  never advances. Direct part-renderer calls are not proven safe.

Production therefore uses the vendored **DaisySP `StringVoice` + SVF**, which
sounds like a pluck rather than the firmware synth. The native probe remains in
`mod/src/fmbass_sample.c`, compiled out unless `-DMINIMOD_FMBASS_PROBE=1`.

---

## 6. Sequence recording (not implemented)

External MIDI plays but is **not recorded** into a sequence. What was tested:

1. Physical pads record: SEQ RECORD + PLAY, three pads, STOP, playback gives RMS
   5318 / peak 32767.
2. Chromatic MIDI is not recorded: same pass with notes 60/64/67 leaves all five
   sequences with `seqEventList.events == []`.
3. **Stock external MIDI is not recorded either** (16 Levels off, stock pad
   mapping). So this is a firmware feature gap, not a 16 Levels regression.
4. The saved `.xpj` is not a reliable record of live sequence content; playback
   audio is.

Path analysis: the dispatcher's caller for a physical pad is the pad queue pump
`FUN_01f27360` (file `0x1e27554`); external notes enter at `FUN_01f25cf0`
(`0x1e25dd0`). Neither contains a recorder call. The pad pump is invoked from
`FUN_020ce3e0` (call site `0x20ce480`), the **observer notification loop** — the
pad-grid observer layer, which is where the sequencer records. The chromatic
transform sits at the dispatcher, *below* that layer, so the sequence never sees
it.

Tried and ruled out: replaying the transformed note with the original external
source instead of the pad sentinel — notes 60 and 72 are silent, routing depends
on `0x7ffffffa`. Probed recorder candidates: `FUN_0221a800` (SampQOL
"recwriter") never called during a recorded pad press; `FUN_02219d00` is the note
dispatch, not the recorder.

Next step, if taken: hook `FUN_020ce3e0`, log the method index and listener
targets for one recorded vs one unrecorded pad press, identify the listener that
only runs while recording, and feed it the chromatic note (or replay the pad-grid
event and apply pitch through the setter).

---

## 7. Code review of the earlier implementation

An external-style review of the pre-native AutoTrim module (~3,900 lines then)
found one genuine memory-safety bug and a set of release-hardening problems. All
of the critical and hardening items were fixed:

| item | problem | resolution |
|---|---|---|
| C1 | `mlog` wrote `vsnprintf`'s *would-be* length, over-reading the stack buffer | clamp to the buffer size |
| C2 | the debug command channel (arbitrary memory write) shipped enabled | compiled out with `#if MINIMOD_DEBUG` |
| C3 | page watches removed write permission from live pages; a returning fault handler resumed after the fault | debug-only, with guarded recovery |
| C4 | process-wide handlers for `SIGSEGV`/`SIGBUS`/`SIGILL`/`SIGABRT` that returned | removed |
| C5 | fatal-signal handler also returned from the mod's exit path | removed |
| M1–M6 | magic `MINIMOD_TESTTRIM` hook, dead experimental hooks, polling threads | removed or reworked; selector-driven policies |
| M4 | `/proc/self/mem` cached as `O_RDWR`, where a read returns `EIO` and froze the UI | separate `O_RDONLY`/`O_WRONLY` descriptors |

Remaining low-severity notes were cosmetic: a stale description of a debug hook
constant, magic numbers, and `snprintf` truncation. The review is also where the
"pads do not repaint by design" clarification came from (stock behaves the same).

---

## 8. Per-pad export design study

Before the converter existed, the modding notes asked whether per-pad stems could
be produced on-device: either `mute + re-render` (days of work, ~6 min per song)
or a `single-pass tap` (weeks, <30 s). Neither was built. The problem moved to
the offline converter (`converter/`), which renders stems from the project file
and uses one-pad projects exported by the device as ground truth
([`../converter/docs/DEVICE-PROBE.md`](../converter/docs/DEVICE-PROBE.md)).

---

## 9. Superseded documents

The consolidated docs replace a larger set of handoffs and investigation logs
(AutoTrim handoff, HPSS handoff and RE log, MIDI method notes, spectral-chop
history, code review, trim-page log, ecosystem survey, and the device notes).
Their unique measurements are preserved here and in
[`MECHANISMS.md`](MECHANISMS.md)/[`FEATURES.md`](FEATURES.md); the originals
remain in git history, and the raw captures (Ghidra output, decompiled
fragments) remain in [`re/`](re/README.md).
