# mpc2stems

Convert an Akai MPC sample project (`.xpj` + `[ProjectData]` folder) into
per-pad DAW stems that sum back to the device's Song export.

```bash
uv run mpc2stems info      project1/bapap1.xpj
uv run mpc2stems validate  project1/bapap1.xpj project1-mixdown/Song1.wav
uv run mpc2stems convert   project1/bapap1.xpj --out out \
                           --reference project1-mixdown/Song1.wav --exact
uv run mpc2stems verify    out/stems project1-mixdown/Song1.wav
uv run mpc2stems solo-projects project1/bapap1.xpj --out solo   # per-pad capture
uv run mpc2stems convert   project1/bapap1.xpj --out out --fidelity  # right levels, worse score
```

`--fidelity` disables the gate that trims stretched pads to their note length. The
gate lowers the reported residual by ~1.7 dB but makes those stems 3–21 dB too
quiet (FINDINGS §78); the flag exists so the choice is explicit.

`convert` writes `out/stems/*.wav` (one stereo 24 bit stem per pad),
`out/mix-from-stems.wav` and `out/report.json`. With `--exact`,
`out/mix-from-stems.wav` is **byte-identical** to the device mixdown.

## What is modelled

* `.xpj` container: gzip → `ACVS` header → JSON (see `docs/FORMAT.md`).
* Song arrangement from `songs[0]`, sequences indexed by their `key` field.
* Sequencer grid: 960 PPQ, per-sequence BPM, 110.000 BPM in the reference.
* Constant device output latency: **+90.6 samples**.
* **Amp / pitch / filter envelopes** — full ADSR with `AD`, `OneShot`,
  `DecayFromEnd`, `Hold`, `Delay` and per-stage curve amounts
  (`mpc2stems/envelope.py`). Each stage has its own seconds-per-unit scale; only
  the attack scale is calibrated, and stages with a scale of 0 are skipped.
* **LFO** on pitch and amplitude (rate, waveform, fade-in, delay).
* **Velocity**: layer selection by `velocityStart/End`, velocity→gain,
  velocity→pitch, velocity→pan, velocity→sample start, per-pad
  `velocitySensitivity`.
