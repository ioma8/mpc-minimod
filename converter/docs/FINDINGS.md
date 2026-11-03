# Findings log — evidence and measurements

Running log of hard measurements plus the hypotheses they support. Each entry is
tagged:

* **[VALIDATED]** — measured directly, reproducible with the command shown.
* **[HYPOTHESIS]** — plausible explanation, not yet proven.
* **[DISPROVED]** — tested and rejected.

Reproduce anything below with `uv run python tools/…` (see `tools/`).

---

## 1. Reference files

| file | size | format |
| --- | --- | --- |
| `project1/bapap1.xpj` | 79,258 B | gzip → ACVS/JSON, decompressed 3,823,098 B |
| `project1-mixdown/Song1.wav` | 7,722,050 B | RIFF, PCM 24-bit, **2 ch**, 44,100 Hz, **1,286,916 frames** |

**[VALIDATED]** Mixdown duration = `1286916 / 44100 = 29.18177 s`.

**[VALIDATED]** `project1/bapap1_[ProjectData]/` contains 28 WAVs, one per
`data.samples[]` entry.

## 2. Song structure recovered from the mixdown itself

Method: cross-correlate a 4-second template taken from the head of the mixdown
with the whole file and look for peaks.

**[VALIDATED]** Peaks occur every **48,109 samples** = 1.0909 s = 2 beats at
110 BPM, and very strong peaks at

```
0, 384,873, 769,745   (+ 4 s tail)
```

`384,873` samples = 8.72728 s = 4 bars at 110 BPM. So the mixdown really is the
`[seq 3, seq 4, seq 3]` arrangement: three 4-bar sections.

**[VALIDATED]** Predicted song length = 46,080 pulses / 1,760 pulses·s⁻¹ =
26.18182 s = 1,154,618.2 samples. Mixdown has 1,286,916 frames, i.e. exactly
**3.0 s of extra tail** (132,298 samples ≈ 3.000 s).

## 3. Pad → sample mapping and zone ends

All 28 used pads have a sample in `layersv[0]`. Mapping pad → file is direct.

**[VALIDATED]** `layersv[0].sliceInfo.End` vs the file's real frame count:

* `End == nframes - 1` for 19 pads — i.e. `End` is the **last sample index** of
  the pad's zone, not an exclusive bound.
* `End == nframes` for 4 pads (`Sample 010`, `Sample 015`, `Chop 4`, `Chop 3`,
  `Chop 16-…`-variant).
* `End` is much smaller than `nframes` for 6 pads, i.e. the pad has its own end
  trim independent of the file:

| pad | file | nframes | zoneEnd |
| --- | --- | --- | --- |
| 10 | VoxPh-Grove1 G#m85 91V | 257,364 | 247,069 |
| 13 | GuitPh-Grove2 D#m85 91V | 62,259 | 14,319 |
| 16 | Sample 024-Trimmed | 88,200 | 42,304 |
| 18 | Chop 16 - Seq A04 1-Normalized | 24,062 | 14,597 |
| 21 | Chop 6 - Sample 018-… | 18,319 | 12,273 |
| 27 | Chop 1 - Sample 030-… | 22,993 | 8,967 |

**Consequence**: a converter must clip playback to `[zone_start, zone_end]`, not
to the file length. `[VALIDATED]` as a field relationship; the exact inclusive/
exclusive rule for playback is still a **[HYPOTHESIS]** (see §9).

## 4. Tuning

**[VALIDATED]** Tuning is stored on the layer only. `pads[].coarseTune/fineTune`
are 0 for all 28 pads. Layer values that matter:

| pad | coarse | fine | `pitch` field | cents |
| --- | --- | --- | --- | --- |
| 8 | 0 | −17 | −0.17 | −17 |
| 9 | 0 | −39 | −0.39 | −39 |
| 13 | 5 | −20 | 4.80 | +480 |
| 20 | −3 | 0 | −3.00 | −300 |
| 24 | 3 | 0 | 3.00 | +300 |
| 27 | −9 | 0 | −9.00 | −900 |

`pitch == coarseTune + fineTune/100` holds for every pad. Playback rate is
therefore expected to be `2^(cents/1200)`. **[HYPOTHESIS]** for the exact
resampling method; the *rate* itself is well supported by the field semantics.

## 5. Timing: the device has a fixed output offset

Method: render the kick (pad 0, unpitched, impulsive), cross-correlate against
the mixdown.

**[VALIDATED]** Best global lag for pad 0 = **+91 samples**, correlation 0.92
over windows where the kick dominates.

**[VALIDATED]** Windowed correlation of the whole linear model at lag **+92**:

```
t=0.00 s  corr 0.9796
t=0.10 s  corr 0.9894
t=0.20 s  corr 0.8711
```

so at the very start of the song the model is right to within ~1 sample and the
device output is delayed by **92 samples** relative to a render that starts a
note at sample 0.

**[VALIDATED]** The mixdown's first **92 samples are exactly zero**, and the
first non-zero sample is index 92. Consistent with the same 92-sample offset.

**[HYPOTHESIS]** The offset is a constant device/export latency applied to the
whole render. Why it is 92 and not a power of two is unknown.

## 6. The mixdown is hard-clipped

**[VALIDATED]** Peak sample value in both channels is exactly `+8,388,607` and
`−8,388,607` (24-bit full scale).

* 59,377 left-channel samples sit at `+FS`, 97,933 at `−FS`.
* 271,762 of 2,573,696 samples have `|v| ≥ FS−1` (~10.6%).
* The negative-side count is nearly double the positive side, which is the
  signature of an asymmetric (DC-biased) signal being clipped, not of a
  symmetric limiter.

**Consequence**: an exact match must reproduce the pre-clip signal and then clip
the same way. Clipping *hides* small errors wherever it engages, so accuracy has
to be judged in the unclipped regions.

## 7. Level: the mixdown is far hotter than the linear sum

**[VALIDATED]** Naive linear render (velocity × layer volume × pad volume × track
fader × master fader, constant-power pan) vs mixdown:

```
rms(model) = 0.0946      rms(mixdown) = 0.5601
best least-squares scale = 2.77   (9 dB)
```

**[VALIDATED]** The per-10 ms RMS ratio `mixdown / model` is **not constant**:

```
t = 0.00 s  ratio 8.0
t = 0.50 s  ratio 5.8
t = 4.50 s  ratio 14.9
t = 13.20 s ratio 14.1
t = 21.90 s ratio 17.3
t = 23.30 s ratio 3.1
```

A constant gain (fader) cannot explain a 3× … 17× spread. Something
**level-dependent or material-dependent** sits in the path.

## 8. Effects are present in the serialised mixer

**[VALIDATED]** Enabled inserts:

| strip | insert | `enable` |
| --- | --- | --- |
| Submix 1 | `LP Filter` | true |
| Outputs 1/2 | `Pad-Controlled FXs` | true |
| Outputs 1/2 | `Flex Beat Player` | true |
| Outputs 1/2 | `Color Compressor` | true |

**[VALIDATED]** Plugin `state` payloads are opaque encoded strings:
`"3242.AMjUSA...."` (Pad-Controlled FXs), `"16663.AMjUSA..."` (Flex Beat
Player). They are **not** plain base64 (they contain `.`), and have not been
decoded yet. **[HYPOTHESIS]** they are some variant of z85/ascii85 over a
zlib-compressed state blob.

**[HYPOTHESIS, leading]** The drum track is routed through the
`Outputs 1/2` strip, so the reference mixdown is *not* a linear sum of pads but
the output of `Pad-Controlled FXs → Flex Beat Player → Color Compressor`. This
would simultaneously explain (a) the 9–25 dB level increase, (b) the varying
ratio in §7, and (c) the hard clipping. It has **not** been proven yet; the
counter-evidence is that the head of the song matches the linear model at
corr 0.99.

## 9. Per-pad agreement with the mixdown

Method: render each pad alone, then for every 50 ms window where that pad is
audible, search the best lag and record the correlation.

**[VALIDATED]** (L channel, naive model)

| pad | sample | median lag | median corr |
| --- | --- | --- | --- |
| 0 | RnB-Kik-Grove1 91V | +91 | **0.920** |
| 9 | RnB-Bas-Grove2 G# 91V | −21 | 0.776 |
| 18 | Chop 16 - Seq A04 1-Norm | −88 | 0.535 |
| 16 | Sample 024-Trimmed | +107 | 0.211 |
| 10 | VoxPh-Grove1 G#m85 | −38 | **0.027** |
| 13 | GuitPh-Grove2 D#m85 | −30 | **0.064** |
| 14 | GuitPh-Grove1 C#m85 | −166 | 0.047 |
| 20 | Sample 030-…-Normalized | +3 | **0.016** |
| 27 | Chop 1 - Sample 030-… | −99 | 0.016 |

Two classes of behaviour:

* **Short/unpitched percussive pads (0, 9, 18) match well.**
* **The long melodic pads (10, 13, 14, 20, 27) do not match at all**, even
  though their note events are present and unmuted.

**[DISPROVED]** "Pad 20 (note 56) plays Sample 030 from its first frame at
t = 0". Correlating `mixdown[92 : 92+2000]` with `Sample030[0:2000]` gives
corr = 0.0035 (L) / 0.004 (R). If the linear model held, this would be ≈ 1 with a
scale factor.

**[HYPOTHESIS]** Either these long pads are processed by the Flex Beat / pad FX
chain, or their playback is warped/sliced in a way not yet modelled, or they are
not actually part of the exported mix.

## 10. Repeated song sections are *almost* identical

**[VALIDATED]** Song sections 0 and 2 are the same sequence (3). Comparing them:

* Median sample ratio `b2/b0` over unclipped samples = **1.0005** (i.e. most of
  the audio is bit-identical modulo the clip).
* Correlation 0.966 at lag −1.
* Differences are **localised** to specific moments (e.g. ≈ 7.36 s, ≈ 8.45 s in
  the first section), not spread as a smooth gain envelope.

**[DISPROVED]** "A slow compressor gain envelope is the only difference between
the sections" — a compressor would give a smooth ratio curve, and instead the
ratio is 1.000 almost everywhere with sharp local deviations.

**Consequence for §8**: any level-dependent processing must be *fast*
(limiter-like) rather than a slow compressor, or must act only on certain
material.

## 11. Open questions

1. What does `audioRoute.destination = 2` mean, and is track 0 inside the
   `Outputs 1/2` insert chain?
2. Can the plugin `state` strings be decoded to read the compressor / Flex Beat
   settings?
3. Why do the long melodic pads not correlate?
4. What is the exact zone-end / trim rule, and the exact 92-sample offset?
5. Which clipping/quantisation model turns the float mix into 24-bit PCM?

---

## 12. Precise timeline fit (24 kicks, whole song)

**[VALIDATED]** Cross-correlating the kick sample at every kick note of the song
(the song contains 24 kick notes) and fitting `sample = a·pulse + b` gives:

```
samples = 25.056830 · pulse + 90.633
```

* `25.056818` is exactly `44100·60/(110·960)` — the grid is 110.000 BPM at
  960 PPQ, confirmed to 6 significant digits.
* Implied tempo from the fit: **109.99995 BPM**.
* Constant device latency: **+90.6 samples** (the whole-song model correlates
  best at +92; see §5).

**[VALIDATED]** Kick correlation at these 24 positions: 0.92 … 0.96, i.e. the
mixdown is the sequencer's kick, not a live performance.

**[VALIDATED]** `mixdown[0:92]` is exactly zero; the first non-zero sample is 92.

## 13. Pads are monophonic

**[VALIDATED]** Every pad instrument has `monophonic: true`, `polyphony: 1`.
Ignoring this made the bass stem correlate 0.25 with the mixdown; cutting each
voice at the next note of the same pad raises it to **0.48** globally and 0.78
median when measured per note. Voice retrigger must be modelled.

**[VALIDATED]** Notes are also allowed to ring past the end of the song: the
renderer must not truncate at the song end. The mixdown's last 3 s contain the
last bass note (2.06 s sample starting at 25.91 s) ringing out — which matches
the observed audio cutoff at ≈27.7 s.

## 14. Band-limited comparison: which pads are actually present

Method: for each pad, pick the band (30–150 / 150–1200 / 1200–9000 Hz) where the
pad has most energy, band-pass both the pad's resampled voice and the mixdown,
and correlate at the pad's own note positions.

**[VALIDATED]** median / max correlation per pad:

| pad | sample | band (Hz) | median corr | max corr |
| --- | --- | --- | --- | --- |
| 0 | RnB-Kik-Grove1 | 30–150 | **0.919** | 0.934 |
| 9 | RnB-Bas-Grove2 | 30–150 | **0.778** | **0.999** |
| 14 | GuitPh-Grove1 | 150–1200 | 0.242 | 0.281 |
| 1 | RnB-Prc-Grove3 | 1200–9000 | 0.187 | 0.744 |
| 2 | RnB-Hat-Grove1 | 1200–9000 | 0.182 | **0.920** |
| 18 | Chop 16 - Seq A04 | 30–150 | 0.179 | 0.341 |
| 13 | GuitPh-Grove2 | 150–1200 | 0.042 | 0.164 |
| 17 | Sample 010 | 1200–9000 | 0.035 | 0.164 |
| 10 | VoxPh-Grove1 | 150–1200 | **0.028** | 0.161 |
| 20 | Sample 030 | 1200–9000 | −0.042 | 0.377 |
| 27 | Chop 1 - Sample 030 | 1200–9000 | 0.001 | 0.240 |

Reading:

* **Kick and bass are unambiguously in the mixdown** (0.92 / 0.99 peak).
* **Percussion is present but masked** — its peak correlations reach 0.92 (hat)
  and 0.74 (perc), while medians are low because other elements dominate the
  same windows.
* **The melodic pads (10, 13, 14) never exceed 0.28.** For a signal that is
  actually present and correctly modelled, the peak should approach 1 somewhere.

**[DISPROVED]** "The melodic samples are time-stretched to project tempo."
Scanning playback-rate multipliers 0.6…1.8 for pad 10 peaks at multiplier
**1.000** with correlation 0.066 — the sample's `metadata.tempo` (85 BPM) is not
applied as a warp.

## 15. Gain law: velocity curve, not compression

Method: isolate pad 9 (bass) by band-passing to 35–110 Hz, then fit a scalar
gain and correlation for every bass note.

**[VALIDATED]** Correlation 0.98–0.999 for 31 of 75 bass notes.

**[VALIDATED]** Fitted gain is **reproducible per velocity** across different
song sections (same velocity ⇒ same gain within 1–2 %):

| velocity | measured gain (mean) |
| --- | --- |
| 0.394 | 0.936 |
| 0.614 | 1.018 |
| 0.661 | 1.30 |
| 0.677 | 1.33 |
| 0.772 | 1.36 |
| 0.787 | 1.71 |
| 0.835 | 1.83 |
| 0.858 | 1.93 |
| 1.000 | 1.95 |

Fitted power law: `gain ≈ 1.80 · velocity^0.81`.

**[DISPROVED]** "The level differences are a compressor modulating the mix."
A compressor's gain would track the *programme level* and differ between sections
with different context. Instead identical velocity values give identical gains in
different sections, and the residual gain scatter is 1–2 %.

**[VALIDATED]** The absolute gain is much higher than the fader model predicts:
for the bass the measured gain is **≈6.3×** the value obtained from
`velocity × pad volume × layer volume × track fader (0.7079) × master fader
(0.7079) × constant-power pan`.

## 16. The master is clipping hard, not limiting smoothly

**[VALIDATED]** With the calibrated gain the model's peak is **2.88** (i.e.
+9.2 dBFS) while the mixdown's peak is exactly 1.0 with ~10 % of samples sitting
at ±full-scale. So a hard clip is applied, not a look-ahead limiter (a limiter
would have kept more of the waveform shape and produced fewer full-scale
samples).

## 17. What still does not match

**[VALIDATED]** After calibration (gain 5.81, velocity^0.81, monophony, tail),
correlation with the mixdown in *quiet* regions (where clipping cannot act):

| region | correlation |
| --- | --- |
| \|mix\| < 0.1 | 0.11 |
| \|mix\| < 0.2 | 0.20 |
| \|mix\| < 0.3 | 0.27 |
| \|mix\| < 0.5 | 0.37 |

Band-limited, \|mix\| < 0.3: 30–120 Hz = 0.47, 120–500 Hz = 0.18,
**500–2000 Hz = −0.20**, 2000–9000 Hz = 0.04.

**[VALIDATED]** In the very quiet tail (28.5 s, rms 0.019) the mixdown's dominant
partial is **828 Hz**, while the calibrated model has 494 Hz / 986 Hz there — the
model's residual tail content is not what the mixdown contains.

**So**: the low end (kick, bass) is solved to ~0.99 correlation, the percussion is
identified but masked, and the mid band contains something the current pad model
does not reproduce.

## 18. Per-note tuning modifier — the missing pitch (major breakthrough)

**[VALIDATED]** Every bass note that correlates >0.9 with the mixdown has
`modifier0 = 0.5` (neutral). Every note with a different `modifier0` failed to
correlate at any rate around unity. Scanning the playback rate of those notes
finds:

| `modifier0 − 0.5` | measured cents | formula `(v−0.5)·24000` |
| --- | --- | --- |
| +0.0083 | **+200.06** | +200 |
| −0.0083 | **−201.75** | −200 |

So `modifier0` (Tuning coarse) spans **±1 octave** over 0…1, with 1/240 = one
semitone. Applying this per note raises the whole-mix correlation from
**0.67 → 0.86** (left channel), with no other change.

**[VALIDATED]** Typical decoded values are exact integers: `0.5041666` = +100 c,
`0.5083333` = +200 c, `0.5125` = +300 c, `0.5208` = +500 c, `0.55` = +1200 c.

## 19. Reversed playback

**[VALIDATED]** `layersv[0].direction = 1` on pads 18 and 27. Reversing the
sample before placing it raises pad 18's correlation from 0.179 to 0.511.

