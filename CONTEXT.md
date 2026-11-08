# CONTEXT

Glossary for the MPC Sample domain. Terms only — no implementation, no plans.

> **Scope:** this glossary belongs to the **converter** (`converter/`, mpc2stems:
> song → per-pad stems). It is not the mod's vocabulary; for `minimod`
> (the `LD_PRELOAD` firmware mod) start at [`README.md`](README.md),
> [`mod/README.md`](mod/README.md) and
> [`docs/re/FIRMWARE-MECHANISMS.md`](docs/re/FIRMWARE-MECHANISMS.md).

## Source material

**Song** — the arrangement in Song mode: an ordered list of sequences with repeat
counts. This is what gets exported.

**Sequence** — a fixed-length pattern, counted in bars, holding clips. Carries its
own BPM.

**Program** — the instrument holding 128 pads.

**Pad** — one of the 128 drum pads of a program. A pad owns up to 8 layers, but in
practice one sample. The unit a **stem** is named after.

## Audio

**Mixdown** — what the device's *Export mixdown* produces: the whole song summed
through the master insert chain, written as one stereo WAV. In the reference
project: 29.1818 s.

**Pre-master sum** — the song with all pads summed but *before* the master insert
chain. What dry stems add up to.

**Residual** — RMS of (mixdown − sum of stems): the project's fidelity metric.
It is a *waveform* measure, so it cannot fairly judge content whose phase is
unreproducible. For incoherent audio the error is `|Y|² + |X|²`, so attenuating
that content always lowers the residual; the honest figure and the flattering one
therefore differ: the same model scores 0.1787 with the stretcher 3–4 dB quiet and
0.1854 with its level correct (FINDINGS §72). The current model measures
**0.1784** (whole-mix correlation 0.9441); quote the figure that matches what is
shipped.

**Stem** — the audio of a single pad across the whole song, rendered on its own.
In this project a stem is always **dry**: it does not include the master insert
chain.

**Master insert chain** — processing applied to the summed song, after all pads.
The project declares three inserts (Pad-Controlled FXs, Flex Beat Player, Color
Compressor), but by measurement only the **Color Compressor** is audible: "broadband"
matters, because its gain is one scalar over time, not per band.

What it measurably *is*, from fitting the bus against the mixdown, is a **fast
limiter** — attack ≈15 ms, release ≈1 ms, −3 dB, 3:1, 5 ms RMS detector,
automatic makeup — not the slow 72/119 ms compressor the front-panel knob
positions imply (FINDINGS §71). It is not measurably a saturator: a static
non-linearity (tanh, any drive/mix) and a low shelf were both tested on the bus
and neither helps (§72). The stage's real parameters are in the opaque
`plugin.state` blob of the insert, which is decoded but not yet unpacked (§72).

**Filter type** — the pad filter's mode, a 29-value enum: Lowpass/Highpass/
Bandpass/Bandstop/Bandboost at 1–8 poles, then `Model1..3`, `Vocal1..3` and the
`MPC3000 LPF` emulation. Values are the firmware's, not a guess.

**SVF / state-variable filter** — the topology the device runs on each pad: one
filter that produces low, band and high outputs at once, where the *filter type*
selects which output is heard ("the tap"). Its cutoff and resonance become the
pair of coefficients `f` and `qd`.

**Tap** — which of the state-variable filter's outputs a filter type emits.

## Export options

**Export Master FX** — whether stems pass through the master insert chain. Stems
in this project are always dry, i.e. the option is fixed off.

**Export Tail** — whether the export continues past the last bar so ringing
samples decay instead of being cut. Fixed on, +3 s, matching the stock mixdown.

**Export BPM in Name** — whether the tempo appears in the exported file name.
