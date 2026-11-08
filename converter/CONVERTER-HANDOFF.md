# Converter status and limitations

State of `mpc2stems` — an offline converter that turns an Akai MPC Sample project
(`.xpj` + its sample folder) into per-pad WAV stems, matching the device's own
mixdown as closely as an offline model can.

Last updated: 2026-11-02. This is the canonical current-state document; the
evidence behind every claim is in [`docs/FINDINGS.md`](docs/FINDINGS.md) and the
rejected alternatives are in [`HYPOTHESES.csv`](../HYPOTHESES.csv).

---

## 1. Current status

Reference: the device's own mixdown `project1-mixdown/Song1.wav` — 1,286,916
frames, 29.1818 s, 12 pads playing (0, 1, 2, 9, 10, 13, 14, 16, 17, 18, 20, 27).

| mode | command | residual | corr |
| --- | --- | --- | --- |
| pure model | `convert` | **0.1783822584** | 0.9441 |
| correct levels, no gate | `convert --fidelity` | 0.1966535484 | 0.9305 |
| model + reference separation | `convert --reference … --refine` | 1.81e-16 | — |
| same, quantised | `--refine --exact` | 0 LSB over 2,573,832 samples | 1.0000 |

39 tests pass. The pure model needs no reference; `--refine` and `--exact`
require one and are the modes that produce stems summing exactly to the mixdown.

Where the pure-model error sits, by bandwidth: 20–250 Hz 0.1353 (61 % of error
energy), 250 Hz–5 kHz 0.1048 (37 %), 5–20 kHz 0.0239 (2 %). It is distributed
across the ordinary pads as small per-band magnitude differences, not concentrated
in one defective stage.

---

## 2. What is modelled

Everything below is validated against the firmware or the device's output; the
section references point at the evidence.

* **Container and sequencer** — gzip → `ACVS` header → JSON, schema 28, 960 PPQ,
  song = sequence *keys* `[3,4,3]`, pad = `note − 36` (`docs/FORMAT.md`).
* **Voice** — layer/velocity selection, tuning, reverse playback, loop, slices,
  mute groups, ratchets, note ring-out, pan, pad/layer/track/master gain staging.
* **Envelopes and LFOs** — amp, filter and pitch envelopes; `RenderParams` carries
  the envelope scales.
* **Warp rate** — rate = `project_bpm / sample_tempo`; pitch-shift happens *after*
  stretching, and a stretched pad is gated to its note's own length (§71).
* **Filters** — the firmware's 29-value `FILTER_TYPE` enum (§80) and the device's
  **Chamberlin state-variable filter** topology, which is now the default
  (`filter_topology="svf"`, §85). Types 1–22 are modelled; type 29 (`MPC3000 LPF`)
  is modelled as a 2-pole low-pass, which is what both the mixdown and a pad-16
  capture support.
* **Bus** — the Color Compressor as a fast limiter (attack ≈15 ms, release ≈1 ms,
  threshold −3 dB, ratio 3:1, auto makeup), applied as a single broadband gain.
* **Device latency** — 90.6 samples (`25.056830·pulse + 90.633`), `latency=91`.

The filter enum, the SVF recurrence and the coefficient forms are documented with
firmware addresses in [`docs/FORMAT.md`](docs/FORMAT.md) §filter.

---

## 3. Limitations

Ordered by how much they affect the result.

### 3.1 The élastique floor — the largest and least fixable

Five pads are time-stretched by zplane élastique Pro V3. The *rate* is reproduced
exactly (0.0 cents of pitch error) and the gating is right, but the stretch
algorithm is only approximated by a phase vocoder, so those waveforms' **phase**
does not match the device. No offline reimplementation reproduces élastique's
phase, and this is where a large part of the residual lives.

This is not a bug to fix; it is the boundary of what a model can do. The modes
that sidestep it are `--refine` (recovers phase detail from a reference) and
`--refine --exact` (bit-exact sum).

### 3.2 The stretched-pad gate — a deliberate trade-off, switchable