## 20. Warped pads remain the open problem

**[VALIDATED]** All pads that still fail to correlate after the tuning fix are
exactly the pads with `warpEnable = true`: **10, 13, 14, 20, 27** (Vox, Guitars,
Sample 030, Chop 1-Sample 030).

Evidence that they *are* audible but time-stretched:

* onset-energy test — energy at their note onsets vs random times:
  pad 10 **1.98×**, pad 13 **2.25×**, pad 20 1.39×, pad 27 1.36×;
* in the quiet tail the mixdown's partial is 414 Hz while the Vox sample at the
  same musical position has 494 Hz — the device is playing a *different point in
  the sample*, which is what a pitch-preserving time-stretch does;
* neither a resample (rate scan 0.6…1.8) nor a phase-vocoder time-stretch at
  the tempo ratio (110/85 = 1.294) reproduces the waveform.

**[HYPOTHESIS]** MPC's warp is a proprietary granular/phase time-stretch; without
it those pads cannot be reproduced sample-exactly.

## 21. Current physical-model accuracy

**[VALIDATED]** With tuning, reversal, monophony, tail and calibrated gain:

```
whole-mix correlation   left 0.8609   right 0.8466
model rms 0.3415 / 0.3205   reference rms 0.5580 / 0.5135
```

Per pad (left channel, no gain calibration per pad): kick 0.49, bass 0.74,
pad 18 0.23, warped pads ≈ 0.

## 22. Guaranteed-exact stems

Because the master insert chain sits *after* the pad sum, the physical render can
never reproduce the file exactly. The converter therefore offers an exact mode
that works in the integer domain:

1. render the physical stems,
2. quantise each to 24 bit,
3. compute `residual = mixdown_int − Σ stems_int`,
4. distribute the residual proportionally to each stem's smoothed energy
   envelope with error diffusion, respecting 24-bit headroom.

Result on the reference project: **12 stems, 1,286,916 frames, sum equals the
device mixdown with 0 LSB difference over 2,573,832 samples.**

Residual absorbed per stem (rms relative to that stem's own rms): bass +106 %,
kick +30 %, sample-030 +76 %, chop-16 +43 %, hats +65 %, Vox +81 %. The large
shares on the warped pads are the un-reproduced warp plus their portion of the
master FX.

## 23. Loops: none in this project

**[VALIDATED]** All 28 pads have `loop: false`, `loopMode: 0`, `loopStart: 0`,
`loopEnd: 0`, `loopCrossfadeLength: 0`, `loopFineTune: 0`. `sliceInfo.LoopMode`
is 0 or −1 (inherit) and never an active loop mode. The `smpl` chunks in the
sample WAVs carry `loop count = 0`.

So the reference mixdown contains **no looping**. The converter still reads and
honours the loop fields so other projects work, but for this one they are
inactive — which is why ignoring them changed nothing.

## 24. Warp: the rate is exactly project / sample tempo

Method: pitch tracking (autocorrelation, 150–1200 Hz band) of the isolated Vox
tail vs the Vox sample, then a search over stretch rates minimising the pitch
error.

**[VALIDATED]** Best stretch rate **1.290 – 1.296**, i.e. **110 / 85 = 1.2941**,
with a median pitch error of **0.0 cents** over 40 frames. The `stretchPercentage`
field (100 for this pad) is *not* the rate.

**[VALIDATED]** The stretch preserves pitch: modelling it as a resample would
require an extra +450 cents at rate 1.294 and fails. A pitch-preserving
time-stretch fits exactly.

**[VALIDATED]** Warped pads are **gated by the note length**. Letting them ring
free put spurious energy in the last second and gave tail correlation 0.8375;
gating at the note length gives **0.9811**. Gating *all* pads is wrong (0.58), so
only warped pads are gated.

## 25. Warp algorithm: closest family identified, still approximate

**[VALIDATED]** On an isolated tail window, the stretch algorithms score
(correlation, with a lag search over ±3000 samples):

| algorithm | correlation |
| --- | --- |
| phase vocoder (STFT) | **0.64** |
| WSOLA (waveform-similarity OLA) | 0.26 |
| plain granular OLA (N=512…4096) | 0.12 – 0.22 |
| resample at the same rate | < 0.11 |

So the device uses a **phase-vocoder-like** time-stretch, not a granular or
resampling one. `mpc2stems.warp` implements a phase vocoder, verified to be
sample-aligned (an impulse at 10000 lands at 7680 for rate 1.294, expected 7728)
and pitch-preserving (440 Hz stays 441 Hz).

It is still an approximation: including the warped pads *lowers* the whole-mix
correlation slightly (0.8990 with vs 0.9038 without), i.e. their phase does not
match the device. They are ~−20 dB relative to the mix, so the effect is small.

## 26. Final accuracy

**[VALIDATED]** With tuning modifiers, reversal, monophony, note-length gating of
warped pads, tempo warp and calibrated gain:

```
whole mix correlation   left 0.8989   right 0.8880
residual rms (unclipped) 0.186
```

Progress across the investigation: 0.38 (naive) → 0.67 (clipping + reverse +
warp as resample) → 0.86 (per-note tuning) → 0.90 (warp rate, gating).

## 27. Pad filters — present on five pads

**[VALIDATED]** `synthSection.filterData.value0` is not always transparent:

| pad | filterType | cutoff | resonance | meaning |
| --- | --- | --- | --- | --- |
| 0,1,2,9,10,14,18 | 2 | 1.000 | 0.000 | open (transparent) |
| 13 | 2 | 0.331 | 0.583 | **filtered** |
| 16 | 29 | 0.535 | 0.197 | **filtered** |
| 17 | 11 | 0.457 | 0.126 | **filtered** |
| 20, 27 | 11 | 0.646 | 0.181 | **filtered** |

Type 2 with cutoff 1.0 is transparent, which is why the drum pads matched before
filters were modelled. Modelling the five filtered pads as a resonant low-pass
raises whole-mix correlation **0.8808 → 0.9085**.

The type enum (2 / 11 / 29) and the normalised-cutoff mapping are **not**
identified. The mapping used is logarithmic 20 Hz…20 kHz; the correlation is
flat between 0.15× and 0.65× of that scale, so the exact cutoff is not strongly
constrained by the reference.

**[VALIDATED]** Layer `volume.gainCoefficient` is 1.0 and `volume.controlValue`
is 1.0 on every pad, so the only level control is `instrument.mixable.volume`.

## 28. Where the remaining error lives

**[VALIDATED]** With the model at its best (tuning + reverse + warp + filters +
monophony + tempo warp), fitting a single global gain gives:

```
residual rms 0.163   reference rms 0.558        (≈ −10.6 dB)
```

Per band, the residual is 7–9 dB below the reference in the low bands and equal
to it in the mid/high bands — i.e. the model is **bass-heavy and missing mid
content**.

**[VALIDATED]** Fitting one gain per pad (least squares, unclipped samples)
raises correlation to **0.959** but drives pads 10, 13 and 16 to **zero gain**.
So those pads' rendered content does not match the reference at all — it is not
a level problem.

**[DISPROVED]** "The remaining difference is the master clipper/limiter."
Fitting `clip(g·s)`, `tanh(g·s)` and a threshold/ratio limiter over a grid never
beats plain scaling: best residual 0.1628 vs 0.1644. A static master curve cannot
explain the gap.

**[VALIDATED]** The residual correlates with the model at **+176 ms (+0.159)**,
**+644 ms (+0.159)** and **−1 bar (−0.154)** — weak periodic structure, consistent
with tempo-synced processing (Flex Beat) rather than a reverb tail.

## 29. Two output bugs fixed

**[VALIDATED]** `mix-from-stems.wav` was written from the *physical* stems while
the stem files themselves were the exact ones, so the mix file did not match the
device mixdown (residual 0.194). It is now written from the exact integer stems
and is **byte-identical** to `Song1.wav`.

**[VALIDATED]** Gating warped pads at their note length improved the tail but cut
Sample 030 to 0.7 s, which is audibly wrong. Gating is therefore **off by
default**; gating only pads 10/13/14 scores 0.9132 vs 0.9085 but shortens the
Sample 030 pad, so the default is no gating.

## 30. Final state

| quantity | value |
| --- | --- |
| physical model correlation | **0.9044 L / 0.8966 R** |
| physical residual rms | 0.252 (ref 0.558) |
| exact stems: sum vs mixdown | **0 LSB over 2,573,832 samples** |
| `mix-from-stems.wav` | byte-identical to `Song1.wav` |

Reaching 1:1 with the *dry* stems is blocked by two things that cannot be read
from the project file: MPC's warp algorithm (phase-vocoder-like, but its output
phase does not match) and the master insert chain (`Color Compressor`,
`Flex Beat Player`, `Pad-Controlled FXs`), whose plugin state is an undecoded
binary blob.

## 31. The master compressor is recovered

The device manual documents the Compressor effect (SHIFT+PAD 5):

| control | range | this project |
| --- | --- | --- |
| Attack (K1) | 0.100 – 150 ms | 90 % |
| Release (K2) | 3.0 – 300 ms | 80 % |
| Amount (K3) | 0 – 100 % | 30 % |
| In Boost (SHIFT+K3) | — | 0 |
| Color (B1) | parallel bass boost + saturation + pitch instability | — |
| Makeup | computed automatically from input level | — |

The knob positions follow the logarithmic mapping (the fit selected it over
linear for both attack and release):

```
attack  = 0.1 * (150/0.1)**0.90  =  72.2 ms
release = 3.0 * (300/3.0)**0.80  = 119.4 ms
```

**[VALIDATED]** Fitting threshold and ratio against the reference gives
**−6 dB, 2:1**, maximum gain reduction 3.4 dB. The compressor is gentle.

**[VALIDATED]** In this project only the compressor is active. The user
confirmed Flex Beat and the pad-controlled FXs were **not** used, so the master
chain is a single compressor — no other insert processing needs modelling.

**[VALIDATED]** A bus compressor is a shared gain curve, so the same curve can be
applied to every stem and they still sum to the compressed bus exactly. Applying
it raises the model correlation **0.9044/0.8966 -> 0.9219/0.9120** and drops the
residual from 0.252 to 0.217.

**[DISPROVED]** "The Color stage explains the missing top end." Parallel bass
boost and parallel saturation were both fitted; each gains only ~0.005 residual
(0.1797 -> 0.1723). The Color stage is not the gap.

## 32. Time-stretch is iZotope Radius

**[VALIDATED]** iZotope licensed its time-stretching and loop-slicing technology
to Akai for the MPC line (2012 press release). iZotope's stretcher is **Radius**,
which is proprietary and not published. This explains why no open algorithm
matches: a phase vocoder reproduces the *rate* (validated to 0.0 cents) but not
the *phase*, and WSOLA / granular / resampling score far worse (0.26 / 0.12-0.22
/ <0.11 vs 0.64 for a phase vocoder on an isolated window).

Reproducing Radius bit-exactly would require the library itself.

## 33. What still does not match

**[VALIDATED]** After the compressor the residual is 0.217 (reference 0.536).
Its composition:

* warped pads (10, 13, 14, 20, 27) contribute ~0.02 — removing them entirely
  moves the residual 0.180 -> 0.160 in the pre-compressor test;
* the rest is concentrated above 2.5 kHz: the model's 2.5–9 kHz energy is
  0.026 vs 0.075 in the reference, and a per-pad gain fit **in that band only**
  reaches just 0.196 correlation — no combination of the pad stems reproduces it;
* harmonic distortion of the low band does **not** explain it (correlation
  < 0.005 for `x²`, `x³`, `|x|`, `tanh` and clip products against the reference HF);
* no global linear filter is present: a least-squares FIR from model to reference
  is essentially a delta (h[0] ≈ 1.01, energy ≈ 1.03) and only improves the
  residual from 0.175 to 0.168 at 128 taps.

So the remaining error is content the project file does not describe, not a
level, EQ, or dynamics error.

## 34. Final state

| quantity | value |
| --- | --- |
| model correlation (with compressor) | **0.9219 L / 0.9120 R** |
| model residual rms | 0.217 (reference 0.536) |
| exact stems vs mixdown | **0 LSB over 2,573,832 samples** |
| `mix-from-stems.wav` | byte-identical to `Song1.wav` |

## 35. What the MPC Sample manual adds

Reading the official MPC Sample guide filled in several parameter ranges and
confirmed or corrected earlier guesses:

| parameter | documented range | found in this project |
| --- | --- | --- |
| Volume (per sample) | −INF, −74.00 … +6.00 dB | `mixable.volume` 0.27 … 1.0 |
| Pan | 50L … C … 50R | `mixable.pan` 0.49 … 0.503 |
| **Vel Sens** | 0 – 127 | `velocitySensitivity` **1.0 on every pad** |
| Attack | 0 – 127 | 0 on most pads, 2/127 on 17, 20, 27 |
| Decay | 0 – 127 (One Shot) | 6/127 with `DecayFromEnd` on 7 pads, else 127 |
| Semi/Fine Tune | ±24 semitones / ±90 cents | `layer.pitch` −9.0 … +4.8 |
| Warp | Off, 50 – 200 %, **Seq** | `warpEnable` + `bpmLock` + `stretchPercentage` |
| **# Beats** | length of the sample in beats | not present in the file |
| Polyphony | Mono / Poly | `monophony` = Mono on every pad |
| Offset | 0 – 100 % trigger delay | `layer.offset` = 0 |
| Filter Types | Off, Classic, LPF2/4, HPF2/4, BPF2/4 | `filterType` **2, 11, 29** |
| Chop | Threshold / Regions 4-8-16 / Manual | `chopMode` 0 or 2, `chopRegions` 4 or 16 |

**[VALIDATED]** Velocity sensitivity is 1.0 on every pad, so a single global
velocity law is correct — there is no per-pad velocity curve to model.

**[VALIDATED]** The amp envelope is mostly flat. Fitting the attack/decay time
scaling moves the residual by <0.001 (0.9221 → 0.9225), because only four pads
have a non-zero attack and it is 2/127.

**[VALIDATED]** `chopMode` / `chopRegions` do **not** truncate playback: every pad
in the song has `sliceIndex = 129` and `sliceInfo` spanning the whole sample, so
the full sample plays.

## 36. The submix LP Filter is enabled and unmodelled

**[VALIDATED]** Pads route to **Submix 1** (`audioRoute.destination = 1`), and
Submix 1's insert chain has an **LP Filter with `enable: true`**. The signal
chain is therefore:

```
pads -> Submix 1 (LP Filter) -> Outputs 1/2 (Color Compressor) -> master
```

Its cutoff and resonance live in the undecoded plugin `state` blob. Fitting it
as a plain low-pass suggests ~500 Hz, which is implausible for a mix bus and
mostly acts as a crude level trim; it improves the residual only
0.2086 → 0.2019, so it is **left unmodelled** rather than guessed.

## 37. Final state

| quantity | value |
| --- | --- |
| model correlation | **0.9262 L / 0.9171 R** |
| model residual rms | **0.209** (reference 0.536) |
| exact stems vs mixdown | **0 LSB over 2,573,832 samples** |
| `mix-from-stems.wav` | byte-identical to `Song1.wav` |

Progression of the dry model: 0.38 (naive) → 0.67 → 0.86 (per-note tuning) →
0.90 (warp rate + gating) → **0.93** (compressor + filter mapping).

The remaining 3–8 kHz deficit (model 0.023 vs reference 0.068 rms) is not
attributable to any pad at any gain, and the residual there is *anti*-correlated
with the model, so it is content the project file does not describe. Closing it
needs either the Radius stretcher or the undecoded plugin states.

## 38. Pad filter types: tested exhaustively, not the source of the residual

The MPC Sample manual lists eight filter types (Off, Classic, LPF2, LPF4, HPF2,
HPF4, BPF2, BPF4) but the project stores **2, 11 and 29**, so the internal enum
is larger than the manual's list. Both were resolved empirically.

> **Superseded by §69.** The enum was later recovered directly from the firmware:
> LPF2=2, LPF4=3, HPF2=7, HPF4=8, BPF2=**11**, BPF4=12, Classic=**29**. The
> empirical assignments in this section were partly wrong (11 is BPF2, not BPF4;
> 29 is Classic, not a plain LP; 2 is LPF2, not Classic).

**Global sweep.** Each of the eight responses was assigned to the stored values
11 and 29 (with 2 fixed to a 2-pole low-pass) and rendered end to end:

| type 11 | type 29 | residual | correlation |
| --- | --- | --- | --- |
| bp4 | lp4 | **0.2079** | 0.9268 |
| bp4 | lp2 | 0.2081 | 0.9267 |
| lp2 | lp4 | 0.2084 | 0.9265 |
| lp2 | lp2 | 0.2086 | 0.9264 |
| hp4 | lp4 | 0.2091 | 0.9260 |

The whole spread is **0.002** — filter identity is within noise. **[DISPROVED]**
"The residual is in the pad filters."

**Joint per-pad fit.** A coordinate descent over {8 types × 13 cutoffs × 5 Q}
for all five filtered pads together reaches **0.17483** from 0.22413. But the
settings it chooses are extreme and not plausible filters:

```
pad 13  bpf4    60 Hz  Q 1.0     pad 20  classic  60 Hz  Q 4.0
pad 16  lpf4   386 Hz  Q 1.0     pad 27  bpf2    242 Hz  Q 4.0
pad 17  bpf2   242 Hz  Q 0.5
```

A 60 Hz band-pass is a mute. The optimiser is not finding a filter — it is
deleting the pads, because their rendered content does not resemble the
reference. That is consistent with §28 and §33.

**[VALIDATED]** Stereo is *not* the explanation either: several samples are
genuinely decorrelated (hi-hat L/R correlation 0.081, Vox −0.295) and the
renderer preserves them (stem L/R correlations match the source), so the
channels were never collapsed.

## 39. Proof that the remaining residual is missing content, not mixing

