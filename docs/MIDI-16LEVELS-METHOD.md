# How the MIDI → 16 Levels problem was actually solved

A retrospective on the methods, tests and tools that produced the breakthroughs,
written so the next firmware feature can reuse them. Companion documents:
[RE-MIDI-16LEVELS-FINDINGS.md](RE-MIDI-16LEVELS-FINDINGS.md) (the decompiled
facts) and [../HYPOTHESES.csv](../HYPOTHESES.csv) (the hypothesis ledger).

## 1. The ground-truth hierarchy

Every claim in this investigation was ranked by authority, and tests were
designed to move one step up this ladder:

1. **The shipped artifact** — same binary hash in the emulator as on the device
   (`b6584cba…`), so emulator results transfer.
2. **Measured audio / measured registers** — PCM recordings, correlation, FFT,
   captured call arguments. Never "it sounds right".
3. **Decompiled code** — Ghidra on the already-analyzed project, after the
   runtime contract was known.
4. **Reference implementation** — SampQOL loaded in the same emulator.
5. **Hypothesis / intuition** — cheapest, weakest, only used to pick the next test.

The rule that saved the most time: *when a measurement and a hypothesis
disagree, the measurement wins, but first prove the measurement is clean.*

## 2. The five breakthroughs, in order

### 2.1 SampQOL as an oracle (proved the feature was possible)

Loading SampQOL v0.1.5 into the same emulator and playing the same MIDI notes
showed selected-pad chromatic playback was achievable:

* MIDI 60 vs selected pad A01: correlation **0.997**
* after selecting pad 5, MIDI 60 vs pad 5: **1.000**
* stock/minimod: maximum 0.05

Its own log `/tmp/mpcqol.log` then listed the exact hook map
(`keys-evin 0x211b090`, `keys-nrsend 0x211a690`, `keys-prognote 0x286fcf0`),
which told us where to look instead of guessing.

**Lesson:** find a working implementation and measure it before reverse
engineering anything.

### 2.2 Byte-level event capture at the dispatcher

The single most productive instrument was dumping the 64-byte note event at the
dispatcher hook and diffing physical pad events against synthesized ones:

```
PAD     ... 01 00 7a 78 ... 0e ...
MIDI->  ... 01 00 7a 78 ... 0e ...
```

Fields that mattered were discovered this way, one at a time:

* `event+0x14` — property mask (u16)
* `event+0x16` — property 0 value (pad note / level / pitch)
* `event+0x30` — selected pad (Velocity form) or row slot (Tune form)

The event is a compact struct; diffing two 64-byte dumps is faster than any
amount of speculation.

**Lesson:** when two paths behave differently, capture the data structure at the
last common point and diff it.

### 2.3 Capturing the app's own call arguments

The final breakthrough came from instrumenting the mod's existing `pitch_off`
hook (the PLUS/MINUS octave feature) to log the app's own call to the property
setter:

```
pitchcall a0=event+0x14 a1=0 a2=0 w3=2  e30=0e   (PAD 6, level 5, origPad 3)
pitchcall ...                       w3=-3 e30=0e   (PAD 1, level 0)
```

That gave the exact contract: **`w3 = level − originalPad`** (signed semitones),
`event+0x30 = selected pad`. Reproducing the app's call for MIDI with
`w3 = note − 60` is what made full-range chromatic work.

The hint came from the user: the mod's own octave shifting already pitched
outside the 16-pad row. Look for a mechanism the codebase already trusts.

**Lesson:** reproduce the firmware's own call, with the firmware's own arguments,
before inventing a new event.

### 2.4 A controlled sample for pitch (sine)

A guitar phrase is useless for pitch measurement: harmonics and FFT peaks lie.
Replacing the selected pad's WAV with a 3 s 220 Hz sine (same 24-bit/44.1k/stereo
format, then restoring the original) produced unambiguous numbers:

| note | offset | measured | expected |
|---|---|---|---|
| 48 | −12 | 110.4 Hz | 110.0 |
| 60 | 0 | 220.7 Hz | 220.0 |
| 65 | +5 | 293.4 Hz | 293.7 |

**Lesson:** when measuring a parameter, replace the unknown with a known signal.

### 2.5 Decompilation from an already-analyzed Ghidra project

The project (`~/mcpsamplebin`, program `mpcsample.bin`) was already analyzed
(69k functions). Running the repo's own script headless opened it in seconds:

```sh
analyzeHeadless ~ mcpsamplebin -process mpcsample.bin -noanalysis \
  -scriptPath tools/ghidra/hpss \
  -postScript ResearchDecompile.java 0x221b090 0x221aa30 0x22472f0 0x28e4400
```

Ghidra VA = file VA + `0x100000`. The decompiled code explained the runtime
observations:

* `FUN_0221b090` — routing keyed by **source**; slot comes from `event+0x30`.
* `FUN_022472f0` — 16 Levels writes exactly one parameter slot for level 0..15.
* `FUN_028e4400` — normalizes a value into `props[index+2]`; index 0 range
  −120..120, 241 steps.

**Lesson:** decompile *after* you have a runtime contract to check it against;
then the pseudocode is confirmation, not exploration.

## 3. Methods that paid off repeatedly

* **State verification before every experiment.** The mod's dispatcher bytes are
  a perfect state probe: `d503233f` = stock, `155f947c` = hook installed. Pad
  MIDI In resets to Off on every app start, and the 16 Levels dialog toggles the
  mode when opened. Most "it doesn't work" runs were invalid state, not bad code.
* **Silence-separated recordings.** The false "note 48 = 247.6 Hz" was PAD 6's
  tail (220·2^(2/12) = 246.9 Hz). Record one event at a time, after silence.
* **RMS + correlation together.** RMS distinguishes "same sample vs different
  sample" (uniform 300..600 vs 4256/16269); correlation proves exact identity
  (1.000) when the same pitch is expected.
* **A hypothesis ledger.** HYPOTHESES.csv forced one hypothesis, one test, one
  recorded conclusion; disproved rows were as useful as proved ones.
* **The user's hardware feedback.** "Pad MIDI In was off", "octave shifting
  already pitches outside the row", "only from C upwards" each redirected the
  investigation faster than any log.

## 4. Traps and how to avoid them

| Trap | Cost | Avoidance |
|---|---|---|
| Assuming the app state (Pad MIDI In, 16 Levels, mode) | Several invalid test rounds | Read the site bytes and screenshot the config before every run |
| Correlation on similar samples | Misleading 0.6–0.9 scores | Use a tonal/unique sample and compare exact expected pitch |
| FFT peak on a phrase | Random frequencies | Use a sine or autocorrelation on a clean window |
| Recording tails | False negative for note 48 | 1–2 s silence before each event |
| Wrong Ghidra address base | "no function" on two key functions | VA = file VA + 0x100000 |
| Deleting an open log file | Lost diagnostics | truncate in place (`: > file`) |
| UI automation of modal dialogs | Mode toggles, wrong type | Verify state after each UI action; prefer one manual arm |

## 5. Reusable checklist for the next firmware feature

1. Confirm the binary hash matches the device.
2. Find a working reference (stock behavior or another mod) and measure it.
3. Capture the data structure at the last common point (dispatcher/engine) and
   diff two paths.
4. Instrument the function the firmware already uses for the feature; log its
   arguments, not yours.
5. Reproduce that call with the firmware's own contract.
6. Verify state (bytes/screens) before every measurement; use a controlled signal.
7. Decompile the functions involved to confirm the contract and document ranges.
8. Record hypotheses, tests and conclusions as you go.