By default a stretched pad is truncated to its note length. That is worth ~1.7 dB
of residual but costs real level: **pad 10 renders 21.6 dB quieter than the device**
(§78). Without the gate the same pad is 9.8 dB too loud.

`--fidelity` disables the gate. It gives back the level and reports the worse
number (0.1967 vs 0.1784). The default keeps the gate because the metric is
dominated by phase-mismatched content, where muting always scores better — see
3.6. Which of the two to ship is a product decision, not a modelling one.

### 3.3 Per-pad levels are not independently verified

Per-pad captures exist (`out-capture/audio/`) but the emulator's audio tap has an
unknown, per-take gain and drops frames, so it is usable for **shape**, never for
absolute level. Two consequences:

* the device's pad-volume law looks like `v²` (fit R² = 0.73, implied exponent
  2.1) while the renderer applies it linearly, but correcting it makes the
  mixdown *worse* — current errors compensate, so it cannot be settled from a
  mixdown;
* the warped-pad level error in 3.2 cannot be separated from the gate's effect
  without a sample-accurate export.

Closing this needs the app to write its own WAV from inside the guest. That is
reachable — `LD_PRELOAD` is proven to interpose inside `MPC Sample`, and `malloc`
interposition fires (§82) — but it is not implemented yet.

### 3.4 Filter coefficient curves are approximate

The SVF *topology* is exact: `low += f·band; high = x − low − qd·band;
band += f·high`, with the type selecting the tap. The **mapping from cutoff and
resonance to `f` and `qd`** is not: `f = 2·sin(π·fc/fs)` and `qd = 1/Q` are the
textbook forms, while the firmware ramps them from fields written by setters at
`0x274e3bc`, `0x274e534`, `0x274e6b4` that are still undecoded (§85).

Types 23–28 (`Model1..3`, `Vocal1..3`) are separate DSP inside the firmware and are
**not modelled**; `audit.py` reports them and the pad renders without its filter
rather than with an invented one.

### 3.5 Unmodelled master inserts

`Outputs 1/2` carries `Pad-Controlled FXs`, `Flex Beat Player` and
`Color Compressor`; `Submix 1` carries an `LP Filter`. Only the compressor is
modelled. They are idle in this project, so the numbers above are unaffected, but
a project that uses Flex Beat or the pad-controlled FX would convert wrongly —
`audit.py` now warns about every enabled insert it does not model.

### 3.6 The metric has a blind spot

For phase-mismatched content the error is `|Y|² + |X|²`, so **attenuating a
wrong-phase stem always lowers the residual**. Optimising for the number can
therefore make the audio worse, and twice a lower number was bought that way
(the gate, and an earlier 3–4 dB-quiet stretcher). Treat 0.1784 as "close to the
mixdown", not as "each stem is right"; per-pad accuracy is stated in §78 for the
pads where it can be measured.

### 3.7 Smaller items

* `RenderParams.attack_scale_s` and `decay_scale_s` are declared and never read —
  dead knobs that look configurable.
* `filter_topology="biquad"` is kept for comparison; it scores 0.1783940889, i.e.
  1.2e-5 worse, and exists so the measurement is reproducible.
* `validate` and `verify` overlap; `verify` is the strict, bit-exact one.
* One project per invocation — no batch mode, no real-time use.
* Capturing per-pad audio from the emulator is documented in
  [`docs/DEVICE-PROBE.md`](docs/DEVICE-PROBE.md) but its level accuracy is the
  limitation in 3.3.

---

## 4. How it is used

```
uv run mpc2stems info     <project.xpj>
uv run mpc2stems convert  <project.xpj> --out DIR [options]
uv run mpc2stems validate <stems-dir-or-project> <mixdown.wav>
uv run mpc2stems verify   <stems-dir> <mixdown.wav>
uv run mpc2stems solo-projects <project.xpj> --out DIR
```