**[VALIDATED]** Decisive test. Build a basis from the rendered stems and ask
whether *any* linear combination of them can reproduce the reference:

| basis | residual |
| --- | --- |
| 12 pad stems, one gain each | 0.1513 |
| 12 pads x 3 bands = 36 signals, one gain each | **0.1486** |
| simplest model (tuned, compressor, causal filters) | 0.2076 |

Even with a free per-pad, per-band gain on every stem the floor is **0.149**.
Adding 24 more degrees of freedom buys 0.003. So no gain, EQ, balance or pan
setting can close the gap — the reference contains signal that the rendered
stems do not contain.

**[VALIDATED]** Per-band correlation against the reference at zero lag:

| band | correlation |
| --- | --- |
| 20–200 Hz | **0.965** |
| 200–1 kHz | 0.426 |
| 3–8 kHz | 0.157 |
| 6–14 kHz | 0.633 |

**[VALIDATED]** No time offset: the best lag in every band is 0 samples, so this
is not a group-delay or resampler-alignment error.

**[VALIDATED]** Causal vs zero-phase pad filters: causal is slightly better
(0.2086 → 0.2076) and is physically correct, so it is now the default. It does
not close the gap.

## 40. Conclusion

The model residual cannot reach zero from this project file plus this mixdown.
Four independent measurements agree:

1. per-pad least-squares gains drive the questionable pads to zero;
2. the joint filter fit only improves by *muting* pads (60 Hz band-pass);
3. the 3–8 kHz band is 3x too quiet and uncorrelated, and no pad at any gain
   reproduces it;
4. the full per-pad, per-band linear basis bottoms out at 0.149.

The two candidates that remain — iZotope Radius (proprietary, licensed by Akai)
and the undecoded plugin `state` blobs (submix LP Filter, compressor) — cannot
be recovered from the files present.

## 41. Complete parameter audit

Every field of the note events, sequence, clip and instrument was enumerated and
classified. This is the full list of what exists and what is used.

**Note events** (`eventList.events`): `time`, `type`, `channel`, `muted`,
`selected`, `invented`, and `note{note, velocity, length, probability, ratchet,
articulation, modifierValue0-15, modifierActiveState0-15}`.

| field | observed | used |
| --- | --- | --- |
| `muted` | `false` on all 308 notes | no effect |
| `probability` | 100 on every note | no effect |
| `ratchet` | 1 on every note | no effect |
| `velocity` | 0.11 – 1.0 | yes |
| `length` | 15 – 1231 pulses | only for gate mode |
| `modifierActiveState` | only index 0 (tuning) and 15 (slice) | tuning yes, slice no |

**Sequence / clip**: `bpm` 110, `transposition` 0, `swing` 0, quantisation
`strength` 0.0 (so no timing shift), `perClipParameterValues` two entries both
value 0.0 with parameter id 256. All inert.

**Program**: `transpose` 0, `midiKillGroup` −1, `padNoteMap.noteForPad` =
36, 37, 38 … (confirms note = 36 + pad), `programPads.pads` all 0,
`monophonic` false, `monoRetrigger` false, track `mixable.volume` 0.708,
`pan` 0.504, `sends` all 0, insert chain **empty**.

**Instrument — fields that vary between the pads in the song** (the ones that
must be modelled):

| field | modelled |
| --- | --- |
| `mixable.volume`, `mixable.pan` | yes |
| `layer.pitch` / `coarseTune` / `fineTune` | yes |
| `layer.direction` | yes |
| `warpEnable`, `stretchPercentage`, `tempo`, `bpmLock` | yes |
| `filterCutoff`, `filterResonance`, `filterType` | yes |
| `ampEnvelope.Attack`, `Decay`, `DecayFromEnd` | yes |
| `whichMuteGroup` | **added — see §42** |
| `chopMode`, `chopRegions`, `noteCounters` | verified inert |
| `userSelectableWarpPoolIndex` (8 or 13) | selects a warp algorithm — not usable |

**Instrument — constant or inert**: `velocitySensitivity` = 1.0 on all pads,
`filterEnvelopeAmount` 0, `filterKeytrack` 0, `filterVelocity` 0,
`velocityToStart` 0, `velocityToPitch` 0, `velocityToPan` 0,
`lfoData` all destinations 0, `modLinks` source/target 0,
`articulations` type 0, `padEffects` all `fxValue` 0.0.

## 42. Mute groups — a real omission, now modelled

**[VALIDATED]** `whichMuteGroup` is non-zero on four pads in the kit:

| group | pads |
| --- | --- |
| 1 | 2, 3 |
| 3 | 8, 9 |
| 5 | **12, 13, 14** |

The manual: *"When pads are assigned to the same mute group, the last pad played
silences any other playing samples in the same group."*

Pads **13 and 14 both play in the song and share group 5**, so they cut each
other — and every new pad-13 note kills pad 14's long sample. The renderer now
truncates a voice at the next note in its mute group. Pad 14's stem drops to 27 %
of its previous energy, and the model improves:

```
residual 0.2076 -> 0.2061      correlation 0.9270 -> 0.9282
```

**[VALIDATED]** Routing: pads -> `audioRoute.destination 1` (Submix 1) ->
destination 2 (Outputs 1/2). Submix 1's LP Filter is `enable: true` in the file,
but the fit prefers an implausible 500 Hz and the user confirms no global filter
was used, so it is treated as bypassed.

## 43. Pad filter types resolved: type 11 is a BAND-PASS

The user recalled using a **band-pass** on the Sample 030 pad, which is the
`filterType = 11` group (pads 17, 20, 27). That explains the symptom that
started this: the pad sounded far too quiet and dull compared with the device.

Two things had been wrong:

1. **Type mapping.** Type 11 was modelled as a low-pass. As a 4-pole band-pass
   it is both the user's recollection and the better spectral fit.
2. **Cutoff mapping.** The normalised cutoff was mapped over 20 Hz – 20 kHz,
   which put pad 20 at 1734 Hz — far too dark. The band-energy fit prefers a
   scale factor of **2.0**, i.e. roughly 40 Hz – 40 kHz of headroom, which is
   more plausible for a sampler.

Final mapping, calibrated on band energy rather than waveform correlation:

| stored | response | example cutoff |
| --- | --- | --- |
| 2 | 2-pole low-pass | pad 13 at 395 Hz |
| 11 | **4-pole band-pass** | pad 20 at 3.5 kHz |
| 29 | 4-pole low-pass | pad 16 at 1.6 kHz |

with `Q = 0.5 + resonance × 2.0`.

**[VALIDATED]** Band energies now track the reference closely:

| band | reference | model | error |
| --- | --- | --- | --- |
| 3–8 kHz | 0.0670 | 0.0730 | +0.7 dB |
| 1.2–3 kHz | 0.0354 | 0.0455 | +2.2 dB |
| 200–1200 Hz | 0.1127 | 0.1198 | +0.5 dB |

Pad 20's stem goes from rms 0.0087 to **0.0746**, with 3–8 kHz content 0.0653
against the reference's 0.0670 — i.e. it is now audible and as bright as the
record, which is what the ear was reporting all along.

## 44. Why waveform correlation was the wrong objective

**[VALIDATED]** Correlation rewards *removing* content: any pad whose rendered
phase does not match can only lower the score, so every fit that maximised
correlation drove the doubtful pads to zero gain or to a 60 Hz band-pass (a
mute). Band energy is phase-blind and therefore the better discriminator for
timbre.

The two objectives disagree on this project:

| objective | configuration | result |
| --- | --- | --- |
| waveform correlation | heavy LP (scale 0.6) | corr 0.928, but 3–8 kHz 9 dB too quiet, pad 20 inaudible |
| band energy | BPF type 11 (scale 2.0) | corr 0.910, spectral error < 2 dB, pad 20 audible |

The delivered model uses the second: **correlation 0.9102 L / 0.8978 R, residual
0.2344**, with the stems spectrally correct and every pad audible.

## 45. Sample Slice modifier — the pad was playing chopped quarters

The user reported that the Sample 030 stem retriggered far too often: in the
first sequence the long sample should play roughly once, but the render
restarted its opening four times.

**[VALIDATED]** The four note events for that pad (pulses 0, 3840, 7680, 11520)
carry **modifier 15 (Sample Slice)** with values

```
0.0, 0.007874, 0.015748, 0.023622   =   0, 1, 2, 3   out of 127
```

So they are **slices 0, 1, 2 and 3**, not four triggers of slice 0. The sample is
in Chop Mode (`chopRegions: 4`) and the `slic` chunk splits it into four equal
quarters of 91 973 frames. Played in order across the four bars they reproduce
one continuous pass through the sample — exactly what the ear hears.

Effect of honouring the modifier:

| | bar 1 vs bar 2 correlation | residual | correlation |
| --- | --- | --- | --- |
| slice ignored (retrigger) | 0.859 (identical restarts) | 0.2326 | 0.9104 |
| **slice honoured** | **0.022** (continuous) | **0.2246** | **0.9160** |

**[VALIDATED]** `slic` chunk format: `u16 version, u16 count`, then 32-bit slice
points interleaved with small bookkeeping values. Offsets in range that increase
strictly give the boundaries; the 16-slice variant decodes to exactly 17
boundaries and the 4-slice variant to 5 (one spurious value under 64 is
discarded). `mpc2stems.wav.read_slices` implements this.

**[VALIDATED]** Only this pad uses the modifier: 4 notes in sequence 3, none in
sequence 4. Modifier 0 (tuning) remains the other active modifier.

## 46. State after the slice fix

| quantity | value |
| --- | --- |
| model correlation | **0.9159 L / 0.9050 R** |
| model residual | 0.2246 |
| band energies (3–8k / 1.2–3k / 200–1.2k) | 0.073 / 0.045 / 0.120 vs 0.067 / 0.035 / 0.113 |
| exact stems vs mixdown | **0 LSB over 2,573,832 samples** |

## 47. Bug hunt after the slice mistake

A systematic sweep for further "modifier-class" errors: anything that changes
*which part* of a sample plays, *how* it plays, or *when*.

**Modifiers.** Across all five sequences the only active modifiers are:

| seq | modifier 0 (tuning) | modifier 15 (slice) |
| --- | --- | --- |
| 0 | 138 notes, all value 0.5 | — |
| 1, 2 | 16 notes, pad 13 | — |
| 3 | 31 notes, pads 9 and 13 | 4 notes, pad 20 |
| 4 | 63 notes, pads 9, 13, 27 | — |

Both are handled. No other modifier (pan, level, filter, envelope, layer) is
active anywhere — so there is no second slicing/tuning class of bug.

**[VALIDATED]** Layers: no pad in the kit has a sample on any layer other than 0,
`velocityStart/End` are 0/127 everywhere, `sampleStart/End` are 0,
`offset` is 0, `loop` is false, `layer.volume` is 1.0 and `pan` 0.5.

**[VALIDATED]** Filter slot 1 (`filterData.value1`) is `filterType 0` on every
used pad, so the second filter stage is inactive.

**[VALIDATED]** Zone trims are correct: extending any pad's `zone_end` to the
whole file makes the residual much worse (pad 18: 0.2246 → 0.3089), so
`sliceInfo.End` is being applied as intended.

**[VALIDATED]** `slic` chunk: the 16-slice file was being read as 17 slices
because the last slice point is `frames - 1` and the reader appended the file
end. Fixed to only extend on a gap larger than 64 samples.

**[VALIDATED]** Warp rate rule: tempo ratio wins over the alternatives.

```
tempo (project/sample_tempo) 0.2203   mixed (bpmLock-aware) 0.2297
stretchPercentage            0.2321   no warp               0.2453
```

**[VALIDATED]** No per-pad timing offset: cross-correlating every stem against
`reference - other pads` peaks at lag 0 for all pads that carry signal.

## 48. Which pads the model actually explains

Leave-one-out: the residual change when a pad's stem is removed.

| pad | Δ residual | reading |
| --- | --- | --- |
| 9 (bass) | +0.249 | essential, well modelled |
| 0 (kick) | +0.113 | essential, well modelled |
| 18 (chop) | +0.047 | contributes correctly |
| 16 | +0.001 | neutral |
| 27, 17, 2, 1, 14 | −0.000 … −0.002 | neutral |
| 20 | −0.004 | slightly harmful |
| 10 (vox) | −0.007 | harmful |
| **13 (guit)** | **−0.014** | most harmful |

Matched-filter presence test against the reference confirms every warped pad is
genuinely in the mix, with the warp applied:

| pad | best match | at |
| --- | --- | --- |
| 18 (unwarped) | **0.972** | 8.18 s |
| 10 | 0.545 | 25.93 s — exactly its final note onset |
| 13 | 0.515 | 3.93 s |
| 14 | 0.248 | 9.59 s |
| 27 | 0.230 | 28.82 s |

Pad 10 matching at 0.545 *at its own note onset* independently confirms the
110/85 = 1.294 warp rate found earlier by pitch tracking.

The warped pads are therefore present but not sample-aligned — consistent with
iZotope Radius producing a different waveform from any open stretcher.

## 49. State after the hunt

| quantity | value |
| --- | --- |
| model correlation | **0.9190 L / 0.9078 R** |
| model residual | **0.2203** |
| exact stems vs mixdown | 0 LSB over 2,573,832 samples |
| compressor | −3 dB, 3:1 (re-fitted against the corrected model) |

## 50. Firmware analysis — the stretch is zplane élastique Pro V3

The MPC Sample 1.3.0 updater was unpacked and analysed.

**Container.** `Contents/Resources/Update.img` is an `AZ0x` archive: a 0x60-byte
header naming the snapshot (`SNAPSHOT-20260402131256`), product `AC50` and layout
`recovery / boot / rootfs`, followed by 64-byte partition records
(`tag u32, version u32, start u64, size u64, count u64` + SHA-256). Three
partitions, all **XZ**-compressed:

| partition | start | compressed | payload |
| --- | --- | --- | --- |
| PARR (recovery) | 0x1ac880 | 12.4 MB | |
| PART (boot) | 0xd87718 | 12.6 MB | |
| PART (rootfs) | 0x19967d8 | 142.4 MB | **ext4**, 427 MB, 5 821 files |

**Application.** `usr/bin/MPC Sample` — 114 MB, stripped ARM64 PIE, build id
`1dac7f48…`. Web UI at `var/www/ac50/` (React), `version.json` reports
`1.3.0.12-release-g40e46ca7991`, matching the project container version exactly.

**[VALIDATED]** The time-stretch is **zplane élastique Pro V3**, not iZotope
Radius. Evidence:

* the licence text embedded in the binary: *"This product incorporates
  elastique Pro V3 by zplane.development."*
* C++ RTTI/class names: `CElastiqueProV3`, `CElastiqueProV3Direct`,
  `CElastiqueV3Mobile`, `CElastiqueCoreBase`, `CelastiqueSOLOV3`,
  `PSOLASynthForElastiqueProV3`, `Flatten Clip Elastique`.

**Effect framework.** Plugins are "H3" parts with typed parameters — evidenced
by `wzParameterType*` names (94 distinct, including `CompressorRatio`,
`CompAttack`, `CompRelease`, `FilterLP_withOff`) and an embedded H3 patch XML
(`<H3Part Type='Dyn3Compressor' …>`).

**Filter enum.** The Sample-Mode UI string table lists the types in order
`Classic, LPF2, LPF4, HPF2, HPF4, BPF2, BPF4` (each an 8-byte fixed string). With
`Off` as index 0 this makes the stored value **2 = LPF2**, consistent with the
drum pads being transparent at cutoff 1.0. The project uses values **11** and
**29** for the filtered pads, which lie outside this 8-entry enumeration, so the
serialised `filterType` is an internal index that this table does not cover.

## 51. Why the model residual cannot reach zero

**[VALIDATED]** Decisive experiment. For pad 10 in its isolated tail (the last
Vox note, where nothing else masks it):

| measure | value |
| --- | --- |
| magnitude-spectrogram correlation (nperseg 2048) | **+0.8927** |
| waveform correlation, same window | **−0.0202** |

The stretched pad therefore has the *right content, at the right time, at the
right level and with the right spectral shape* — and the *wrong phase*. That is
precisely what a proprietary stretcher produces: the magnitude STFT is
determined by the source and the rate, while the phase trajectory is generated by
the algorithm's own transient/phase model.

Since élastique Pro V3 is a commercial library, statically linked into an ARM64
binary with no source and no host-runnable form, its phase cannot be reproduced.
A phase-locked vocoder was implemented and measured as *worse* (0.2248 vs 0.4816
on the same tail), so it is not a phase-locking variant either.

**Therefore the dry model has a hard floor.** The sum can still be made exact —
that is what `--exact` does — but the per-pad waveforms of the five warped pads
cannot be modelled bit-exactly from the project file.

## 52. Vocoder analysis parameters

**[VALIDATED]** With the stretcher identified, its analysis window was tuned
against pad 10's isolated tail (correlation as a function of n_fft/hop):

| n_fft | hop | tail corr | global residual | global corr |
| --- | --- | --- | --- | --- |
| 2048 | 512 | 0.559 | 0.2203 | 0.9192 |
| **512** | **64** | 0.619 | **0.2170** | **0.9219** |
| 640 | 32 | 0.681 | 0.2217 | 0.9202 |
| 1024 | 256 | 0.594 | 0.2189 | 0.9205 |

`n_fft 512 / hop 64` is now the default (the best isolated-tail setting, 640/32,
is worse globally because it is chosen on one pad only).

Identity phase locking was implemented and measured **worse** on the same tail
(0.2248 vs 0.4816 at the time), so élastique is not a phase-locking variant
either.

## 53. Final state

| quantity | value |
| --- | --- |
| model correlation | **0.9217 L / 0.9103 R** |
| model residual | **0.2170** |
| stems vs mixdown | **0 LSB over 2,573,832 samples** |
| `mix-from-stems.wav` | byte-identical to `Song1.wav` |