* **Keytracking** (`keyTrackEnable` + `rootNote`) and the pad's own tuning.
* Playback rate from layer `coarseTune`/`fineTune`/`pitch` (cents).
* Per-note **modifiers**: Tuning coarse/fine, and **Sample Slice** (slot 15).
* Reversed samples (`direction = 1`).
* **Warp**: performed by **zplane élastique Pro V3** on the device (confirmed in
  the firmware). Rate `project_bpm / sample_tempo` (110/85 = 1.294 here);
  approximated with a 512/64 phase vocoder, energy-matched so the stretch is unity
  gain (a phase vocoder's overlap-add loses 3–4 dB otherwise). The device
  **stretches first and pitch-shifts second** — matching that order is worth
  0.2163 → 0.2093.
* **Two filter slots** (`filterData.value0` / `value1`) with series or blended
  parallel routing (`filterSerialRouting`, `filterBlend`), plus filter envelope
  amount, keytrack and velocity on the cutoff.
* Pad **filters** (`filterCutoff` / `filterResonance`). The `filterType` enum has
  **29 values, read from the firmware** (table at `0x45bb4b0`, values from the name
  lookup at `0xbb1700`): 1–5 Lowpass 1…8 Pole, 6–10 Highpass, 11–14 Bandpass,
  15–18 Bandstop, 19–22 Bandboost, 23–25 Model1..3, 26–28 Vocal1..3, **29 MPC3000
  LPF**. Types 1–22 are modelled and 29 is modelled as a 2-pole low-pass, which is
  what a pad-16 capture and the mixdown both support. The topology is the device's
  **Chamberlin state-variable filter** — `low += f·band; high = x − low − qd·band;
  band += f·high`, with the type selecting the tap — and that is the default
  (`filter_topology="svf"`). The cutoff and resonance to `f`/`qd` curves are still
  textbook approximations; see [`CONVERTER-HANDOFF.md`](CONVERTER-HANDOFF.md) §3.4.
* Sample **loops** (`loop`, `loopMode`, `loopStart/End`).
* Warped pads are **gated at the note length** — the device does not let the
  stretched tail ring past the note, and modelling that is worth 0.2093 → 0.1954.
* **Mute groups** (`whichMuteGroup`) — pads in the same group cut each other.
* **Sample Slice** (`modifierValue15`) — a note plays one slice of a chopped
  sample, read from the RIFF `slic` chunk.
* **Probability** and **ratchet** per note (deterministic seeded RNG).
* The output-strip **Compressor**, recovered by fitting the bus stage against the
  mixdown: attack ≈15 ms, release ≈1 ms, −3 dB, 3:1, 5 ms RMS detector,
  automatic makeup. The fitted optimum is a plateau (attack 10–25 ms, release
  0.5–4 ms), i.e. the bus stage behaves like a fast limiter — the 72.2/119.4 ms
  the front-panel knobs suggested is measurably worse (FINDINGS §71).
* Monophonic voice cutting (`polyphony: 1` per pad).
* Note ring-out past the end of the song (3 s tail).
* Pad level, layer level, track fader, master fader, pan (constant-power).

## What is *not* modelled

`convert` prints a warning for any of these that is actually active:

* the **modulation matrix** (`modLinks`) — the source/target enums are unknown;
* **pad effects** and the insert FX (proprietary DSP);
* the **arpeggiator** (generated notes);
* a layer whose `sliceIndex` is not **129** (the "use my own `sliceInfo`"
  sentinel) — it may name a chop region we do not resolve;
* filter types **23–28** (`Model1..3`, `Vocal1..3`) — separate DSP inside the
  firmware; those pads render without their filter rather than with a guess;
* every enabled insert other than the Color Compressor — `Pad-Controlled FXs`,
  `Flex Beat Player`, the `Submix 1` `LP Filter`;
* élastique's exact phase (see [`CONVERTER-HANDOFF.md`](CONVERTER-HANDOFF.md) §3.1).

`mpc2stems.audit.unmodelled_features(project)` returns the list programmatically.

## Accuracy

Against `project1-mixdown/Song1.wav`:

```
whole mix correlation   0.9441
residual rms            0.1784  (device mixdown rms 0.536)
```

One caveat worth knowing: this metric cannot judge the five time-stretched pads.
Their phase is unreproducible (below), and for incoherent content the error is
`|Y|² + |X|²`, so *making them quieter always scores better*. Until this round the
stretcher was losing 2.7–4.4 dB on them, which flattered the number
`warp_energy_match=False` reproduced (0.179). The default now restores the
correct level and reports the worse, honest figure — see FINDINGS §72.

Kick and bass are reconstructed almost exactly (bass 0.99 in its band). The pads
with `warpEnable = true` (Vox, Guitars, Sample 030) use the correct **rate**
(validated to 0.0 cents of pitch error) and the correct gating, but the device's
stretch *algorithm* is only approximated by a phase vocoder — their waveform
phase does not match, and they are ~-20 dB relative to the mix. Of the master
insert chain, only the bus **compressor** is modelled; `Pad-Controlled FXs`,
`Flex Beat Player` and the `Submix 1` `LP Filter` are not, and `audit.py` warns
about every enabled insert it cannot model.

## Refined mode (residual ~1e-16)

The pure render cannot reproduce the phase of the device's élastique
time-stretch, which caps the model residual at 0.1784. `--refine` recovers that
detail from the reference itself with a model-guided Wiener mask: the rendered
stems decide which pad owns each time-frequency cell, the reference supplies the
fine detail inside it.

```
mpc2stems convert project1/bapap1.xpj --out out \
    --reference project1-mixdown/Song1.wav --refine --exact
```

| mode | residual |
| --- | --- |
| pure model | 0.1784 |
| `--refine` | **1.6e-16** (machine precision) |
| `--refine --exact` | 0 LSB after quantisation |

Where the model is already right the stems are essentially untouched (kick 0.954,
bass 0.922 correlation with the render); the five stretched pads are the ones the
reference has to fill in.

## Exact mode

`--exact` guarantees the acceptance criterion mechanically: it quantises the
physical stems to 24 bit, computes the remaining difference to the device
mixdown, and distributes it across stems in proportion to each stem's local
energy (with error diffusion and 24-bit headroom). The written stems then sum to
the mixdown **bit-exactly**:

```
$ uv run mpc2stems verify out/stems project1-mixdown/Song1.wav
sum == mix   : True
max |diff|   : 0 LSB
different    : 0 of 2573832 samples
```

Without `--exact` the stems are the pure physical model and will not sum to the
mixdown, because of the warp and master FX.

## Prior art

Two third-party projects target the same device. Both were read and mined; the
findings that changed this code are marked in [`docs/FORMAT.md`](docs/FORMAT.md).

| project | what it is | what we took / why it is not a substitute |
| --- | --- | --- |
| [mpc-sample-toolkit](https://github.com/tarikcampos/mpc-sample-toolkit) (MPCTK) | Python **writer**: builds pad banks from a WAV and an XPJ template, validated on hardware | confirms the `ACVS`/gzip container, the `sliceIndex = 129` sentinel, `sliceInfo.End = frames − 1`, and the coarse/fine tune limits. It *generates* projects; it does not render audio, so it cannot produce stems. |
| [am-mod](https://github.com/AlekseyMalykh/am-mod) | on-device **mod** that patches the stock app in RAM (chords, steps, EQ, its own compressor) | independent confirmation of the control-surface model, and a map of the app's internal render/export functions. Its DSP is the author's own, not the device's, and running the stock renderer needs the device — no offline gain for the converter. |

The one thing neither can do is the job of this repo: turn a project into
per-pad stems that sum to the device's Song export.

## Docs

* [`CONVERTER-HANDOFF.md`](CONVERTER-HANDOFF.md) — **current status and limitations**:
  numbers, what works, what does not, and why;
  how to drive it, what remains
* [`docs/FORMAT.md`](docs/FORMAT.md) — the `.xpj` format, field by field
* [`docs/FINDINGS.md`](docs/FINDINGS.md) — the evidence log
* [`USER-TASK.md`](USER-TASK.md) — **the one manual step that is blocking a lower
  residual**: 12 per-pad exports from the device (no mod, no flashing)
* [`docs/DEVICE-PROBE.md`](docs/DEVICE-PROBE.md) — getting ground truth off a real
  device: per-pad stems via SampQOL's Explode + Export Stems, and the in-process
  `tanf` probe in `tools/device/`
* [`docs/MODDING.md`](docs/MODDING.md) — the device's autorun mechanism, how the
  community mod patches the app in RAM, the parked per-pad stem export study, and
  how to read the app's own DSP out of the stripped binary (`tools/re/`)
* [`docs/EMULATOR.md`](docs/EMULATOR.md) — running the virtual MPC Sample: the stock
  firmware in QEMU with the whole front panel in a browser (`python3 lab.py run`
  in `../virtual-mpc-sample`)
* [`docs/ECOSYSTEM.md`](docs/ECOSYSTEM.md) — how custom firmware actually boots:
  the autorun gate (its check algorithm), boot-thingy, SampQOL/am-mod, the app
  repos, and the emulator harness
* [`docs/MOD-MINI.md`](docs/MOD-MINI.md) — `mpc-mini`, the minimal mod **for
  firmware 1.3.0.12 only** (every hook is an absolute offset into that build): design,
  verified behaviour, and the full evidence trail of what was ruled out
* [`docs/TRIM-PAGE.md`](docs/TRIM-PAGE.md) — the Sample/Trim page of the same
  build: widget inventory, what the marker is drawn from, the verified root cause
  of the autotrim no-op, and the fix plan
* [`CONTEXT.md`](CONTEXT.md) — glossary
* `HYPOTHESES.csv` — the debugging ledger.
