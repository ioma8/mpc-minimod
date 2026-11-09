# Features — behavior, implementation, evidence

What the mod does today, how each feature is wired into the firmware, what was
measured, and what is still open. Mechanisms are catalogued in
[`MECHANISMS.md`](MECHANISMS.md); the road to each one is in
[`HISTORY.md`](HISTORY.md).

| feature | trigger | state |
|---|---|---|
| 16-Levels pitch shift | `PLUS` / `MINUS` | verified on hardware |
| AutoTrim | `SHIFT` + `B3` | verified in the emulator (model read-back) |
| External MIDI chromatic | MIDI keyboard, Pad MIDI In on, 16 Levels | verified on hardware |
| SAMPLE OPTIONS menu | `SHIFT` + `DATA` | verified in the emulator |
| HPSS | menu → HPSS | emulator-verified |
| Spectral Chop | menu → Spectral Chop | emulator-verified (exact boundaries) |
| Add Reverb / Vinyl / Lo-Fi | menu → effect | emulator-verified |
| Bass Synth | menu → Bass Synth | emulator-verified |

Two invariants hold across the mod: **it never writes a sample file** and it
refuses rather than guesses. Every action either reports what it did
(`<Action>: copy created`, `Spectral: n slices`) or names the reason it stopped.

---

## 16-Levels pitch shift

**Behavior.** With 16 LEVELS lit, `PLUS`/`MINUS` move the whole 16-pad pitch row
by ±8 semitones per press (half the grid), up to ±32 semitones; pressing both
resets to the original pitch. The keys are swallowed only while 16 Levels is
active, so they keep their stock meaning elsewhere.

**Implementation.** The offset is added at the app's 16-Levels pitch setter
(`A_PITCH_OFF`, mod `0x27e4400`): `pitch_off(a0, a1, a2, a3)` calls the original
with `a3 + 8*g_octave`. No renderer context object is modified. 16 Levels mode
is tracked from the panel toggle and cleared when the Sample page goes away.

**Evidence.** Verified on hardware. The supported branch is the common 16-Levels
path; a separate firmware branch in the renderer is not hooked.

---

## AutoTrim

**Behavior.** `SHIFT`+`B3` trims the selected pad's sample to its silent edges.
While `SHIFT` stays held the presses cycle **3 % → 6 % → 9 % of the file peak →
clear**; releasing `SHIFT` resets the cycle. The threshold has a 90-count
absolute floor; `MINIMOD_TRIM_DB` overrides the relative part for the first
step (default 3 % ≈ −30.46 dB). Each step shows a one-second toast
(`Trim 3% of peak`, …). AutoTrim is disabled in Chop mode so `SHIFT`+`B3` keeps
its stock Split meaning, and the soft-key caption is hidden there too.

**Implementation.** On the B3 press edge (the button repeats while held) the mod
resolves the pad's sample, reads its audio — the app's in-memory float32 buffer
first, so it also works before a sample is saved, with the WAV as the fallback
for streamed samples — computes the first/last frames above the threshold, and
calls the app's native control handler (`0x1968020`) with the live dispatcher
context and the pad index. Order matters: Start is opened to
zero, End is set to `last + 1`, then Start is set to `first` — that lets the
bounds expand when clearing an existing trim. The command updates the model and
renderer; the mod writes no property, no layer field and no marker.

Sample resolution: pad index (last note-on, default A01) → surface pad vector
`+0x2808/+0x2810` → pad state `+0x22e8` → layer `+0x4a8` → Sample. The WAV path
comes from the sample holder's text pointer (`*(sample+0x30) + 0x120`) first,
then a pointer scan, then a frame-count match in `TempProjects` for unsaved
recordings. The frame count is validated against `TrimParameter+0x1e8`.

WAV parsing walks RIFF chunks (Akai recordings put `JUNK` before `fmt`, so `fmt`
sits at 72, not 20) and handles PCM 16/24/32-bit and IEEE float32, including
`WAVE_FORMAT_EXTENSIBLE` subformats.

**Evidence.**