The residual floor is now explained from first principles: 0.89 magnitude
correlation with −0.02 waveform correlation on an isolated warped pad means the
remaining error is entirely the phase trajectory of zplane élastique Pro V3.

## 54. Firmware: what the export path looks like

The application retains C++ names in `.rodata` even though `.symtab` is stripped,
which gives the class vocabulary directly:

```
MpcInterfaceBehaviourR...R16MixdownGenerator...
MixdownGenerator::Export(MixdownProperties const&, int, function<void()>)
MixdownGenerator::ExportSong(MixdownProperties const&, SequenceChain&, function<void()>)
MixdownGenerator::ExportKitArrangement(MixdownProperties const&, int, function<void()>)
MixdownProperties::RenderSourceType
MixdownStatus
```

`MixdownGenerator::ExportSong` is the song-mode mixdown entry point. Strings
found: `Export Song to audio mixdown or new sequence?`, `Exporting song`,
`Exporting arrangement`, `Exporting track`, `Exporting program : `. The one code
reference to `Exporting song` resolves to **0x18bcb00** (via an ADRP+ADD scan of
`.text`), inside a very large function that also pulls `MixdownStatus` and the
progress callbacks.

**[VALIDATED]** `"emulation": 0` in the project, so the Vintage Emulation
(`VintageEmulationEffects` / `VintageEmulationInsert` / `EmulationState`) is
**off** and does not colour the mixdown.

**[VALIDATED]** `MixdownProperties::RenderSourceType` exists as a property, i.e.
the exporter has an explicit render-source selection. Without the enum values
recovered, the FX-inclusive path is assumed, matching the user's report.

**Limit of this pass.** `.text` is 61 MB (~15 M ARM64 instructions) and the
binary is stripped. Full decompilation of `ExportSong` and its callees is a
multi-session effort; this pass established the entry points, the plugin
framework and the DSP provenance, not a line-by-line reconstruction of the
mixdown loop.

## 55. Where the residual actually lives

**[VALIDATED]** Band correlation with the complete model:

| band | corr | model rms | reference rms |
| --- | --- | --- | --- |
| 20–200 Hz | **+0.969** | 0.5186 | 0.5354 |
| 200–1200 Hz | **+0.377** | 0.1172 | 0.1127 |
| 1200–3000 Hz | +0.472 | 0.0447 | 0.0354 |
| 3–8 kHz | +0.282 | 0.0668 | 0.0670 |
| 8–16 kHz | +0.619 | 0.0231 | 0.0200 |

Every band has approximately the right *energy* — so this is not a level or EQ
error anywhere. It is a content/phase error.

**[VALIDATED]** The user's objection to the "it is only élastique" explanation is
correct. Removing the warped pads one at a time:

| subset | 200–1200 | 3–8 kHz | residual |
| --- | --- | --- | --- |
| all pads | +0.377 | +0.282 | 0.2184 |
| no pad 13 | +0.491 | +0.281 | 0.2043 |
| no 13, 10 | +0.539 | +0.282 | 0.2005 |
| no warped at all | +0.539 | +0.159 | 0.1945 |

Two warped pads account for most of the mid-band loss (0.377 → 0.539), but the
residual only falls to 0.1945 — **0.19 of the 0.22 is not the stretcher.** The
non-warped pads still leave a mid-band correlation of 0.539.

**[VALIDATED]** Even so, the warped pads are not removable: without them 3–8 kHz
collapses from 0.282 to 0.159. They supply the top end that nothing else does.

## 56. The linear floor and what calibration buys

A free per-pad gain fit against the reference (12 scalars) reaches

```
residual 0.2170 -> 0.1963     correlation 0.9219 -> 0.9390
```

with fitted gains kick 1.11, bass 1.10, pad 18 0.88, pads 1/2/16 ~0.6,
pads 17/20 ~0.25–0.35, pads 10/13/27 ~0.

Extending the basis to 12 pads × 3 bands (36 free gains) only reaches **0.1486**.
That is the hard linear floor: no gain, EQ or pan of these stems explains the
reference below it, which means the remaining error is in the *content* of the
rendered stems, not in how they are combined.

`mpc2stems convert --calibrate` applies the 12-gain fit (clamped to [0.2, 4] so
no stem is silenced).

## 57. Parameter audit against the user's hypothesis (amp envelope / pad filters)

Every candidate was tested one at a time, and each was judged with **band
energy** as well as residual, because minimizing the residual alone rewards
removing uncorrelated content (see §44).

**[DISPROVED] Amp envelope.** Attack and decay scales were swept over their full
plausible range:

| attack_scale_s | 0.0 | 0.6 | 1.2 | 2.0 | 5.0 |
| --- | --- | --- | --- | --- | --- |
| residual | 0.2175 | 0.2169 | 0.2172 | 0.2170 | 0.2166 |

| decay_scale_s | 0.0 | 0.15 | 0.3 | 0.5 | 1.0 | 2.5 |
| --- | --- | --- | --- | --- | --- | --- |
| residual | 0.2172 | 0.2172 | 0.2175 | 0.2178 | 0.2186 | 0.2234 |

The pads carry `Attack 0` and either `Decay 1.0` (no decay) or `Decay 6/127`
with `DecayFromEnd`, so the envelope is effectively flat. It is not the cause.

**[DISPROVED] Filter placed after the pitch shift.** Filtering before the
resample scores 0.2172; filtering at fixed Hz after it scores 0.2237.

**[PROVED] Filter cutoff scale 2.0 is correct — the residual alone would have
led to a wrong answer.** Minimizing the residual pushes the scale toward zero:

| cutoff scale | 0.30 | 0.50 | 1.00 | 1.50 | 2.00 | 3.00 |
| --- | --- | --- | --- | --- | --- | --- |
| residual | 0.1986 | 0.2022 | 0.2241 | 0.2140 | 0.2172 | 0.2283 |

but band energies expose that as muting — 3–8 kHz collapses to 0.024 against a
reference 0.067:

| scale | 200–1200 | 1200–3000 | 3–8 kHz |
| --- | --- | --- | --- |
| reference | 0.1127 | 0.0354 | **0.0670** |
| 0.30 | 0.0854 | 0.0203 | 0.0236 |
| 1.00 | 0.1298 | 0.0229 | 0.0287 |
| **2.00** | 0.1172 | 0.0448 | **0.0668** |

Mean band-energy error is minimized at **scale 2.0 (0.91 dB)**. Kept.

**[VALIDATED] Filter types.** Pads 16 and 17 are not warped, so their filters
can be identified from audio. Best fits: pad 16 (type 29) → **low-pass**, pad 17
(type 11) → **band-pass**, confirming the earlier empirical mapping
(2 = LP2, 11 = BP, 29 = LP). The fits are shallow (0.16 / 0.07) because the pads
are quiet, but both pick the same shapes as the band-energy analysis.

**[VALIDATED] Volume law.** `gain = volume^p`: p=1.0 gives 0.67 dB band error and
0.2172 residual; p=1.1 gives 0.62 dB and 0.2147. Linear is correct within
measurement noise.

**[DISPROVED] Note-length gating of the warped pads.** Gating pads 10/13/14/27
lowers the residual to 0.2064 but pushes the 200–1200 band from 0.1172 to 0.1019
(reference 0.1127) and the band-energy error from 0.78 to 0.84 dB — mild muting.
Left as an option, not the default.

**[DISPROVED] Per-note tuning.** The bass tuning modifier was re-fitted note by
note over ±1200 cents: values of +200, −200 and +500 all optimise at exactly
**0 extra cents** (correlation 0.96–0.98), so `cents = (v − 0.5) · 24000` is right.

**[DISPROVED] Hidden sources.** No metronome/click settings exist in the project;
`"emulation": 0` disables vintage emulation; matched-filtering all 28 sample files
against the model residual produces no unused pad above 0.20.

## 58. Firmware validation of the project parser

The `.xpj` deserializer was located in the stripped binary by resolving
`ADRP+ADD` pairs against `.rodata`, then decompiled.

**[VALIDATED] The deserializer stores raw values with no transformation.**
`filterData` is read in this exact field order (recovered from the code at
0x1476240, each name loaded into the parser context then passed to a typed
reader):

```
filterKeytrack  -> int/float
filterType      -> int     (0x146b8c0, integer reader)
filterCutoff    -> float   (0x1457fc0, number reader)
filterResonance -> float
filterEnvelopeAmount, afterTouchToFilter, filterVelocity, ...
```

The number reader at `0x1457fc0` inspects the JSON token and converts
int/uint/double to a single float (`scvtf`/`ucvtf` then `fcvt s0, d0`) and
stores it directly. **There is no scaling, normalisation or unit conversion** —
so `filterCutoff: 0.3307` in the file reaches the DSP as 0.3307. The
normalised-float model used by `mpc2stems` is therefore correct, and any
0..1 → Hz curve lives in the DSP, not in the parser.

**[VALIDATED] Field coverage.** Every field name the model consumes exists in
the binary's serializer vocabulary (`filterCutoff`, `filterType`,
`filterResonance`, `filterEnvelopeAmount`, `filterKeytrack`, `filterVelocity`,
`filterBlend`, `filterSerialRouting`, `ampEnvelope`, `sliceInfo`,
`stretchPercentage`, `whichMuteGroup`, …).

**[VALIDATED] The filter DSP is a standard bilinear biquad.** A helper at
0xfcc950 computes

```
mov w0, 0xfdb ; movk w0, 0x4049   -> 0x40490FDB = pi
fcvt s0, d0                        ; fs
fmul s1, s1, s2                    ; fc * pi
fdiv s0, s1, s0                    ; (fc*pi)/fs
bl tanf
```

i.e. `g = tan(pi * fc / fs)`, the textbook RBJ/bilinear coefficient, which is
what `mpc2stems.filters.biquad` implements.

**[VALIDATED] Fields that are present but inert in this project.**

| field | observed | conclusion |
| --- | --- | --- |
| `filterEnvelopeAmount` | **0.0** on every pad | filter envelope inert |
| `filterVelocity`, `filterKeytrack`, `pressureToFilter`, `cutoffRandom` | 0.0 | no cutoff modulation |
| `filterData.value1.filterType` | 0 on all 128 pads | second filter stage off |
| `layersv.volume.law` | 0 or 1, but `controlValue` = 1.0 everywhere | law cannot change unity |
| `program.transpose` | 0 | no global transpose |
| `program.mixable.volume`, submix and output faders | 0.7079458 (= −3 dB) each | pure scalar, absorbed by output gain |
| `mixable.sends` | all 0 | no sends |

## 59. The filterType enum is not the UI list

**[VALIDATED]** Across all 128 pads only three distinct values occur:

```
filterType 2  -> 117 pads   (the default; cutoff 1.0 = transparent)
filterType 11 ->   6 pads
filterType 29 ->   5 pads
```

The only filter-name table in the binary is the Sample-Mode UI list,
`Classic, LPF2, LPF4, HPF2, HPF4, BPF2, BPF4`, stored as 8-byte fixed strings at
0x45a2828. An 8-entry list cannot produce the values 11 and 29, so
**`filterType` is an internal DSP enum, not an index into that list.**

> **Superseded by §69 — this inference was wrong.** The firmware binds
> **2 = LPF2**, **11 = BPF2** and **29 = Classic** directly in the filter combo
> code. The reasoning below (2 = Classic) is kept as the record of a disproved
> hypothesis, not as current fact.

Since value 2 is the default on 117 of 128 pads and MPC's default drum filter is
*Classic*, the most likely reading was **2 = Classic**, a modelled analogue filter
rather than a textbook LPF2. That distinction only matters for one pad in this
project — **pad 13**, the only pad using type 2 with a closed cutoff
(0.3307). Every other type-2 pad sits at cutoff 1.0, where any low-pass is
transparent. Pad 13 is also the single worst-matching stem in the mix
(−0.014 leave-one-out, coherence −0.02 in the mid band), which is consistent
with its filter being a modelled analogue response rather than a plain biquad.

**[LIMIT]** The internal enum's values for 11 and 29 could not be resolved from
the binary: no name table exists for them, and the coefficient function is
reached through a virtual call, so no direct call site exposes the model switch.
The working mapping (11 = band-pass, 29 = low-pass) comes from the audio: pad 17
(type 11) fits a band-pass best and pad 16 (type 29) fits a low-pass best, and it
is what the band-energy calibration independently supports.

## 60. Complete format reference

`docs/FORMAT.md` has been rewritten as a complete, field-by-field specification
of the `.xpj` format, validated against both the project and the firmware:

* container and gzip header
* the four serialisation conventions (`{value0}`, key/value maps,
  `EnumCerealisationWrapper(...)`, runtime-built `modifierValue<N>` arrays)
* all 66 top-level `data` keys
* the full instrument (pad) key list, the 8-layer key list, the note-event key
  list and the 16 modifier slots
* envelopes, the two filter slots, warp, chop + the RIFF `slic` chunk
* clips, songs, mixer/routing/inserts, the sample pool
* a single table of every value encoding in one place

Facts re-verified while writing it: 128 instruments, 8 layers each, 28 pads
carrying a sample (none above layer 0), `velocityStart = 0` on all of them,
32 `modLinks`, 16 `modifierPlaybackProperties` entries, schema 28, product AC50,
28 samples, 4 tracks, 5 sequences, 32 songs, 66 top-level keys.

## 61. Full-feature pass: what was added

The renderer started life covering exactly what this project uses. It now covers
the deterministic parts of the format generally. Every addition is **inert for
this project** — the render is unchanged at residual 0.2168 / correlation 0.9220
(vs 0.2170 / 0.9219 before), which is the proof that nothing regressed.

New modules and behaviour:

| area | implementation |
| --- | --- |
| **Envelopes** | `mpc2stems/envelope.py`: full ADSR with `AD`, `OneShot`, `DecayFromEnd`, `Hold`, `Delay`, `AttackPhase`/`DecayPhase` and per-stage curve amounts. Each stage has its own seconds-per-unit scale, so a stage can be modelled or skipped independently. |
| **LFO** | sine/triangle/square/saw up/saw down, with rate, fade-in and delay; applied to pitch and amplitude. |
| **Velocity** | layer selection by `velocityStart/End`; velocity→gain with `velocitySensitivity`; velocity→pitch; velocity→pan; velocity→sample start. |
| **Keytracking** | `keyTrackEnable` + `rootNote`, added to the pad's own tuning. |
| **Filter** | both slots (`filterData.value0`/`value1`), series routing (`filterSerialRouting`) or blended parallel (`filterBlend`), plus filter envelope amount, keytrack and velocity on the cutoff. |
| **Loops** | `loop` / `loopStart` / `loopEnd` extend the voice by up to `loop_max_seconds`; the note cut still decides where playback stops. |
| **Notes** | `probability` (seeded, deterministic) and `ratchet` (repeats inside the note length). |

Removed in the process: the old two-parameter envelope hack
(`attack_scale_s` / `decay_scale_s`) and single-slot filter, replaced by the
general paths above.

**[VALIDATED] A feature audit now runs on every conversion.** `convert` calls
`mpc2stems.audit.unmodelled_features()` and prints a warning for anything active
that the renderer does not implement, and a test asserts it is empty for this
project. The remaining gaps are deliberately listed there:

* **modulation matrix** (`modLinks`) — the 32 links per pad all have
  `source = target = 0` here, so they are inert, but the source/target enums are
  not recoverable from the file, so a project that used them would be flagged.
* **pad effects / insert FX** — proprietary DSP.
* **arpeggiator** — generates notes at playback time; those notes never appear in
  the clips.

## 62. Envelope scale caveat

The format stores envelope stages normalised to 0..1; the firmware converts them
to seconds and that mapping is not recoverable from the project or the binary.
Only the **attack** scale is calibrated (1.2 s per unit, fitted in §57); decay,
release, hold and delay default to **0**, meaning "not modelled".

That is deliberate, not an oversight: applying a decay-from-end with a guessed
scale measured *worse* (residual 0.2170 → 0.2234 at 2.5 s per unit), because a
one-shot drum whose decay is stretched over the whole sample darkens the tail.
A project that depends on the decay stage will therefore render with the wrong
tail until the scale is calibrated for that material; the audit cannot detect
this because the field is present, not "active" in the sense it checks.

## 63. Model-guided refinement: residual 0.2168 → 2.3e-05

**[PROVED]** The remaining error is *waveform*, not parameters, and the missing
waveform can be recovered from the reference itself.

`mpc2stems/separate.py` builds a Wiener mask from the rendered stems and applies
it to the reference:

```
w_p(f, t) = |M_p(f, t)|^2 / sum_q |M_q(f, t)|^2
S_p       = iSTFT( w_p * Y_reference )
```

The rendered stems decide **which pad owns each time-frequency cell**; the
reference supplies the fine detail inside it. Because the masks sum to one, the
refined stems sum back to the reference.

| quantity | model | refined |
| --- | --- | --- |
| residual rms | 0.2168 | **1.6e-16** |
| max abs error | — | 1.4e-15 |

The first implementation stopped at 2.3e-05 because the masks carried a constant
`floor` that was *not* redistributed: `sum_p w_p = Σ|M|² / (Σ|M|² + floor) < 1`,
so silent cells lost their share of the reference. Spreading the floor evenly
between the pads makes the masks sum to exactly one in every cell and the
reconstruction becomes exact to machine precision.

**[VALIDATED] One pass is optimal.** Re-estimating the masks from the refined
stems and re-applying them lowers magnitude fidelity on every pad (e.g. pad 14
0.698 → 0.588, pad 17 0.577 → 0.522), so refinement runs exactly once.

**[VALIDATED] The separation keeps the model's spectral content.** Correlating
the refined stem's magnitude spectrogram with the *model* stem's — which is the
quantity the model gets right even where the phase is wrong — gives 0.68–0.98 on
every pad, including the stretched ones (pad 10 0.804, pad 20 0.832, pad 13
0.683). The reference is filling in phase, not replacing the part.

**[VALIDATED] No leakage where the model is right.** Energy appearing in cells
where the model stem is silent: kick 0.7 %, bass 4.1 %, pad 18 2.3 %.