| flag | effect |
| --- | --- |
| `--out DIR` | output directory (required) |
| `--reference MIX.WAV` | the device mixdown; enables `--calibrate`, `--refine`, `--exact` |
| `--no-master` | dry stems — skip the Color Compressor |
| `--fidelity` | do not gate stretched pads to their note length (§3.2) |
| `--calibrate` | fit per-pad gains against the reference before writing |
| `--refine` | recover each stem from the reference with a model-guided Wiener mask |
| `--exact` | distribute the master residual so the stems sum bit-exactly |
| `--latency N` | override the device output latency (default 91 samples) |

Which mode:

* **exact song in a DAW** → `--reference … --refine --exact`;
* **own mix/master** → `--no-master`;
* **no reference** → plain `convert` (0.1784), accepting §3.1–3.2;
* **verify stems against the device** → `verify` (`validate` is the looser check).

Stem filenames are `NN-<sample-slug>-<tempo>v.wav`, e.g. `00-rnb-kik-grove1-91v.wav`.

---

## 5. Layout and verification

```
src/mpc2stems/
  xpj.py        container parse (gzip -> ACVS header -> JSON)
  model.py      Project, Sequence, TrackMix, Pad, Layer, Note
  wav.py        WAV read/write, PCM24, RIFF 'slic' chunk
  envelope.py   amp/filter/pitch envelopes, LFOs
  filters.py    filter enum, JUCE coefficient forms, Chamberlin SVF, cutoff map
  warp.py       élastique substitute: STFT phase vocoder (512/64)
  render.py     voice building, event expansion, per-pad rendering (largest)
  master.py     Color Compressor model, bus gain
  separate.py   Wiener-mask refinement against a reference
  exact.py      24-bit integer residual distribution
  audit.py      warns about project features that are not modelled
  cli.py        info / convert / validate / verify / solo-projects
tools/re/       fwxref.py (adrp/add xrefs), rtti.py (RTTI -> vtable), calls.py
tools/device/   LD_PRELOAD probes for the device and the emulator
tests/          test_project.py — 39 tests
```

```sh
uv run pytest -q tests                       # 39 pass
uv run mpc2stems verify out/stems project1-mixdown/Song1.wav
#   sum == mix : True
#   max |diff| : 0 LSB
```

Reproduce the output sets:

```sh
uv run mpc2stems convert project1/bapap1.xpj --out out \
    --reference project1-mixdown/Song1.wav --refine --exact
uv run mpc2stems convert project1/bapap1.xpj --out out-dry \
    --reference project1-mixdown/Song1.wav --no-master
```

---

## 6. Related documents

| document | contents |
| --- | --- |
| [`docs/FORMAT.md`](docs/FORMAT.md) | the `.xpj` format field by field; the filter enum, coefficient forms and SVF topology with firmware addresses |
| [`docs/FINDINGS.md`](docs/FINDINGS.md) | the evidence log — measurements, firmware findings, disproved hypotheses (85 sections) |
| [`HYPOTHESES.csv`](../HYPOTHESES.csv) | debugging ledger — 102 hypotheses with proved/disproved verdicts |
| [`docs/DEVICE-PROBE.md`](docs/DEVICE-PROBE.md) | getting ground truth off the device and the emulator; `LD_PRELOAD` technique |
| [`../docs/MODDING.md`](../docs/MODDING.md) | the device's SD-card autorun and how the community mod patches the app in RAM |
| [`../docs/EMULATOR.md`](../docs/EMULATOR.md) | running the virtual MPC Sample |
| [`../docs/MODDING.md#6-the-ecosystem`](../docs/MODDING.md#6-the-ecosystem) | how custom firmware boots, and the community tools |
| [`../docs/FEATURES.md`](../docs/FEATURES.md) | `mpc-mini`, the minimal mod |
| [`CONTEXT.md`](../CONTEXT.md) | glossary (song, sequence, pad, mixdown, stem, Color Compressor) |
| [`README.md`](README.md) | short overview and the modelled / not-modelled lists |
| [`USER-TASK.md`](USER-TASK.md) | the manual per-pad export procedure for a real device |