| measurement | result |
|---|---|
| 15,129-frame kick, 3 / 6 / 9 % | bounds `23..9794`, `62..7731`, `71..5358` |
| float32 A07 (90,197 frames) | `81..60816` |
| float32 A09 (98,280 frames) | `76..83103` |
| current emulator check, 10,501 frames | `14..8159`, `23..7503`, `24..7453`, clear `0..10500`, start mirror consistent, no restart |

The unit tests of [`mpc-sample-sdk/src/platform/{wav,trim}.rs`](https://github.com/ioma8/mpc-sample-sdk/tree/main/src/platform) check the parser and the
scan (PCM16/24/32 + float32, negative full-scale, malformed headers, the
memory path); `mod/tests/emulator_autotrim.py` reads the trim bounds from the
live model (`layer+0x7b8` start, `+0x810` end, `+0x868` mirror) and asserts the
whole cycle.

**Limits.** Real hardware; project save/reload; non-first sample layers;
selection through routes other than a pad press; selection while a sequence
plays. Because the selected pad is the last note-on pad, sequencer playback can
change it — a dedicated UI-selection source would be needed before release.

---

## External MIDI chromatic playback

**Behavior.** With **16 Levels** on and **Pad MIDI In** enabled in MIDI Config,
an external keyboard plays the selected sample chromatically over the whole
keyboard: the sample's **own root note** is the original pitch (C3/MIDI 60 when
the sample carries none), each semitone is one semitone, no wrapping, no
fallback to other pads. This applies **only to notes from outside the app**;
pads played on the device in 16 Levels keep the firmware's own slot mapping
(slot 0 = the sample at its root), exactly as before. Chords, note-offs and sustain
stay in the firmware's own path. With 16 Levels off, events pass through
unchanged.

**Implementation.** The native note dispatcher (`0x211b090`) is hooked **lazily**
when 16 Levels turns on and restored to stock when it turns off or the Sample
page goes away; the first 16 bytes are verified before each swap. For an
external note the mod rebuilds the event for the selected pad (property mask
cleared, `event+0x30 = selected pad`), reads the sample's root note from
`sample+0x130 -> data+0x3e4` (0–127, fallback 60), calls the app's own pitch
setter with `w3 = note − root`, then dispatches once with the physical-pad source
`0x7ffffffa`. Note-offs take the same path with velocity 0.

**Evidence.** A01 `RnB-Kik-Grove1 91V` has root 88 (read from the live model);
the debug readout gives `midirow 67 -> row −21`, where the old fixed-C3 code
produced +7. On a 220 Hz sample whose root is 60: note 48 → 110.4 Hz, 60 →
220.7 Hz, 65 → 293.4 Hz; all notes 36–96 played the selected sample (RMS 161–613
instead of stock 4256/16269). With 16 Levels off the dispatcher's first word is the
stock `d503233f` (hook installed: `155f947c`). Verified on hardware;
`mod/tests/emulator_midi_chromatic.py` is the automated check.