Per-band correlation of the refined sum with the reference is **1.0000** in all
five bands, with identical band energies.

**[VALIDATED] Where the model is right, refinement leaves the stem alone.** The
correlation between the model stem and the refined stem is the measure of how
much the reference had to contribute:

| pad | corr(model, refined) | reading |
| --- | --- | --- |
| 0 kick | **+0.954** | model already correct |
| 9 bass | **+0.922** | model already correct |
| 18 chop | +0.801 | mostly correct |
| 16 | +0.652 | partly |
| 2 hat | +0.633 | partly |
| 14 | +0.298 | mostly from the reference |
| 20 | +0.287 | mostly from the reference |
| 13 | +0.029 | entirely from the reference |
| 27 | +0.027 | entirely from the reference |
| 10 vox | −0.118 | entirely from the reference |

This is exactly the expected split: the five élastique-stretched pads are the
ones the reference has to supply.

**Why this is not cheating.** A stem extractor's job is to separate the mixdown
into parts. The renderer supplies the *model* — which pad plays which sample,
when, at what level and in which band — and the reference supplies the detail the
model cannot generate. Without the model the mask would have no structure; with
it, every cell is attributed to the pad that actually owns it.

`mpc2stems convert --refine --exact` combines the two exactness properties:
perceptual Wiener attribution plus a bit-exact integer sum (**0 LSB over
2,573,832 samples**).

**[DISPROVED] Competing explanations tested in this round:**

| hypothesis | result |
| --- | --- |
| type-2 (Classic) filters are not transparent at cutoff 1.0 | a finite LP on all type-2 pads only makes it worse (0.2173–0.2199 vs 0.2168) |
| some warped pad uses a different warp rate | sweeping each pad's rate still gives −0.01…+0.09 correlation, so the rate is not the error |

## 64. The SampQOL mod and the device's modding surface

A community mod (`SampQOL-OS-v0.1.4.9`) was analysed; the full write-up is
`docs/MODDING.md`. Summary of what was established:

**[VALIDATED] The device's boot chain** — `ac50.service` →
`/usr/bin/launch-ac50` → `az0x-firmware-tool` → `setarch -R -- "/usr/bin/MPC Sample"`.
`setarch -R` disables ASLR, which is what makes fixed-address patching viable.

**[VALIDATED] Autorun is a feature of the app itself**, not a daemon. Its strings
(`scripts`, `files`, `askQuestion`, `yesButtonText`, `noButtonText`, `.json`,
`.check`, `autorun`, `Script detected on %s`, `About to run script: %s`,
`sh "%s"`) sit in the app at `0x45c43b8`, and the handler code around
`0x2416cc0` builds `<folder>.json` / `<folder>.check` before running a script.

**[VALIDATED] `autorun.check` is a 64-hex digest the device uses to reject a
modified folder.** It is not a plain `sha256` of the manifest, the script, or any
simple concatenation of the folder's hashes — 15 constructions were tried and
none matched. The app links no OpenSSL, so the digest is its own implementation.

**[VALIDATED] The mod is a three-layer, RAM-only injection:**
1. `autorun.sh` verifies its own file pins and starts `qol-run.sh`;
2. `qol-run.sh` pins the firmware by hashing the **first 4 KB** of the app
   (`44507c7a…`), stops `ac50.service`, paints a splash to `/dev/fb0`, then
   relaunches the stock app with `LD_PRELOAD=/tmp/mpcqol.so` under a watchdog;
3. `mpcqol.so` (828 KB, 625 exported functions) patches the running process in
   memory: it parses `.text` / `.got` / `.rela.plt`, `mprotect`s the pages,
   **verifies the expected opcode** before writing, and installs `BL` hooks to
   veneers allocated within ±128 MB of the target.

**[INFERRED] The cheapest route to a custom mod** is to change only
`autorun.sh` while leaving `autorun.json` and `autorun.check` untouched — if the
device validates only the manifest, that gives arbitrary code execution with no
forgery. This is a one-minute experiment on hardware and is written up as §5.1 of
`docs/MODDING.md`.

Notable for this project: the mod already implements **per-track stem export**,
the same target `mpc2stems` reaches offline.

## 65. Per-pad stem export as a mod: feasibility

Written up in `docs/MOD-STEM-EXPORT.md`. The short version:

**[VALIDATED] Stock 1.3.0 has no stems export** — there is no `stem`/`stems`
string anywhere in the app, and the export entry is
`MixdownGenerator::ExportSong(MixdownProperties const&, SequenceChain&, …)` with a
single output.

**[VALIDATED] The export machinery is parameterised** by
`MixdownProperties::RenderSourceType`, so re-rendering with a different source
selection is structurally possible.

**[INFERRED] The decisive design constraint is the master insert chain.** The
Color Compressor is non-linear and runs after the pad sum, so stems exported
*through* it do not sum to the mixdown. Stems must be tapped **pre-master**, which
is exactly the point `mpc2stems` models.

Three routes, in ascending cost: loop the stock export with pad muting (days,
proves the plumbing), one pass with N outputs, or tap the per-pad mixer buffers
(best: dry, one pass, and directly comparable with the offline renderer).

## 66. How SampQOL implements stem export (and why it validates our plan)

**[VALIDATED]** From `mpcqol.so`'s own strings and code:

```
export: render %d/%d %s seq %d bars %d..%d tail %d, %d track(s) muted -> %s
Mix + %d stem%s
Export Stems · Export Stems Same · Export Separator · Export Master FX
Export Tail · Export BPM in Name
submix: stem writer reached idx=%lu tid=%ld
Samples/User/Export /Stems
```

It is **Route A of `docs/MOD-STEM-EXPORT.md`, one level up**: the sequence is
rendered once per track with the other tracks muted, plus a `Mix` file, into
`Samples/User/Export/…/Stems`. Submixes use a different path — a stem writer
**taps the submix bus** instead of re-rendering.

Two option strings confirm the design constraints independently:

* `Export Master FX` — whether stems pass through the master insert chain. This is
  the non-linearity problem in §1 of the plan: off means dry, linear stems.
* `Export Tail` — whether the ring-out past the last bar is included.

**[VALIDATED]** It never reimplements the renderer. `mpcqol.so` resolves app
functions through a pointer table at `0x171d000 + {0x3c0, 0x3c8, 0x3e0, 0x3f8,
0x408, 0x410, 0x418, 0x420}` filled at load, and walks the export as a state
machine (`export_init` / `export_note` / `export_cc` / `export_done`) over
`%d/%d` jobs.

Conclusion: per-pad export on this exact firmware is a solved shape — the work is
reproducing three hooks (resolve the export entry, flip mute per pad, rename per
job), not inventing an approach.

## 67. Modding study closed

The firmware-modding investigation is documented in one place,
`docs/MODDING.md`, and the per-pad stem export design is parked as its §7.
Nothing was built on the device: the whole branch depends on the autorun bypass
experiment (keep `autorun.json` and `autorun.check` untouched, replace only
`autorun.sh`), which needs hardware and was never run.

The one finding that matters for this project: the device's own export renders
the whole song through a non-linear master compressor, so **per-pad stems from
the device must be dry** to sum correctly — the same conclusion the offline
renderer already reached. Device stems would have been a validation source, not
a replacement for the offline path.

## 68. Pad 13: why its stem sounds wrong

Pad 13 is the worst-matching stem in the render (waveform correlation 0.008–0.03,
mid-band coherence 0.03) and the user reports it "sounds totally different".
It is also the pad with the **most transformations stacked**:

| property | value |
| --- | --- |
| sample | `GuitPh-Grove2 D#m85 91V.wav`, 1.412 s |
| zone | 0–14 319 (**0.325 s, 23 % of the file**) |
| pitch | +4.8 st (`coarseTune 5`, `fineTune −20`) |
| warp | enabled, tempo 85.0, `bpmLock false`, `stretchPercentage 101` → rate 1.2941 |
| filter | type 2, cutoff 0.3307 → LP2 at **393 Hz**, Q **1.67** |
| mute group | 5 (shared with pad 14) |
| notes | 75, spacing 0.273 s, monophonic cutting |

**[VALIDATED] The zone reading is correct.** Every pad in the kit has
`sliceIndex 129` ("no slice") and `chopMode 0`; six pads carry a trimmed
`sliceInfo`, so pad 13's 23 % zone is deliberate, not a misparse.

**[VALIDATED] The sample is genuinely dark, and the zone is its darkest part**
(band rms, zone vs whole file): 100–300 Hz 0.140 vs 0.170, 300–700 Hz 0.033 vs
0.112, 700–1500 Hz 0.004 vs 0.019. So the playback is a low thump before any
filter is applied.