**Limits.** The root is read from the selected pad's sample on every external
note; a sample whose root cannot be read falls back to 60. The root-aware path is
verified end to end in the emulator on A01 (root 88: C3/C#3/G3 →
−28/−27/−21, `emulator_midi_chromatic.py`); a hardware MIDI keyboard on a non-60
sample is still to be checked. Pad MIDI In is not persisted by the app and must
be re-enabled after every start (this is the most common false "it does not
work"). The SampQOL reference also has a
separate chopped-pad mode; MiniMod applies chromatic pitch without a
slice-versus-pitch setting.

---

## SAMPLE OPTIONS menu

**Behavior.** On the Sample page, `SHIFT`+`DATA` opens a seven-row overlay —
HPSS, Spectral Chop, Add Reverb, Vinyl, Lo-Fi (Decimator), Bass Synth, Close.
The DATA wheel moves the highlight, `DATA` confirms, `SHIFT`+`DATA` closes.
While the overlay owns input, other controls are suppressed; a status line
reports the outcome of the last action, and a one-second toast repeats it.

**Implementation.** One navigation source: the mod matches the exact native
integer signal object (`surface+0x2da8`, emitter `0x140dbc0`) and moves by the
delta's sign, consuming the event while the menu or an action owns input. Every
other signal object is forwarded. Identical consecutive deltas are events, not
deduplicated values. A separate decoded-control hook blocks knob/fader controls
while the menu is open and lets DATA through to the firmware's decoder.
`/run` logging is not involved; the release build has no log.

**Evidence.** The unit tests of `rust/crates/minimod/src/{input,menu,routing}.rs`
(repeated signed events, zero, unrelated signals, busy suppression, closed-menu
forwarding) and `mod/tests/emulator_menu.py` (three openings × 32 turns, both
directions, knobs ignored, DATA closes). The menu row geometry is measured from
the rendered screen (row centres 24 px apart, first at y≈55), not assumed.

---

## HPSS

**Behavior.** Splits the selected sample into `HPSS Harmonic` and
`HPSS Percussive` on the next two empty pads through the stock Chop Extract
command. The source is left untouched and never replaced. If fewer than two pads
are free the action reports `HPSS: need 2 pads` and writes nothing; if the
second write fails it keeps the first and reports `HPSS: 1 of 2` (no destructive
rollback — clearing a pad outside the native command made the app delete the
file and report a missing sample on reload).

**Algorithm** (`rust/crates/dsp/src/hpss.rs`, pinned to the original C by the
golden fixtures in `rust/tests/golden`; the full derivation and the tuning record are
in [`../tools/hpss/ALGORITHM.md`](../tools/hpss/ALGORITHM.md)):
STFT 2048 / hop 512, symmetric Hann, reflect padding, time median 41 frames
(`odd(0.476·sr/hop)`), frequency median 17 bins (`odd(366·N/sr)`), power 6,
margin 0.55, complementary masks, `Σw²` overlap-add normalisation, no gate, no
normalisation. `h + p` reconstructs the input.

**Evidence.** Native output matches the Python reference sample-by-sample:

| case | frames × ch | harmonic max err | percussive max err | h+p vs source |
|---|---|---|---|---|
| long mono | 63,906 × 1 | 7.2e-8 | 8.9e-8 | 4.0e-7 |
| short mono | 5,083 × 1 | 8.9e-8 | 7.8e-8 | 4.7e-8 |
| stereo | 44,100 × 2 | 8.9e-7 | 7.2e-7 | 7.4e-7 |
| digital silence | 4,410 × 1 | 0 | 0 | exact |

Python reconstruction error −138.4 … −141.0 dB. Both outputs are playable and
persist across an `ac50` restart; the user-content source hash is unchanged
(`2028b908…`); the app's project working copy may be re-dithered (±2 LSB).
Ordinary `SHIFT+B1` Extract and `SHIFT+B3` AutoTrim still behave. The
full-sample override was proven with a live chop/trim range of `0..5083` while
HPSS wrote all 63,906 frames.

**Limits.** Real hardware; native undo grouping (two extracts = two commands);
a genuinely full 128-pad project; large-sample memory pressure; navigating away
while the worker runs; the stock writer emits float32.

---

## Spectral Chop

**Behavior.** Cuts the selected pad's sample into **regions, not notes** — a new
chord, a new phrase, a different drum — and rewrites that pad's chop in place at
up to 16 detected frames. It creates no samples, uses no free pads, writes no
file, and kills the previous chop (stock slice edits are not undoable either).
The menu reports `Spectral: n slices on this pad`, or the reason it refused
(`no sample on this pad`, `cannot read sample audio`, `no onsets`, `cannot
select Manual chop`, `native marker write failed`). Input is the sample's
in-memory float32 buffer first — so an unsaved recording works — with the WAV as
the fallback for streamed samples.

**Detector** (`rust/crates/dsp/src/spectral.rs`; reference `tools/hpss/spectral_chop.py`):

| step | rule |
|---|---|
| input | mono sum for analysis only; STFT 1024, hop 512, symmetric Hann |
| candidates | every frame with an attack: per-band log flux (bands split at 250 Hz and 2 kHz), threshold `0.1·mean + 2.0·local median(±0.5 s)`, prominence `0.5·mean` above the valley since the previous candidate, 40 ms apart |
| region length | `0.45 / onsets-per-second`, clamped to 0.18–0.75 s — dense material gets short regions, sustained material long ones, one rule |
| keep | slice long enough **and** the sound moved on: chroma cosine distance ≥ 0.05 (12 bins, chord change) **or** 48 log-band distance ≥ 0.15 (different sound); silence on one side counts as a change |
| position | steepest 1 ms step within ±15 ms, backed off to 15 % of the transient peak |
| limits | no cut in the last 120 ms; 16 slices; time order; trailing zero-padded frames ignored |

**Write recipe.** All model work happens on the UI thread, one phase per paint
pass; the input hook only publishes a request. Enter the stock Chop view if
needed; select **Manual** through the chop binding's own model callback
(`screen+0x168`, `+0x9c == 0`); then `reset` → one `insert-at-frame` per detected
frame → `reconcile` ends and tail → select slice 0; read every boundary back and
only then report success. Because the native API allocates, links listeners and
broadcasts, the Chop view updates itself.

**Evidence.** `mod/tests/emulator_spectral.py` restarts the app, verifies the
deployed library digest, finds the screen through its vtable, reads the chop
model out of `/proc/<pid>/mem`, runs the real menu action and compares exact
frames. Fixture: A13 `GuitPh-Grove3 E85 91V`, 62,259 frames →
`[264, 14187, 30059, 45419, 54635]`, last slice ending at 62,259, chop type
Manual (0). The test also covers three repeats, the Regions and Threshold
starting modes, playback, Chop re-entry and restart; the pid never changes.

**Limits.** The menu is the only entry point (`SHIFT`+`CHOP` is not bound); 16
slices (stock limit); the previous chop is replaced; a user edit during the
operation is detected only through the sample identity check; Manual selection
depends on the binding layout (`+0xd8/+0xc0/+0xe0/+0xe8`) and a firmware change
there produces an explicit refusal; real hardware untested. Threshold mode's
positions and the serialized form of a manual chop remain unknown.

---

## Add Reverb, Vinyl and Lo-Fi

**Behavior.** Each action reads the selected pad's audio (in-memory float32
first, WAV fallback), renders it through the firmware's own H3 effect, and
assigns the copy to the next free pad through stock Extract. **Add Reverb** uses `Air::H3M::StereoReverb` and appends the
generated tail, so the copy is longer than the source; **Vinyl** uses the
firmware's `Vinyl`; **Lo-Fi** the `Decimator` (lower bit depth and sample rate).
The source is not replaced. If there is no free pad, or the selected sample
changes during processing, the action stops without submitting output.

**Implementation.** The input audio is the selected sample's in-memory float32
buffer, with the WAV as the fallback for streamed samples. [`mpc-sample-sdk/src/platform/effect.rs`](https://github.com/ioma8/mpc-sample-sdk/blob/main/src/platform/effect.rs)
creates a standalone effect instance
(factory by mod offset, context `{rate, 0, 512}`, vtable slot 9 setup with the
context pointer, slot 11 per stereo block, slot 1 destroy). Parameters are
written through `*(float**)(effect+0x18)`: Vinyl `Drive 0.18`, `Noise Level
0.12`; Decimator `Bit Depth 0.58`, `Sample Rate 0.70`. The renderer feeds four
seconds of silence and trims the tail at a peak-relative threshold plus a 100 ms
margin; the output peak is limited to 0.98 only if needed. The tail is written
through the Extract writer boundary described in
[`MECHANISMS.md`](MECHANISMS.md#the-writer-boundary): the final source block is
replaced by the remaining rendered buffer and its frame count is raised for that
one call, so the native writer finalises the longer WAV and native Extract still
registers and assigns it.

**Evidence.**

| action | output |
|---|---|
| Reverb on A01 (10,501 frames) | `Reverb*.wav`, 154,896 frames (+144,395 tail), float32 stereo 44.1 kHz, tail RMS 0.03568, output RMS 0.097476, peak 0.98 |
| reverb tail length | follows the effect's own RT60 (measured 145,333 frames ≈ 3.30 s on the stock patches), trimmed at a peak-relative threshold plus 100 ms |
| Vinyl | same length, RMS 0.315 |
| Lo-Fi | same length, RMS 0.424 |

All three report `copy created`, assign the pad, keep the firmware pid and
`NRestarts=0`. The source's audio differs only by the app's own re-dither
(max 2 LSB), verified by `mod/tests/emulator_bass_sample.py`'s source check;
an earlier backup/restore implementation wrote into the re-serialised file and
corrupted it — see [`HISTORY.md`](HISTORY.md#the-source-restore-mistake-2026-11-08).

**Limits.** Real hardware; the effect parameters are fixed in the mod (no
dry/wet or size control); tail length is fixed at 4 s; output is float32;
native undo grouping not exercised.

---

## Bass Synth

**Behavior.** The menu opens a parameter page: pads 1–16 audition C2–D#3
chromatically, `K1` brightness, `K2` drive/excitation, `K3` low-pass cutoff, and
`DATA` renders one centred stereo C2 (MIDI 36) pluck, 1.5 s with a 400 ms
key-down portion, and assigns it to the next free pad through stock Extract.
The selected pad's sample is not rewritten.

**Implementation.** `rust/crates/dsp/src/bass.rs` is a Rust port of the DaisySP `StringVoice`
(a Karplus-Strong physical model) and SVF subset: C2 65.406 Hz, four preview voices,
`K1` sets brightness/damping, `K2` accent/structure/drive, `K3` maps
exponentially from 700 Hz to about 16.8 kHz (or 45 % of the sample rate). The
live preview is mixed into the real output call: the app writes interleaved
S32_LE (4 channels, 44.1 kHz) through `snd_pcm_writei`, and the mod's exported
`snd_pcm_writei` tap (`backend/audio.rs`, the one the looper uses) adds the preview to
channels 0/1 (format and rate read from the PCM handle once). Each voice decays
over 1.2 s at level 0.5, and pads are consumed at the control-surface event hook
while the page is open, so the firmware never selects or switches the sample
underneath. The saved render is peak-scaled to 0.86. The DaisySP subset is
ported into the crate (upstream revision `2c72eaf9…`, MIT); no runtime library
and no C is added.

**Evidence.** `Bass C29.wav`: 66,150 stereo frames at 44.1 kHz, C2 fundamental
(measured 69 Hz peak with 1 Hz-bin resolution), RMS 0.06421, pad assigned,
`Bass Synth: copy created`, no restart. Live preview is now checked from the
emulator's PCM stream, not only from counters: one pad gives an audible pluck
(peak ≈0.49, ≈0.13 RMS) that decays and stops after 1.2 s, with no growth.

**Limits.** The native FMBass instrument is unresolved (see
[`MECHANISMS.md`](MECHANISMS.md#h3-instrument-host-boundary-fmbass)); the
shipped renderer is the local DaisySP model, which sounds like a pluck, not the
firmware's synth. Real hardware untested. The investigation probe for the native class
(`fmbass_sample.c`) was never shipped and went with the C; it is in `main`.

---

## Build and verification

```sh
cd mod
./build.sh          # release .so, 0 warnings, glibc symbol versions <= 2.39
./pack.sh           # SD package with a valid autorun.check

# host-side checks (no emulator, no device)
cd ../rust && ./check.sh         # fmt, clippy, rustdoc, unit tests, golden DSP fixtures, aarch64 build
```

Emulator scenarios need the sibling `virtual-mpc-sample` checkout and a preloaded
library (`lab.py preload <repo>/mod/build/minimod.so`). Run each from a fresh
`systemctl restart ac50` in the guest: while SAMPLE OPTIONS is open the mod owns
the panel, so a leftover menu swallows the next test's keys.

```sh
python3 mod/tests/emulator_menu.py --panel http://127.0.0.1:8765
python3 mod/tests/emulator_autotrim.py --lab ../virtual-mpc-sample --pad 1
python3 mod/tests/emulator_spectral.py --lab ../virtual-mpc-sample --boots 1
python3 mod/tests/emulator_reverb.py --lab ../virtual-mpc-sample --effect reverb
python3 mod/tests/emulator_bass_sample.py --lab ../virtual-mpc-sample
python3 mod/tests/emulator_midi_chromatic.py --lab ../virtual-mpc-sample  # Pad MIDI In on, Port = External
```

`emulator_autotrim` reads the trim bounds from the live model, so it works with
the release build. `emulator_spectral` and `emulator_reverb` pin a demo fixture
and say so when it is missing. The 16-Levels pitch shift and the MIDI transform
outside 16 Levels are covered by the hardware runs recorded above.