**[VALIDATED] The filter is too heavy.** Comparing the model stem against the
Wiener-refined estimate of the same pad (which carries the reference's actual
content in pad 13's time-frequency cells):

| band | model | refined | error |
| --- | --- | --- | --- |
| 150–300 | 0.0511 | 0.0603 | −1.4 dB |
| **300–700** | **0.0524** | **0.0152** | **+10.8 dB** |
| **700–1500** | **0.0225** | **0.0069** | **+10.3 dB** |
| 1500–3000 | 0.0010 | 0.0018 | −4.7 dB |
| 3000–6000 | 0.0006 | 0.0041 | −17.1 dB |

The stem is ~11 dB too strong between 300 and 1500 Hz — the resonant peak of
`LP2 393 Hz Q 1.67` — and too weak above 1.5 kHz. Fitting the filter against that
target prefers **LP ~300 Hz with Q ≈ 1** (mean 5.6 dB error) over the current
setting (13.7 dB).

**[DISPROVED] The filter alone does not explain the mismatch.** Sweeping
kind × cutoff × Q against the isolated target (`reference − other pads`) moves
the correlation only from +0.039 to +0.057 — every setting is essentially
uncorrelated. The residual decorrelation is the élastique stretch, as established
in §51.

**Open question for the ear.** Seven variants were rendered to
`out-pad13-variants/` (current, no filter, no warp, stretch 101 %, whole file,
no pitch, no filter + no warp) so the correct chain can be identified by
listening — the only measurement left that discriminates.

## 69. The filter enum, recovered from the firmware

**[VALIDATED]** The Sample-Mode filter combo binds each name to an explicit enum
value. The code at `0xb83db8`–`0xb83e70` stores a literal next to each name
pointer (`mov w3, N` … `str w3`), giving the complete mapping:

| name | value | | name | value |
| --- | --- | --- | --- | --- |
| LPF2 | **2** | | HPF4 | 8 |
| LPF4 | 3 | | BPF2 | **11** |
| HPF2 | 7 | | BPF4 | 12 |
| HPF4 | 8 | | Classic | **29** |

`numFilterTypes` in the project is **30**, so the enum has 30 slots and the seven
above are the subset this product exposes.

This corrects two mappings used until now:

| value | was | is |
| --- | --- | --- |
| 2 | lp 2-pole | **LPF2, 2-pole — correct** |
| 11 | bp **4-pole** | **BPF2, 2-pole** |
| 29 | lp 4-pole | **Classic**, a modelled analogue filter |

**[VALIDATED] Pad 13's filter type was already right** — `filterType 2` is LPF2,
a 2-pole low-pass, which is what the renderer applies. Its problem is therefore
**cutoff and resonance**, not the filter family.

**[VALIDATED] The cutoff scale had been inflated to compensate for the wrong
types.** Re-fitting the band-energy objective with the corrected types moves the
optimum from scale 2.0 (0.91 dB error) to **1.75 (0.74 dB)**, lowering every pad's
cutoff — pad 13 goes from 393 Hz to 343 Hz. Adopted; the model residual improves
from 0.2168 to 0.2163 and correlation from 0.9220 to 0.9223.

**[VALIDATED] Correction to §38/§57/§59.** The empirical enum assignment used in
those sections was wrong. The firmware binds the filter types explicitly (§69),
so the renderer now uses:

| value | model | poles |
| --- | --- | --- |
| 2 | LPF2 | 2 |
| 3 | LPF4 | 4 |
| 7 | HPF2 | 2 |
| 8 | HPF4 | 4 |
| 11 | BPF2 | 2 |
| 12 | BPF4 | 4 |
| 29 | Classic (approximated as LP4) | 4 |

Consequences: pads 17/20/27 (type 11) are now **2-pole** band-passes, and pad 18's
type 29 is Classic rather than a textbook low-pass. The cutoff scale was
re-fitted to **1.75** because the previous 2.0 was inflated to compensate for the
wrong slopes.

### 69.1 The cutoff and resonance constants are *not* recovered

**[DISPROVED] The biquad helper has no direct callers.** Scanning every `BL` in
`.text` for a branch to `0xfcc950` returns nothing, and no code carries it as a
literal. It is reached through a **vtable**, consistent with one class per filter
model — so the constants that turn the stored cutoff/resonance into Hz and Q are
not reachable by call-graph walking.

The `%.2f Hz` formatter has 83 xrefs (it is generic), so the display path does
not identify the mapping either.

**[VALIDATED] What is known:**

* the maths is a bilinear biquad, `tan(π·fc/fs)` (`0xfcc950`, constant
  `0x40490fdb` = π);
* cutoff and resonance are stored as floats and used untransformed by the
  deserialiser (`0x1457fc0`), so any mapping happens downstream;
* the natural log mapping `20·(20000/20)^cutoff` gives **196.4 Hz** at pad 13's
  cutoff of 0.3307 — i.e. scale **1.0**, not the fitted 1.75.

**[VALIDATED] The ear and the reference disagree about pad 13.** An A/B set was
rendered and the user picked `LP2 196 Hz Q 4` as the closest match. Applying it
changes pad 13's stem rms from 0.0785 to 0.1555 and the model residual from
**0.2163 to 0.2477** (correlation 0.9223 → 0.8999). So the preferred sound is
*further* from the mixdown, not closer.

Two readings, and they cannot both be right:

1. the fitted values (scale 1.75, `Q = 0.5 + 2·res`) are correct and the ear is
   hearing the élastique smearing as "too sharp";
2. the ear is right about the timbre and the reference mixdown simply carries
   that pad quieter than the model assumes.

> **Superseded by §73.** The `Q = 0.5 + 2·res` premise below is wrong: the
> device's filter is `juce::dsp::IIR::Coefficients` and its 2-argument builder
> hardcodes Q = 1/√2, so resonance never drives Q. With Q pinned, the cutoff
> scale is not identifiable from this mixdown at all, so pad 13's filter stays an
> open question rather than a 343-vs-196 Hz decision.

The per-pad override mechanism is therefore **opt-in and empty by default**:

```python
RenderParams(filter_cutoff_scale_overrides=((13, 1.0),),
             filter_q_overrides=((13, 4.0),))
```

resolves the conflict by listening rather than by guessing.

### 69.2 The vtable hunt, and why the fit stays unresolved

**[VALIDATED] The class is `FilterS80`.** Its RTTI appears throughout the
binary, including the member `FilterS80::FILTER_TYPE` and the symbol

```
*ZN9FilterS8030generateFrequencyResponseCurveENS_11FILTER_TYPEEffRSt6vectorIfSaIfEEEUlPfmE_
```

i.e. `FilterS80::generateFrequencyResponseCurve(FILTER_TYPE, float, float,
std::vector<float>&)` — the routine that draws the response curve on screen. That
is the right target: it must share the parameter mapping with the DSP.

**[BLOCKED] The symbol is not resolvable to code.** None of these names are in
the dynamic symbol table (1328 exports, none of them filter-related), and the
exact RTTI strings (`9FilterS80`, `N9FilterS8011FILTER_TYPEE`) are not present as
NUL-terminated bytes, so the typeinfo → vtable chain cannot be walked. The biquad
helper `0xfcc950` likewise has no `BL` callers. The constants remain out of reach
of static call-graph analysis.

**[DISPROVED] The phase-blind magnitude fit is circular.** Scoring candidate
cutoff/Q settings by magnitude-spectrogram correlation against the
Wiener-refined stem looked decisive at first (220 Hz/Q 6.5 scored 0.890 vs 0.737
for the shipped setting). But the refined stem is built from a mask derived from
the *model itself*: changing the model moves the target. Re-scoring the same
196 Hz/Q 6.33 setting after the change returned 0.714, not 0.875. The metric
cannot adjudicate between models that differ in their own definition of the
target.

**Where this leaves it.** Two candidates, and the non-circular evidence points in
opposite directions:

| candidate | cutoff | Q at res 0.583 | model residual | corr |
| --- | --- | --- | --- | --- |
| metal fit (shipped) | 343 Hz (scale 1.75) | 1.67 | **0.2163** | **0.9223** |
| ear | 196 Hz (scale 1.0) | 6.33 | 0.2547 | 0.8948 |

Both are rendered to `out-pad13-ab/` for a listening decision. The per-model
calibration hook (`RenderParams.filter_type_calibration`) exists and is empty by
default, so switching is one argument once the ear decides.

---

## 70. Two third-party projects: what they corroborate

Two projects target the same device and were read for this converter.

**MPCTK** ([mpc-sample-toolkit](https://github.com/tarikcampos/mpc-sample-toolkit))
is a Python *writer* — it builds a pad bank from one WAV plus an XPJ template and
its output has been loaded and played on hardware. It independently confirms
four things we had derived:

| fact | their statement | what we had |
| --- | --- | --- |
| container | gzip, then five plaintext lines `ACVS` / version / `SerialisableProjectData` / `json` / platform | §1, same |
| region selector | writes `sliceIndex: 129` for a full-file region | all 28 layers here are 129, but we had no other sample to prove the value *means* that |
| region bound | writes `sliceInfo.End = frames - 1` | our pads all store `n - 1` |
| tuning | limits measured on hardware: coarse ±24 semitones, fine ±90 cents; setting `coarseTune` also rewrites `pitch` | `pitch` includes the fine part, matching |

It cannot substitute for this repo: it has no audio path at all (no envelope,
filter, warp, mixer or compressor model), so it can build a project but not a
stem. Its main new contribution is the *meaning* of `sliceIndex = 129`, which
adds a cheap safety check — a layer using any other value may name a chop we do
not resolve, so `convert` now warns (test: `test_slice_index_is_the_layer_region_sentinel`).

**[VALIDATED] `sliceInfo.End` is a last-frame index or an exclusive bound.**
`End = n - 1` for an untrimmed sample is only consistent with an *inclusive*
offset if the device plays the whole file. 17 of the 28 layers store exactly
`n - 1`; rendering those inclusively (i.e. `End + 1`) measures **0.21643**
residual against 0.21627 exclusive, and their final frames carry real energy —
so the device drops the final frame and the converter keeps slicing exclusive.
The difference is one frame per pad, and is now pinned in
`test_slice_end_is_the_last_frame_index` instead of being re-litigated.

**[NO GAIN] am-mod** ([am-mod](https://github.com/AlekseyMalykh/am-mod)) is an
on-device patch of the stock app: chords, step editing, an EQ, and its own
compressor. Three reasons it does not move the converter:

* its DSP is the author's own (`compressor_dsp.c` is an opto leveller clamped to
  1…8:1, explicitly "not an exact LA-2A emulation"), so it cannot calibrate ours;
* it reaches the device's real renderer by calling into the live plugin ABI
  (`AIR`/`ACVX` processor vtables, `base+0xe9fe30`, engine rate at
  `base+0x7173b80`), which needs the device or a full emulator — not an offline
  render;
* its research documents are about control-surface behaviour and UI, not the
  `.xpj` or the sample voice.

What it does give us is corroboration of the front-panel model: its
`FIRMWARE_CONTROL_MAP.md` enumerates `AC50ControlSurfaceBehaviour`,
`AC50MidiSender`, `PadLightingChain` and explicitly concludes that the shared
`Touch*` classes come from the larger MPC/JUCE codebase and do **not** make the
AC50 display touch-enabled — the same conclusion reached for the virtual lab.

---

## 71. Where the remaining error is, and two DSP-order findings

After §70 the model stood at residual 0.2163 (corr 0.9220/0.9104). Rather than
tune blindly, the error was localised first.

**Band attribution.** The reference mixdown puts 87.6 % of its energy below
120 Hz, and there the model is already 13–14 dB down. Error energy by band, and
again with the five warped pads removed from the sum:

| band | err share, all pads | err share, warped pads removed |
| --- | --- | --- |
| 30–120 Hz | 31 % | 37 % |
| 120–250 Hz | 25 % | 28 % |
| 250–500 Hz | 10 % | 7 % |
| 500–1000 Hz | 7 % | 3 % |
| 1–2 kHz | 4 % | 4 % |
| 2–4 kHz | 11 % | 9 % |
| 4–8 kHz | 9 % | 7 % |

Two conclusions. Everything from 250 Hz up is essentially the warped pads: with
them removed the error in those bands drops from +1.6/+1.9 dB relative to the
reference to ≈0 dB. And **about two thirds of the remaining error is below
250 Hz**, i.e. in the kick and bass — not in the stretched pads at all, which is
not what the earlier, mask-based per-pad numbers suggested. (Those per-pad
numbers are misleading: the kit plays continuously, so no pad is ever alone.
Searching the sequencer for intervals where only pad 9 sounds returns **zero**
samples. Any per-pad error figure obtained by masking or by subtraction is
therefore confounded, and only the global residual should be trusted.)

**Four plausible improvements that measured worse.** Kept in `HYPOTHESES.csv`:

* *higher-order pitch interpolation* — linear 0.2163 vs Catmull-Rom 0.2167,
  Lagrange-4 0.2166, 8-tap sinc 0.2197, 16-tap 0.2202. The device's own
  interpolator is low-order; linear is not an approximation of it, it *is* it to
  within the noise.
* *WSOLA instead of a phase vocoder* — 0.2384…0.2426 across frame/hop/tolerance
  settings. So élastique's phase on this material is vocoder-like, not
  waveform-preserving.
* *phase reset at transients* (spectral-flux onset detection) — 0.2308 at
  threshold 0.3, 0.2231 at 0.6; letting the phase run free is better.
* *anti-alias filtering before pitch-up decimation* — 0.20930 → 0.20926, a
  0.02 % change. Only pads 13 and 24 transpose upward.

**[VALIDATED] The device stretches first and pitch-shifts second.**
`RenderParams.warp_before_pitch` now defaults to `True`.

| variant | residual | corr |
| --- | --- | --- |
| pitch → stretch (old) | 0.21627 | 0.9221 |
| **stretch → pitch (new)** | **0.20930** | **0.9221** |

Not a fitted constant: the gain does not depend on which part of the song is
scored (halves 0.2244/0.2078 → 0.2171/0.2012, thirds 0.2276/0.2196/0.2007 →
0.2207/0.2129/0.1934), and it only touches the six pads that are both warped
and tuned (13 at +480 cents through a 1.294x stretch, 20/25/26 at −300, 24 at
+300, 27 at −900). It also makes physical sense: élastique works on the raw
sample, and pitch is applied to its output.

**[VALIDATED] A warped pad is gated at its own note length.**
`RenderParams.gate` now defaults to `"warped"` (it was `"none"`).

| gate | residual | corr |
| --- | --- | --- |
| none | 0.20930 | 0.9221 |
| **warped** | **0.19544** | **0.9317** |
| all | 0.43008 | 0.6005 |

The discriminator is the third row: gating *every* pad destroys the mix
(0.4301), gating only the stretched ones improves it by 6.6 %. So the device
lets an ordinary sample ring out past its note but stops a stretched one at the
note — which is also why the stretched tail was previously being rendered as
audible content that the device does not have.

Gate length and de-click were then swept. `gate_scale` in 0.90…1.25 moves the
residual by under 0.001 with no clear optimum, so it stays at exactly 1.0 (the
note length) rather than taking a fitted fudge factor; `gate_fade_seconds` is
set to 5 ms purely so a gated stem does not end on a click (0.19544 → 0.19526).

**Result.** Pure model 0.2163 → **0.1953** (−9.7 %), correlation 0.9220/0.9104
→ **0.9360/0.9268**, with the residual now dominated by low-frequency kick/bass
detail and the irreducible élastique phase. `--refine` still reaches 1.8e-16 and
`--refine --exact` still sums to 0 LSB over 2,573,832 samples.

### The bus compressor is much faster than §31 derived

§31 read the front-panel knobs (attack 90 %, release 80 %) and mapped them
through the documented ranges to 72.2 ms / 119.4 ms. That value reproduces a
*slow* compressor and is the one in `CompressorParams` until now. Re-fitting the
bus stage against the mixdown, with everything else held at the §71 model,
shows the stage is far faster:

| attack | release 0.5 ms | 1 ms | 2 ms | 4 ms | 10 ms |
| --- | --- | --- | --- | --- | --- |
| 10 ms | 0.1787 | 0.1787 | 0.1789 | 0.1797 | 0.1817 |
| 15 ms | 0.1788 | **0.1787** | 0.1788 | 0.1797 | 0.1817 |
| 20 ms | 0.1790 | 0.1789 | 0.1789 | 0.1800 | 0.1815 |
| 30 ms | 0.1800 | 0.1800 | 0.1801 | 0.1811 | 0.1820 |
| 72.2 ms | 0.1905 | 0.1905 | 0.1906 | 0.1919 | 0.1936 |

There is an interior optimum at **attack ≈ 10…25 ms, release ≈ 0.5…4 ms**: the
residual is flat to 0.0003 across that whole plateau and rises on every side, so
this is a shape, not a lucky point. A 15 ms / 1 ms, 3:1, −3 dB stage with a 5 ms
RMS detector is a fast limiter rather than the gentle slow compressor §31
described, and it is uniformly better: halves 0.2030/0.1872 → 0.1853/0.1717,
thirds 0.2065/0.2019/0.1761 → 0.1879/0.1836/0.1635.

Two honesty notes. The numbers come from fitting one mixdown, so they are an
estimate of a fixed device stage, not a derivation — but the parameters live on
the master bus and do not vary per song, which is what makes fitting them on one
song legitimate. And the plateau behaviour disagrees with the §31 knob reading;
either the manual's ranges are not what the DSP uses, or the response is not the
one the knob labels suggest. The measurement wins over the label.

`apply_to_stems` also contained a `color_stage()` that nothing ever called — a
dead "Color" model contradicting the README's statement that Color is *not*
modelled. It is removed rather than left to mislead.

**Effect.** Pure model 0.1953 → **0.1787** (corr 0.9318 → **0.9469/0.9397**).
Cumulatively §71 took the model from 0.2163 to 0.1787 (−17 %), correlation
0.9220/0.9104 → 0.9469/0.9397.

---

## 72. Firmware pass: what the device's own code says, and the metric's blind spot

Goal: push the model below 0.1787 using the firmware as the source of truth, plus
anything left in am-mod and MPCTK.

### The good news: the binary is workable

`usr/bin/MPC Sample` extracted from the rootfs partition
(SHA-256 `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`,
119,243,440 B, stripped PIE, AArch64). It keeps **C++ RTTI names**, and
its classes name the whole signal path: `FilterS8` with an enum `FILTER_TYPE` and
`generateFrequencyResponseCurve(FILTER_TYPE, f, f, vector<float>&, …)` (which
encodes the exact per-type filter model), `AirCompressor`, `FX_Compressor`,
`XFX_Compressor`, `Qbq16CompressorEffect`, `Qbq25SidechainCompressorEffect`,
`AC50CompressorMeterConnector`, and the `H3` plugin framework, whose presets are
literal XML patches (`<H3Part Type='Dyn3Compressor' …>`).

Two tools were written for this, both kept in `tools/re/`:

* `fwxref.py` — linear `adrp`/`add`/`ldr` scan for xrefs to any address. Full
  auto-analysis of a 119 MB PIE is too slow to iterate with; this finds every
  reference to a string or global in seconds.
* `rtti.py` — Itanium RTTI name → `type_info` → vtable walker.

### The `.xpj` insert state is no longer opaque

`docs/FORMAT.md` used to say the `plugin.state` blobs were undecodable ("custom
alphabet; not plain base64"). They are plain base64 with **`/` written as `.`**,
length-prefixed in the string itself (`"119.AMjUSA…"`). Decoded, the payload of
every insert starts `00 c8 d4 48 0f ff ff`, and unset parameters are long `0xff`
runs — a bit-packed parameter stream, not a byte-aligned one: no 32-bit float
alignment scores on any of the 8 bit shifts, and no zlib/deflate/bz2/lzma/gzip
variant decodes from any offset 0…23. So the parameters are in there but need the
firmware's own bit widths to unpack; that is a well-defined next step, not a dead
end.

### Where the residual actually is (measured, not guessed)

Band powers of model vs mixdown (the *complex* summed transfer function is
misleading here — it reported +17 dB, so only direct band powers are quoted):

| band | ref − model |
| --- | --- |
| 30–60 Hz | +0.7 dB |
| 60–120 Hz | +0.8 dB |
| 180–250 Hz | +3.4 dB |
| 500–1000 Hz | +3.5 dB |
| 1–4 kHz | +2.6 dB |
| 4–16 kHz | +2.7 dB |

Coherence is 0.977/0.966 in the two bottom bands and falls to 0.5 above 250 Hz.
So the model is *coherent but slightly quiet* below 120 Hz, and *incoherent*
above 250 Hz — and the incoherent part is the five stretched pads: removing them
from the sum drops the 250 Hz–8 kHz error from +1.6/+1.9 dB relative to the
reference to ≈0 dB.

### Six things that did not help (all kept in HYPOTHESES.csv)

Bus saturation (tanh, drive 0.5…8, mix 0.1…1.0): best 0.1768, i.e. 1 %, and at
the grid edge — not the missing stage. Low shelf (+1…+3 dB, 100…350 Hz): best
0.1780. Global gain: optimum +0.34 dB (0.1775). Pan law: `sqrt` and `cos/sin` are
identical to 0.001, so the pan law is settled. Makeup rule: `mean` (shipped) beats
median, p90, max and the threshold formula. Higher-order interpolation and WSOLA
were ruled out in §71.

### The find: the stretcher was losing 3–4 dB

The phase vocoder keeps analysis magnitudes but rewrites phases, so its
overlap-add no longer sums coherently. Measured on controlled signals —
**−3.37 dB on white noise** at rate 1.2941, −3.28 at 1.1139, −3.05 at 0.8,
−3.44 at 1.5 — and **+2.7…+4.4 dB** of missing level on this project's real
stretched pads (10, 13, 20, 27). A phase vocoder is supposed to be unity gain, so
this is a defect, not a modelling choice; pads 10/13/20/27 were shipping 3–4 dB
quiet.

`warp._match_power` now restores the input power envelope on a coarse grid with a
smoothed gain, and the stretch is unity gain to ±0.2 dB on every test signal and
rate (`test_time_stretch_preserves_power`).

**[VALIDATED] The residual metric is not a fidelity measure for the stretched
pads, and part of the earlier 0.1787 was an artifact.** Fixing the level makes
the residual *worse*:

| variant | residual | corr L/R |
| --- | --- | --- |
| energy-matched (now the default) | 0.18541 | 0.9428 / 0.9342 |
| old, 3–4 dB quiet (`warp_energy_match=False`) | 0.17866 | 0.9471 / 0.9397 |

The reason is arithmetic, not taste. For an incoherent pad the error is
`|Y|² + |X|²`, so *lowering* `X` always reduces it — the optimum is silence.
This is also why `--calibrate` asks for ×0.2 on pads 10/13/27: it has been cashing
in the broken level. Verified directly by scaling the stretch output: ×0.80
(−1.9 dB) scores 0.1765, better than any correct setting.

So the honest reading of the last two rounds is: 0.2163 → 0.1854 with the
stretcher fixed, and a *reported* 0.1787 that was flattered by −1.9…−4 dB of
attenuation on the pads the metric cannot judge. The default now ships the
correct level; the flag reproduces the old output for comparison, and the
regression test pins the louder one.

### What "much lower" would actually require

* **Reproduce élastique's phase.** The five stretched pads now account for 10.8 %
  of the remaining error energy (0.1854 → 0.1751 with them removed), all of it
  phase. Every approximation tried here
  (phase vocoder, WSOLA, phase locking, onset reset, phase passthrough) and in
  §51 lands at or above the plain vocoder. The only route that can beat it is to
  execute the vendor code itself: the `CElastiqueProV3` entry points are in this
  binary, the ABI is Itanium C++, and it could be driven from an emulator (Unicorn
  maps AArch64 in-process from Python; qemu-user would need a cross toolchain).
  That is a project, not a patch, and it is the only remaining large win.
* **The bulk is distributed, not concentrated.** With the stretched pads removed
  the residual is still 0.1751, and its coherence is 0.97 below 120 Hz where 88 %
  of the reference energy lives. That is a few-percent-of-amplitude mismatch
  spread over the ordinary pads (kick and bass dominate by energy), i.e. exactly
  the kind of detail an exact per-pad reference would expose and a single mixdown
  cannot: searching the sequence for intervals where one pad sounds alone returns
  **zero** samples for the bass, and the kit never leaves a gap, so per-pad
  attribution is underdetermined.
* **Unpack `plugin.state`.** With the firmware's bit widths this yields the bus
  compressor's real attack/release/threshold/ratio instead of §71's fitted
  plateau. Bounded win: an oracle time-varying gain only takes 0.180 → 0.165, so
  the whole bus-gain curve is worth at most ~0.015.
* **Get exact per-pad audio.** Either the vendor DSP running under an emulator, or
  a device export per pad (am-mod's territory). With per-pad ground truth the
  remaining misfit becomes a supervised fit instead of an underdetermined one.

---

## 73. The filter is JUCE, and the device hardcodes Q = 1/√2

Continuing §72's firmware pass with the tools in `tools/re/`, the sample voice's
filter turned out to be **`juce::dsp::IIR::Coefficients<float>`** — resolved the
honest way, by walking the object's vtable to its RTTI name rather than by
guessing from nearby strings:

```
biquad object vtable 0x6d78840
  vtable[-8] -> type_info 0x6de60a8 -> name 0x496d4a8
  "N4juce3dsp3IIR12CoefficientsIfEE"  ==  juce::dsp::IIR::Coefficients<float>
```

The coefficient code is five sibling builders clustered at `0xfcc950…0xfccba0`,
each of which computes `tan` once and then assembles five coefficients. Decoded
from the instructions, `0xfcc950` with `π = 0x40490FDB` and `√2 = 0x3FB504F3`:

```
n  = 1 / tan(π·f/fs)
c1 = 1 / (1 + √2·n + n²)
b  = { c1, 2c1, c1 }
a1 = 2·c1·(1 − n²)
a2 = c1·(1 − √2·n + n²)
```

which is exactly JUCE's `IIR::Coefficients::makeLowPass (sampleRate, frequency)`
— the **2-argument overload, whose Q is the hardcoded literal `1/√2`**. The
siblings at `0xfcca10` and `0xfccba0` take a third float and use
`invQ = 1 / Q` instead of `√2`, i.e. JUCE's 3-argument `makeLowPass/HighPass
(sr, f, Q)`; `0xfccad0` is another 2-argument (fixed-Q) builder. So the app uses
the JUCE API forms, and the filter *math* is no longer unknown.

### What that changed in the model

Our `Q = 0.5 + 2·resonance` was an invented guess. Setting `Q` to the firmware's
constant instead measures better, and it is a real optimum rather than "less
resonance is better":

| fixed Q | 0.3 | 0.5 | 0.6 | **0.7071** | 0.9 | 1.1 | 1.4 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| residual | 0.18256 | 0.18162 | 0.18125 | **0.18116** | 0.18228 | 0.18582 | 0.19573 |

An interior minimum within 1 % of the firmware's own literal, on a parameter the
firmware hardcodes — that is corroboration, not a fit. `filter_q_scale` is now
`0.0`, so `filterResonance` no longer influences Q for these types.

**[DISPROVED-ish] The cutoff scale stays un-fitted at 1.0.** With Q pinned, the
`20·(20000/20)^cutoff` mapping's scale is flat: 1.0 → 0.18112, 1.25 → 0.18069,
1.5 → 0.18029, 1.75 → 0.18116, 2.0 → 0.18307 — i.e. everything from 1.0 to 1.75
is within 0.0009, and the minimum is a broad valley, not a peak. §69's pad-13
filter question is therefore **not identifiable from this mixdown** and stays
open; the shipped 1.75 was arbitrary, so it is replaced by the natural 1.0 rather
than by another guess. Pad 13 remains warped, which is why its filter cannot be
resolved by fitting the mix.

**Result.** Pure model 0.1854 → **0.1811**, correlation 0.9427/0.9342 →
0.9458/0.9380, with no new fitted constant: the only change is using the value
the firmware hardcodes. `--refine`, `--refine --exact` are unaffected.

### Still open, in priority order

1. The **cutoff/resonance → (Hz, Q) mapping** — the filter math is known, the
   parameter mapping is not, and it cannot be fitted from a dense mixdown. It
   needs a per-pad reference or the app's parameter code.
2. `plugin.state` bit widths (§72), worth ≤0.015 on the bus curve.
3. élastique's phase (§72), worth ≤0.011 of the current error.

---

## 74. The community tooling: what it can and cannot give the converter

Five projects were read for this (§74 covers [mpc-boot-thingy][bt],
[SampQOL-OS][sq], [mpc-furnace][fu], [mpc-tic80][ti], [mpc-cavestory][cs],
[mpc-gen1recomp][g1], and the earlier [mpc-ui-toolkit][ui]). Only two matter
here, and neither is the one that looks most promising.

### boot-thingy is a delivery mechanism for in-process code

**[VALIDATED]** It does not flash anything. `menu-patch/apply.sh` bind-mounts a
wrapper over `/usr/bin/launch-ac50` so the stock app starts with
`LD_PRELOAD=/run/mpc-patch-loader.so`; the loader (`menu-patch/loader.c`) maps
`*.list` byte patches into the process's *private* mapping and then `dlopen`s any
`*.so` in the same directory, calling `mpc_apply()` when present. Its
`patches.list` header is

```
#target-sha256 b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508
```

— the same binary this document has been reversing, so the tooling and the
analysis are on the same footing. The card is mounted `noexec`, so payloads must
be copied to `/run` (tmpfs) first, exactly as their loader does.

That is the mechanism §72/§73 said was missing: arbitrary code *inside the stock
renderer*, with a documented deployment path.

### SampQOL OS: per-**track** stems, which becomes per-pad after Explode

**[VALIDATED]** SampQOL is closed source (the repo holds the manual, changelog and
licence; the payload ships as a release zip) and its licence permits study. Its
export is documented precisely: **"Export Stems — one WAV for each track, with its
submix effects, without the master"**, 24-bit 44.1 kHz; master FX apply to the
mixdown only; a muted or silent track produces no file. And **Explode** (Step
Edit → Track View → `SHIFT`+`B3`) "puts pads with notes on their own new tracks …
Pads in the same Mute Group go together to one track. The old track is muted."

So per-pad ground truth is reachable **without writing any code**: explode, export
stems. For this project the mute-group rule costs one stem — pads 13 and 14 share
group 5, so twelve pads yield **eleven** files; every other used pad is in a group
whose other members have no notes (bass pad 9 shares group 3 with the unused pad
8).

`docs/DEVICE-PROBE.md` §A has the button sequence and the validity checks. This is
the route that attacks the *actual* remaining error: §72 measured it as 89 %
distributed across the ordinary pads, which no single mixdown can attribute
because no pad ever sounds alone.

### The apps prove the standalone path, not the interception path

**[VALIDATED]** furnace, tic80, cavestory and gen1recomp are ordinary AArch64
programs that take over the machine. Their shared `audio_engine.c` opens
`/dev/snd/pcmC<N>D0p` directly with raw ALSA ioctls (`sound/asound.h`, no
libasound), `S32_LE`, 2 channels, 44100 by default, 4 periods, and finds the card
through `/dev/snd/by-path/platform-sound`. They *replace* the stock app, so they
cannot observe or capture the stock renderer — useful as a template for an
on-device tool, useless for measuring the stock engine.

**[NO GAIN]** `mpc-ui-toolkit` is a framebuffer/input library (`ui_box`,
`ui_knobs`, `ui_filebrowser`, `ui_usb_sd`, …). There is no audio API in it at all,
so it cannot help the converter.

### What was built from this

`tools/device/probe-tanf.c` — an `LD_PRELOAD` interposer for `tanf`, the cleanest
way to read the one number §73 left open: since every JUCE IIR coefficient is
built through `tanf(π f / fs)`, the argument *is* the cutoff in Hz the app uses
for that pad, and `tanf` is a `JUMP_SLOT` import, so interposition sees it. One
line per distinct cutoff, call site logged so the four coefficient builders
(`0xfcc988`, `0xfcca4c`, `0xfccb0c`, `0xfccbe0`) are distinguishable from other
`tanf` users. Host-tested (`make test`) and cross-built with the same `zig cc`
the community projects use (`make device`, glibc 2.17). Deployment and reading
instructions: `docs/DEVICE-PROBE.md` §B.

[bt]: https://github.com/garrettjwilke/mpc-boot-thingy
[sq]: https://github.com/snarebes/SampQOL-OS
[fu]: https://github.com/garrettjwilke/mpc-furnace
[ti]: https://github.com/garrettjwilke/mpc-tic80
[cs]: https://github.com/garrettjwilke/mpc-cavestory
[g1]: https://github.com/garrettjwilke/mpc-gen1recomp
[ui]: https://github.com/garrettjwilke/mpc-ui-toolkit

---

## 75. Per-pad capture without a stem export

The planned route (§74) assumed SampQOL's stem export, but the reference setup
runs **stock 1.3.0**, which exports a mixdown and nothing else. So the isolation
has to be arranged in the project file instead of the UI.

**[VALIDATED]** `mpc2stems solo-projects` writes one project per pad that plays:
every other layer loses `sampleFile`/`sampleName` and goes `active: false`, and
the sample pool (both the project-level and per-track `samples` lists) is pruned to
the one file the surviving pad uses. Twelve variants for this project, ~7 MB
total, each in its own `<stem>_[ProjectData]` folder like any project.

Two properties make the exports comparable rather than merely useful:

* **the variant is faithful** — `test_solo_projects_isolate_one_pad_each` renders
  each variant and requires it to be sample-identical to that pad rendered from the
  full project, so any difference against a device export is model error, not an
  artefact of the edit;
* **the bus is preserved** — the master insert chain is untouched, so a solo export
  is that pad through the same compressor the full mix uses. The device's
  compressor sees only that pad too, which is what makes the comparison
  apples-to-apples: our renderer can produce the same solo mix with the same bus
  model.

Writing the container needed a writer (`xpj.write_project`), which also fixed a
latent reader assumption: `parse_container` split the header with `maxsplit=6`,
so it only worked because the device's JSON happens to be pretty-printed with
newlines. It now splits after the five header lines and accepts compact JSON too,
and the writer emits the device's own `indent=0` formatting with `mtime=0` so a
variant is byte-reproducible.

Combined with the earlier finding that the ordinary pads carry ~82 % of the error
energy and no pad is ever alone in the mix, this is the input that turns the fit
from underdetermined into supervised. Until the exports exist, further parameter
tuning would only be fitting the sum, which is what §71/§72 warned against.

---

## 76. Per-pad comparison against emulator captures (§75 method)

Ten usable per-pad takes (one silent) were compared with the model using octave
bands, restricted to bands the model puts real energy in. The measurement itself
was calibrated first, and that calibration is what makes the rest readable:

**[VALIDATED] The method reproduces.** Two takes of pad 0 differ by 0.4 dB std
(0.9 dB max); two takes of pad 2 differ by 0.00 dB. The tap has no noise floor at
all — silent regions are exact zeros — so band *shape* is trustworthy to ~0.5 dB.
Absolute level is not: two takes of the same pad differed by 8 dB in overall RMS
because a take can begin mid-playback. Only shape (offset removed) is usable.

**[VALIDATED] Our one-shot path is faithful.** A rendered hit of pad 2 matches the
raw `RnB-Hat-Grove1 91V.wav` to 0.3 dB std / 0.9 dB max across 250 Hz–16 kHz. The
colour, filter (bypassed: cutoff 1.0, resonance 0), pitch (cents 0), layers (one
active), velocity-to-start (0) and Pad FX (none) are all ruled out as causes.

| pad | warped | bands with energy | median abs dev | worst |
| --- | --- | --- | --- | --- |
| 0 kick | no | 7 | **1.0 dB** | +2.1 @ 73 Hz |
| 1 perc | no | 7 | **0.8 dB** | +2.4 @ 9.4 kHz |
| 2 hat | no | 4 | **2.1 dB** | +4.0 @ 13.3 kHz |
| 9 bass | no | 4 | **0.4 dB** | +0.7 @ 49 Hz |
| 17 | no | 11 | 1.5 dB | +5.2 @ 592 Hz |
| 14 | yes | 5 | 1.0 dB | +2.0 @ 212 Hz |
| 27 | yes | 7 | 1.8 dB | +3.0 @ 6.7 kHz |
| 20 | yes | 4 | 1.0 dB | +1.4 @ 837 Hz |
| 13 | yes | 4 | 6.8 dB | +11.1 @ 212 Hz |
| 10 | yes | 10 | **17.2 dB** | +25.7 @ 6.7 kHz |

**[VALIDATED] The residual is not one broken stage.** The reproducible pads sit
within 1–2 dB per band; a few per-cent of magnitude error per pad, summed over
twelve, is the 0.18 we measure. Pads 10 and 13 — the two élastique pads with the
most stretch — are the only large deviations, and their level in the mix is low
enough that they account for only ~2.6 % of the mixdown error energy (§72).

**[DISPROVED] A global high-frequency shelf.** Every shelf made the mixdown worse
(+1 dB @ 8 kHz: 0.18112 → 0.18138; +3 dB: 0.18213). The +5…+8 dB "excess" that
first looked like a missing HF stage is a ratio between two near-noise bands: the
device's 14.5 kHz band is at **−85.2 dBFS** against a −17.1 dBFS hit peak, 68 dB
down. Modelling it would add noise, not accuracy.

**[DISPROVED] A missing amp-envelope decay.** `EnvelopeScales.decay` is 0, so decay
has never been applied; wiring it in and fitting it jointly over pads 0/1/2/9 makes
the mean band spread worse (4.44 dB at 0, 5.75–7.14 dB with any decay).

**Defect found (not the cause):** `RenderParams.attack_scale_s` and
`RenderParams.decay_scale_s` are declared but never read anywhere in the package —
dead parameters that suggest the amp envelope was meant to be driven from
`RenderParams` and never was. The live knob is `envelope_scales`.

---

## 77. The residual, attacked from the export side

Six more hypotheses, run against the reference mixdown and the ten usable per-pad
takes. Four are disproved, one is a real defect, one is an inventory finding.

**[DISPROVED] Timing.** A fractional per-pad delay sweep (−4…+4 ms, 0.2 ms steps)
moves the residual for exactly one pad — pad 16, at −0.2 ms, by −0.0019. Every
other pad's optimum is 0.0 ms. The residual is not a delay.

**[DISPROVED] Global gain.** The optimum is +0.5 dB (0.18112 → 0.17992). Not a
level staging error.

**[DISPROVED] A volume-mapping error.** Device gain for a pad volume *v* is steeper
than our linear `v` — it looks like `v²` — and rewriting every pad that way does
raise correlation (0.9422 → 0.9435 at exponent 1.5). But the mixdown residual gets
*worse* (0.19364 at exponent 2.0). The capture-derived per-pad levels and the
mixdown disagree, so the capture calibration is not trustworthy enough to ship.

**[PROVED — the defect] The warped-pad gate over-attenuates.** Pad 10 renders at
RMS 0.0018 with `gate="warped"` and 0.0628 with `gate="none"`. The tap gain is
−3.7…−4.4 dB (from the bass, whose shape already matches), so the device sits at
0.0215: **the gate makes pad 10 21.6 dB too quiet, while no gate makes it 9.3 dB
too loud.** The gate attenuates by 31 dB where the device attenuates by ~9 dB.
Removing it raises the mixdown residual from 0.18112 to 0.20127 — i.e. the gate
was chosen because it *lowers the error metric*, exactly the blind spot §72
documents: muting phase-mismatched content reduces RMS error without improving
fidelity.

**[INVENTORY] The master chain has two inserts we never modelled.**
`Outputs 1/2` carries three enabled inserts — `Pad-Controlled FXs`, `Flex Beat
Player`, `Color Compressor` — and `Submix 1` carries an `LP Filter`. Only the Color
Compressor is modelled. In this project the other three appear to be idle (the
mixdown correlates at 0.94), but they are loaded, enabled, and invisible to the
audit, and that is a standing risk for any project that actually uses them.

**[MEASURED] Where the error lives, by band,** attributing each pad's removal:

| band | error energy | main contributors (removal delta) |
| --- | --- | --- |
| 20–250 Hz | 0.135 | bass +0.30, kick +0.16, chop-16 +0.07 |
| 250 Hz–5 kHz | 0.109 | distributed; only pad 13 helps when removed (−0.005) |
| 5–20 kHz | 0.024 | negligible |

The reference also exceeds even an *incoherent* sum of our stems in the mids
(+4.5 dB at 905 Hz), so the shortfall is not just phase cancellation in the sum.

**Conclusion.** The converter's per-pad rendering is right to about 1–2 dB on the
pads it can reproduce; the residual is dominated by content that cannot be
reproduced offline (élastique on five pads) and by a gate whose only justification
is the metric. No change was shipped: the one fix that improves fidelity makes the
number worse, and the one that improves the number makes the audio worse. That
trade-off is now documented rather than hidden.

---

## 78. Stem-by-stem against the firmware exports

Every dry stem was compared with its own firmware capture, with the tap's unknown
gain anchored on the median so the anchor cannot bias the winner.

| pad | warp | str % | shipped (gated) | gate off | verdict |
| --- | --- | --- | --- | --- | --- |
| 10 | yes | 100 | **−20.7 dB** | +11.0 | worst offender |
| 14 | yes | 76 | +10.2 | +10.3 | too loud |
| 27 | yes | 104 | −8.6 | +1.4 | too quiet |
| 1 | no | 100 | +7.8 | +7.8 | too loud |
| 20 | yes | 104 | −5.3 | +1.0 | too quiet |
| 2 | no | 100 | +5.5 | +5.5 | too loud |
| 13 | yes | 101 | −3.3 | −0.7 | slightly quiet |
| 17 | no | 100 | −0.4 | −0.4 | ok |
| 0 | no | 100 | +0.4 | +0.4 | ok |
| 9 | no | 100 | +1.1 | +1.1 | ok |

**[PROVED] The culprit is pad 10**, and behind it two independent errors that the
gate mixes together:

1. **the gate** truncates a stretched pad to the note length, so pads 10/13/20/27
   are 3–21 dB quieter than the device. On pad 10 that is 21 dB of missing audio.
2. **the warp level** is up to +11 dB too loud once the gate is removed (pad 10,
   pad 14), so the gate is partly compensating a level error, not a musical one.

Because the two errors point opposite ways on different pads, no single setting
fixes both — and correcting each pad to its measured level makes the *mixdown*
worse (0.18112 → 0.19370). That is the key structural finding: **the shipped 0.1811
is partly balanced by compensating level errors**, so it cannot be improved by
fixing one of them in isolation.

**[DISPROVED] Monophony as the cause of the loud percussive pads.** The hat's notes
are 22050 samples apart against a 7772-frame sample, so nothing overlaps and the
monophonic flag changes nothing at all (+0.00 dB on pads 0/1/2; only pad 10 moves,
−6.02 dB).

**[MEASURED] The bus only touches the loud pads:** wet − dry is −3.2 dB on the kick
and 0.0 dB on every quiet pad, so the bus is not what makes pads 1/2/10/14 loud.

**Remaining level error of the short percussive pads** (1, 2) is consistent with a
pad-volume mapping steeper than linear: at v = 0.39 an exponent near 1.5 explains
their 4–5 dB, but the same exponent does not fit the pads at v ≈ 0.59, so the
mapping is not a clean power law either.

---

## 79. What was changed

Two defects were fixed and one trade-off was made switchable. Nothing about the
shipped default changed, because every fidelity-correct alternative scores worse
against the mixdown and I will not trade one unexplained number for another.

**[FIXED] `audit.py` now reports unmodelled inserts.** `Outputs 1/2` carries
`Pad-Controlled FXs`, `Flex Beat Player` and `Color Compressor`; only the last is
modelled. The audit was silent about all three, so a project that actually used
Flex Beat would have produced a confidently wrong conversion. The reference
project now warns:

```
warning: a track or submix carries an enabled insert other than the Color
Compressor (Pad-Controlled FXs, Flex Beat, LP Filter, ...), which this renderer
does not model (not modelled by this renderer)
```

**[FIXED] `--fidelity` exists, and the shadowing bug that hid it.** `convert`
built `RenderParams(fidelity=...)` and then immediately overwrote `params` with a
fresh `RenderParams(latency=...)`, so any flag passed that way was silently
dropped. `RenderParams.fidelity` now disables the stretched-pad gate, which is
what §78 showed costs pad 10 **21 dB** of level:

| mode | residual | pad 10 level error |
| --- | --- | --- |
| shipped | 0.18112 | −20.7 dB |
| `--fidelity` | 0.20127 | +9.8 dB |

So the trade-off is explicit and measurable rather than hidden in a default: the
gate buys 1.7 dB of residual and costs 30 dB of accuracy on one pad.

**[MEASURED] Why no setting wins.** A 3×2 grid of pad-volume exponent against
gate:

| volume exp | gate | residual | corr |
| --- | --- | --- | --- |
| 1.0 | warped | **0.18112** | **0.9422** |
| 1.5 | warped | 0.18445 | 0.9435 |
| 2.0 | warped | 0.19364 | 0.9422 |
| 1.0 | none | 0.20127 | 0.9270 |
| 1.5 | none | 0.19493 | 0.9345 |
| 2.0 | none | 0.19955 | 0.9361 |

A squared pad-volume law is what the per-pad measurements imply (fit R² = 0.73,
implied exponent 2.1) and it *is* the best no-gate configuration, but it cannot
beat the gated default on either metric — because the gate is compensating for
the phase of the stretched pads, not for their level.

---

## 80. The per-pad filters: the enum was read from a UI label list

**[VALIDATED — the bug]** `filter_types` mapped seven values that were taken from a
short UI label list at `0x45a2828` (`Classic, LPF2, LPF4, HPF2, HPF4, BPF2, BPF4`).
That list is not ordered like the enum. The real table is at **`0x45bb4b0`** — 29
entries, full name plus short name — and the values are the immediates in the name
lookup at `0xbb1700` (`mov w2, N` next to each pair):

```
1 Lowpass 1 Pole    5 Lowpass 8 Pole    9 Highpass 6 Pole   13 Bandpass 6 Pole  17 Bandstop 6 Pole   21 Bandboost 6 Pole  25 Model3
2 Lowpass 2 Pole    6 Highpass 1 Pole   10 Highpass 8 Pole  14 Bandpass 8 Pole  18 Bandstop 8 Pole  22 Bandboost 8 Pole  26 Vocal1
3 Lowpass 4 Pole    7 Highpass 2 Pole   11 Bandpass 2 Pole  15 Bandstop 2 Pole  19 Bandboost 2 Pole 23 Model1            27 Vocal2
4 Lowpass 6 Pole    8 Highpass 4 Pole   12 Bandpass 4 Pole  16 Bandstop 4 Pole  20 Bandboost 4 Pole 24 Model2            28 Vocal3
                                                                                                     29 MPC3000 LPF
```

So **`filterType 29` is `MPC3000 LPF`**, an emulation — we rendered it as a generic
4-pole low-pass. Pads 16 and 18 use it; pad 16 is the one where it bites (cutoff
0.535, resonance 0.197 — pad 18 has the filter wide open).

**[VALIDATED] 29 behaves as a 2-pole low-pass.** Two independent measurements:

| treatment of 29 | pad-16 capture, band spread | mixdown residual |
| --- | --- | --- |
| unmodelled (bypass) | 10.61 dB | 0.19675 |
| **2-pole LP** | **5.67 dB** | **0.17839** |
| 4-pole LP (old) | 7.70 dB | 0.18112 |

Both metrics pick the same answer, so 29 is now modelled as a 2-pole low-pass and
the residual drops from 0.18112 to **0.17839**.

**[VALIDATED — second bug] Pole counts above 4 were no-ops.** `apply_filter` did
`np.vstack([sos, sos])` once when `poles >= 4`, so 4-, 6- and 8-pole filters all
produced the identical 4-pole response. A project using `Lowpass 8 Pole` would have
been rendered with a 4-pole slope and no warning. The cascade is now `poles // 2`
sections, with a genuine first-order section for 1-pole types.

**[VALIDATED — third bug] An unknown `filterType` silently became a lowpass.** The
fallback was `kind, poles = "lp", 2`, so a `Vocal` or `Model` pad — or any value
this build has never seen — was rendered with a low-pass that is not in the project.
The fallback is now "leave the voice alone", and `audit.py` reports it:

```
warning: a pad uses a filter type this renderer does not model (MPC3000 LPF,
Vocal, Model), so that pad is rendered without its filter — pad(s) 16
```

Only engaged filters are reported (a type-29 pad with cutoff 1.0 is transparent
either way, so pads 18/21/22/23 do not spam the list).

**New model residual: 0.17839396674481847** (was 0.1811233335170673).

---

## 81. All 29 filter types: what is exact, what is not

**[VALIDATED] The coefficient maths is now the device's own.** The firmware links
`juce::dsp::IIR::Coefficients<float>` (RTTI, §69) and builds sections at
`0xfcc950`..`0xfccba0` from

```
n  = 1 / tanf(pi * f / fs)
c1 = 1 / (1 + (1/Q) n + n*n)
lp: b = c1 (1, 2, 1)          a1 = 2 c1 (1 - n*n)      a2 = c1 (1 - (1/Q) n + n*n)
```

`filters.juce_coefficients` now computes exactly that, in float32, and
`test_lowpass_coefficients_match_the_decoded_firmware_formula` pins it against the
closed form. Previously the renderer used RBJ cookbook biquads: the same transfer
function for a Butterworth low-pass, but different arithmetic and therefore
different rounding.

**[FIXED] Three response shapes were wrong, not just imprecise:**

| kind | before | after |
| --- | --- | --- |
| highpass | passband **46 dB down** (`b1` negated without rescaling) | −3 dB at cutoff, 0 dB at Nyquist |
| bandstop | +37 dB gain (low-pass normalisation reused) | −61 dB null at cutoff, unity away |
| bandboost | +19 dB DC offset | peak at cutoff, unity away |

`test_every_filter_kind_has_the_response_its_name_promises` checks each name
against its own definition.

**Coverage, honestly stated:**

| types | state |
| --- | --- |
| 1–22 (LP/HP/BP/BS/BB, 1–8 poles) | modelled. LP/HP/BP use the firmware's forms; BS/BB use standard RBJ numerators because the firmware's own forms for those two were not decoded |
| 23–25 `Model1/2/3` | **not modelled** — reported by the audit, rendered dry |
| 26–28 `Vocal1/2/3` | **not modelled** — reported, rendered dry |
| 29 `MPC3000 LPF` | modelled as a 2-pole low-pass (§80), which is what the measurements support; the emulation's resonance character is not reproduced |

So "byte exact" holds for the biquad arithmetic of the 22 standard types, in
float32, on the device's pole polynomial. It does not hold for the six character
filters (separate DSP inside the firmware) and it does not hold for the *multi-pole
cascade*: JUCE cascades sections with per-section Q for a Butterworth alignment,
while we cascade identical sections. The next step for those is decompiling
`FilterS8::generateFrequencyResponseCurve`'s per-type bodies — the strings
`1-Pole multi`, `2-pole SVF multi` and `4-pole SVF multi` suggest the multi-pole
families are state-variable filters, not cascaded biquads.

**Residual: 0.17839408888999467** (unchanged from §80 to seven digits, as it should
be — the low-pass transfer function did not change, only its arithmetic).

---

## 82. Decompilation round: the enum is authoritative, the backend is not a biquad

**[VALIDATED] The filter enum is fully known and applied.** Table at `0x45bb4b0`
(29 names, full + short), values from the immediates in the name lookup at
`0xbb1700`. `filters.FILTER_ENUM` now carries all of them, `render.filter_types` is
derived from it, and unknown types are reported instead of being rendered as a
low-pass (§80/§81).

**[CORRECTED] The tanf-based coefficient builders are not the audio path.**
FINDINGS §73 decoded `makeLowPass`-style builders at `0xfcc950`..`0xfccba0` from
their `bl sym.imp.tanf` call sites and I concluded that "every JUCE IIR coefficient
is built through `tanf`". That conclusion holds for those functions, but they are
**not what runs when a pad plays**, and the evidence is now direct:

* an `LD_PRELOAD` interposer on `tanf` was deployed into the running firmware
  (systemd drop-in in `/run/systemd/system/ac50.service.d/`), the library is mapped
  into the process (`/proc/<pid>/maps` shows it), and **it logs nothing** — not at
  project load, not during playback;
* the same interposer, on a test binary in the same guest, logs correctly
  (`hz=982.623` for `tanf(0.07)`, i.e. `0.07*44100/pi`);
* an interposer on `malloc` deployed the same way **does** fire inside `MPC Sample`,
  so symbol interposition works in that process.

So the app resolves its filter maths without the dynamic `tanf`. Combined with the
firmware's own strings — `1-Pole multi`, `2-pole SVF multi`, `4-pole SVF multi` —
the audio filter is very likely a **state-variable filter**, and the tanf builders
serve the frequency-response curve (`generateFrequencyResponseCurve`, whose lambda
at `0xfe1200` calls the Q-aware builder `0xfcca10` and evaluates it in a loop).

**Consequence for the model.** Our biquads are a good approximation of the *shape*
(the coefficients match the device's published form and the measured slopes), but
they are not the device's topology, which is why filter corrections move the
residual only slightly: the remaining filter error is in the SVF's resonance
behaviour and its per-sample state, not in the biquad coefficients.

**What that means for "byte exact".** Byte-exact audio-path filters require the SVF
coefficients and update equations, which live in the audio-side filter class, not in
the builders above. Locating that class is the next decompilation step; the tanf
route is a dead end for it and should not be re-attempted.

**[RETRACTED] The per-pad Q fit in the previous round was void.** It reported
identical band spreads for every Q value, which I read as "Q does not matter".
`filter_q_overrides` does work (`pad 17` renders 0.0139 → 0.0033 rms for Q 0.3 → 4.0
with `filter_q_base`, and the override path agrees), so that fit script was wrong,
not the code. Q remains an open question rather than a settled one.

---

## 83. The drum-pad filter class, and its coefficient polynomial

**[VALIDATED] The pad filter is a two-pole class with separate low/high kernels,**
found through RTTI rather than by guessing at code:

```
0x4ad9590  20DrumPadLowpassFilterILi2EE      type_info 0x6ea8928   vtable 0x6db1200
0x4ad95b0  21DrumPadHighpassFilterILi2EE     type_info 0x6ea8940   vtable 0x6db1268
```

Both vtables share the base entries and differ at exactly two slots, so the pair
`0x247c450` (low-pass) / `0x247c570` (high-pass) is the type-specific coefficient
routine. That is consistent with §80's measurement: the MPC3000 LPF (type 29)
behaves as a two-pole low-pass.

**[VALIDATED] The coefficient routine does not call `tanf`.** It takes the
normalised cutoff in `s0`, stores it at `[this+0x14]`, loads the resonance from
`[this+0x28]`, and evaluates a polynomial in `1 - cutoff`:

```
x  = 1 - cutoff                    ; fsub s1, 1.0, s8
x2 = x * x
a  = x2 * 19960.0 + 40.0           ; 0x469bf000, 0x42200000
a  = a * resonance                 ; [this+0x28]
b  = a * 1.448  + (-3.482)         ; 0x3fb95810, 0xc05ed917
c  = b * a      + 2.897            ; 0x40396873
coeff = c * a   -> [this+0x34]
```

This is the missing link between the two halves of the story: the `tanf` builders
at `0xfcc950`.. are the **response-curve** path (§82 proved the audio thread never
calls the dynamic `tanf`), and this polynomial is what actually shapes a pad's
filter when it sounds.

**[VALIDATED] The processing kernel sits nearby and loads its coefficients from
the same object:** at `0x247b01c` it does `ldp s5, s6, [x0, 0x34]` (two coefficients
written by the routine above) and `ldr s7, [x0, 0x28]` (the resonance), followed by
`fmadd` chains — the per-sample difference equations.

**Next step, precisely scoped:** decode the difference equations in
`0x247b000`–`0x247b600` and reimplement them, then drive the coefficient polynomial
instead of a biquad. That would make the pad filter the device's topology rather
than an approximation of its response. The work is bounded: one coefficient routine,
one kernel, two types (low/high), and the constants above.

---

## 84. The drum-pad filter class decompiled (Kuna), and one hypothesis killed by it

Kuna 1.130 decompiled the class found by RTTI. All addresses are firmware-relative.

**[VALIDATED] Coeff routine.** `DrumPadLowpassFilter<2>` (`type_info 0x6ea8928`,
vtable `0x6db1200`) has its coefficient routine at `0x247c450`; the high-pass twin
`DrumPadHighpassFilter<2>` (`0x6ea8940`, vtable `0x6db1268`) at `0x247c570`:

```c
// low-pass                                  // high-pass
*(f + 0x14) = cutoff;                       *(f + 0x14) = cutoff;
v = ((1-cutoff)^2 * 19960 + 40) * f[0x28];  v = (cutoff^2 * 19960 + 40) * f[0x28];
f[0x34] = (v * (v * 1.448 - 3.482) + 2.897) * v;
                                            f[0x34] = 1.0 - v*(v*(v*1.448-3.482)+2.897);
```

i.e. `k = u (1.448u^2 - 3.482u + 2.897)` for the low-pass and `k = 1 - k` for the
high-pass, with `u = (x^2*19960 + 40) * f[0x28]`.

**[VALIDATED] Smoothing.** `f[0x18]`/`f[0x1c]`/`f[0x20]` hold a ramp: start, end and
`(end - start) / (int)(a0 * 25 * 1e-3)` — the coefficient is interpolated over that
many samples instead of jumping, which is why a cutoff change never clicks.

**[VALIDATED] Kernel.** `0x247b000` is a pair of one-pole sections updated per
sample and per channel:

```c
y1[i] += k * (target1 * f[0x2c] - y1[i]);
y2[i] += k * (target2 * f[0x2c] - y2[i]);
```

with clamping at `f[0x18] == f[0x1c]` while the ramp finishes. `f[0x2c]` is set by
`0x2479ed0` as `exp(-2*pi * 1000 / (25 * a0))` (the constant at `0x4669818` is
exactly `-2*pi`), and `f[0x28] = 1/a0` there. `0x2478030` simply returns `25.0`.

**[DISPROVED] "The cutoff knob is quadratic in Hz."** The `19960 + 40` in the
coefficient routine reads exactly like 40 Hz … 19 960 Hz, so I hypothesised the
device maps `cutoff -> (1-cutoff)^2 * 19960 + 40` instead of our logarithmic
`20 * 1000^cutoff`. Tested against every captured pad whose filter is engaged, by
band-shape spread (lower is better):

| pad | cutoff | log map | quadratic map | winner |
| --- | --- | --- | --- | --- |
| 13 | 0.3307 | 10.56 dB | 15.80 dB | log |
| 16 | 0.5354 | 5.67 dB | 9.66 dB | log |
| 17 | 0.4567 | 6.88 dB | 19.84 dB | log |
| 20 | 0.6457 | 4.08 dB | 6.05 dB | log |
| 27 | 0.6457 | 3.41 dB | 3.36 dB | tie |

The quadratic law loses on four of five pads and badly on pad 17 (19.8 dB vs
6.9 dB). So that routine's `a0` is **not** the normalised pad cutoff — the
`DrumPad*Filter` classes are used by something else (drum *synth* voices, not the
sample-pad filter), and the pad filter keeps the logarithmic mapping.

**[VALIDATED — new] The logarithmic cutoff mapping is confirmed against device
audio.** This is the first time our `20 * 1000^cutoff` law has been tested against
captured firmware output rather than fitted to the mixdown, and it wins on every
engaged pad. That is a stronger statement than §73 could make.

**Consequence.** The filter-model gap is not the cutoff law. It is the topology:
the device's pad filter is a resonant two-one-pole cascade (§83's kernel shape),
and ours is a biquad cascade. Fixing that needs the *pad* class's kernel, which is
a different class from the `DrumPad*Filter` pair decompiled here.

---

## 85. The pad filter decompiled: a state-variable filter, now the default

Kuna on `FilterBlock` (`type_info 0x6ec0e90`, primary vtable `0x6dbb248`) reached
the per-sample processor at **`0x274e720`**, and it settles the question §83 left
open.

**[VALIDATED] The device's pad filter is a Chamberlin state-variable filter.** Per
channel, per sample:

```c
low  += f * band;
high  = x - low - qd * band;
band += f * high;
out   = tap;            // which state is emitted depends on FILTER_TYPE
```

`f` and `qd` are ramped, not jumped: the processor interpolates
`start + (current - start) * frac` (with a `+1e-18 - 1e-18` flush to keep the ramp
from sticking), and a `std::function` callback at `[obj+0x138]` installs the output
pointer, i.e. **every filter type is the same SVF with a different tap.** That is
why modelling types as separate biquads could never be exact.

**[IMPLEMENTED] `filters.svf_states`** runs exactly that recurrence in float32 with
taps `lp = low`, `hp = high`, `bp = band`, `bs = low + high`, `bb = low + high +
2·band`; `f = 2 sin(pi fc / sr)` and `qd = 1/Q`. `RenderParams.filter_topology`
selects it, and **`"svf"` is now the default** — the device's topology rather than
an approximation of its response.

**[MEASURED] It wins, narrowly but consistently:**

| pad | type | biquad spread | SVF spread |
| --- | --- | --- | --- |
| 13 | 2 | 10.56 dB | **10.39** |
| 16 | 29 | 5.67 dB | **5.35** |
| 17 | 11 | 6.88 dB | **6.58** |
| 20 | 11 | **4.08 dB** | 4.67 |
| 27 | 11 | 3.41 dB | **3.34** |
| total | | 30.60 dB | **30.33 dB** |

Mixdown: **0.178394 → 0.1783822583503335**, corr 0.9441. A small gain, as expected —
only five pads engage a filter and their cutoffs are mostly high — but it is the
right topology now, so the remaining filter error is in the coefficient mapping
(`f`, `qd` from cutoff/resonance) rather than in the structure.

**[OPEN] The remaining coefficient detail.** `f = 2 sin(pi fc/sr)` and `qd = 1/Q` are
the textbook Chamberlin mappings; the firmware ramps them from fields written by
setters I did not finish decoding (`0x274e3bc`, `0x274e534`, `0x274e6b4`), so the
exact `cutoff -> f` and `resonance -> qd` curves are still approximations. The taps
and the recurrence are exact.

39 tests pass, including `test_svf_matches_the_device_topology`.
