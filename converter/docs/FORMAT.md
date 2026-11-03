# Akai MPC `.xpj` project format

Reverse-engineered from `project1/bapap1.xpj` (device reports product `AC50`,
"MPC Sample", format version `1.3.0.12`, schema `28`) and cross-checked against
the firmware image `MPC Sample 1.3.0` (`usr/bin/MPC Sample`, 114 MB stripped
ARM64). Claims marked **[FW]** are confirmed in the firmware's own deserializer;
**[VALIDATED]** means confirmed by a measurement on this project.

---

## 1. Container

```
gzip stream
  └─ ACVS\n
     1.3.0.12\n                                  format version
     SerialisableProjectData\n                  payload kind
     json\n                                     payload encoding
     Linux\n                                    writer platform
     {...}                                      the JSON document
```

* The gzip header carries no filename (`flags = 0`), so `gzip.decompress` alone
  is enough. Decompressed size: 3,823,098 bytes.
* The JSON document has exactly one top-level key, `data`.
* `data.version` = 28, `data.originalCreatorProductIdentifier` =
  `data.lastSavedProductIdentifier` = `"AC50"`.
* **[FW]** `SerialisableProjectData` is read at `0x1456418`; the payload is
  parsed with **RapidJSON** and walked by **cereal-style named fields** — each
  field name is loaded into the parser context, then dispatched to a typed
  reader.

---

## 2. Serialisation conventions **[FW]**

Four conventions explain most of the file's shape:

| convention | meaning |
| --- | --- |
| `{ "value0": X }` | a scalar wrapped in a struct (older schema fields are absent) |
| `[ {"key": N, "value": V}, … ]` | a *map* stored as key/value pairs — **order is not significant** |
| `"EnumCerealisationWrapper(name)": "Member"` | an enum stored under a key that names the member |
| `modifierValue<N>` / `modifierActiveState<N>` | fixed 16-element arrays; names are **built at runtime** (`…Value%d`), confirmed by a single format string in the binary |

Named-field readers do **no transformation**. The number reader at `0x1457fc0`
converts int/uint/double to `float` (`scvtf`/`ucvtf` then `fcvt s0, d0`) and
stores it verbatim. Everything normalised to 0..1 was normalised by the
**writer**; the parser never denormalises.

---

## 3. Top-level `data` object

The document has 66 keys. Those that affect audio:

| key | type | meaning |
| --- | --- | --- |
| `version` | int | schema version, 28 |
| `key` | str | project key (e.g. `"C Major"`) — not audio |
| `mixer` | obj | global mixer (§15) |
| `emulation` | int | vintage emulation type; **0 = off** here |
| `masterTempoEnabled` / `masterTempo` | bool / float | master tempo |
| `samples` | list[28] | sample pool (§16) |
| `tracks` | list[4] | program tracks (§5) |
| `sequences` | list[5] | sequences (§4) |
| `songs` | list[32] | arrangements (§14) |
| `modifierPlaybackProperties` | obj | the 16 modifier slots, named (§13.3) |
| `quantiser` | obj | global quantise settings |
| `scene`, `qlink*Assignments*`, `assignable*Assignments*`, `padPerformSettings`, `midiLearnSettings`, `midiSendDestinations`, `parameterSnapshotterData`, `clipPlayerData`, `locators`, `tuiMainMode*`, `mpcClipMatrixView`, `horizontalClipMatrixView`, `arpeggiatorProperties`, `stepSequencerBehaviour`, `mpcControlSurfaceBehaviour`, `midiNoteFilterPipe`, `xyfxResponder`, `padQuadrantEnablement`, `currentSequence`, `currentTrackIndex`, `currentClipRow`, `engineMode`, `trackMutePerSequence`, `currentMIDITriggeredTrackMuteType` | obj/str/bool/int | UI, control-surface and assignment state — no effect on audio |
| `info` | obj | `title`, `artist`, `originalArtist`, `album`, `year`, `genre`, `copyright`, `notes` |

---

## 4. Sequences, tempo and the pulse grid

```jsonc
"sequences": [ { "key": 0, "value": { … } }, … ]
```

**[VALIDATED] the list is not in index order — index by `key`, never by
position.** (Reading `sequences[0]` yields a different sequence and therefore the
wrong BPM.)

Sequence `value` keys:

| key | type | meaning |
| --- | --- | --- |
| `version`, `name` | int, str | |
| `bpm` | float | tempo, 110.0 here |
| `lengthBars`, `lengthPulses` | int | length |
| `loopStartBar`, `loopEndBar`, `loopStartPulses`, `loopEndPulses`, `loop` | | loop region |
| `tempoEnable`, `transposition`, `smpteStart`, `timeSignatureTrack`, `autoSelectTrackIndex` | | |
| `trackClipMaps` | list[track][clip] | clip rows, one per track |
| `seqEventList`, `locators` | obj | sequence-level events |

**Grid: 960 pulses per quarter note** (Akai standard).

```
seconds = pulses / 960 * 60 / bpm
```

At 110 BPM one pulse = 0.568 ms; a 4/4 bar is 3840 pulses = 2.1818 s.

---

## 5. Tracks

```jsonc
"tracks": [ {"key":0,"value":{"program": …}}, … ]   // 4 entries
```

* `tracks[0].program` is the drum program (§6).
* `tracks[1..3]` are mixer strips and carry no notes.
* Program scalars: `version`, `name`, `type`, `transpose` (**0 here**),
  `midiKillGroup`, `chainID`, `xfaderRoute`; `mixable` (§15);
  `drum` with `pitch`, `coarseTune`, `fineTune`, `monophonic`, `poliphony`,
  `portamentoTime`, `portamentoLegato`, `driftSpeed`, `portamentoQuantised`,
  `monoRetrigger`, `drumVersion`.

---

## 6. Drum program

```jsonc
"drum": { "instruments": [ …128 pads… ], "padNoteMap": {…}, … }
```

### Pad → note map

`padNoteMap.noteForPad` is an object of `value0 … value127`; pad *n* answers to
note `36 + n`. This matches the `note` field of the events.

### `drum.instruments[]` — one entry per pad (128 entries, index = pad number)

| key | type | meaning |
| --- | --- | --- |
| `version` | int | |
| `coarseTune`, `fineTune` | int | pad tuning |
| `monophonic` | bool | pad is monophonic when true |
| `polyphony` | int | 1 = mono |
| `lowNote`, `highNote` | int | key range |
| `ignoreBaseNote` | bool | |
| `zonePlayTime` | int | 1 on every pad here |
| `whichMuteGroup` | int | **mute group**; 0 = none. Pads in the same group cut each other |
| `muteTargets`, `simultPlayTargets` | obj | |
| `synthSection` | obj | envelopes, filter, LFO (§7) |
| `triggerMode` | int | 0 on every pad here |
| `tempo`, `bpmLock`, `warpEnable`, `stretchPercentage`, `userSelectableWarpPoolIndex` | | warp (§11) |
| `layersv` | list[8] | sample layers (§8) |
| `editAllLayers`, `articulationUseXY`, `articulations` | | |
| `chopProperties` | obj | chop mode (§12) |
| `mixable` | obj | level, pan, mute, solo, route, sends, inserts (§15) |
| `padEffects` | obj | `padEffectsData` — every `fxValue` is 0 here |
| `noteCounters` | obj | |
| `modLinks` | list[32] | modulation matrix: `link.source`, `link.target`, `link.min`, `link.max`, `link.amount`, `link.viaSource`, `link.viaAmount`, `link.shaper`, `link.shaperAmount`, `link.bipolar` |
| `midiOutChannel`, `midiOutNote`, `midiOutBehaviour` | | |
| `loopLock` | bool | |

---

## 7. `synthSection`

| key | type | notes |
| --- | --- | --- |
| `version` | int | |
| `ampEnvelope`, `pitchEnvelope`, `auxEnvelope` | obj | §9 |
| `pitchEnvelopeAmount` | float | 0.5 on every pad — inert, envelope `Type` is 0 |
| `auxEnvelopeAmount` | float | 1.0 here |
| `filterData` | obj | **two slots**, `value0` and `value1` (§10) |
| `filterSerialRouting` | bool | false |
| `filterBlend` | float | 0.5 |
| `filterEnvelope` | obj | filter envelope; **`filterEnvelopeAmount` is 0.0 in this project** |
| `lfoData` | obj | `lfoPitch`, `lfoAmpLevel`, `lfoPan`, `lfoFilterCutOff`, `lfoRate`, `lfoLevel`, waveform — all destinations 0 here |
| `velocitySensitivity` | float | 1.0 on every pad |
| `velocityToPitch`, `velocityToPan`, `velocityToStart` | float | 0 here |
| `attackRandom`, `decayRandom`, `driftSpeed`, `rampData`, `randomisationScale` | | 0 / 1.0 |

---

## 8. `layersv[]` — 8 sample layers per pad

Only layer 0 carries a sample in this kit, and `velocityStart`/`velocityEnd` are
0/127 everywhere, so there is no velocity switching.

| key | type | meaning |
| --- | --- | --- |
| `active` | bool | |
| `sampleName`, `sampleFile` | str | `sampleFile` is the basename inside the `_[ProjectData]` folder |
| `volume` | obj | `gainCoefficient`, `controlValue`, `law`. **`controlValue` is the gain**; `law` only changes how the UI maps a knob (and `controlValue` is 1.0 on every layer here, so `law` is inert) |
| `pan` | float | 0.5 = centre |
| `pitch`, `coarseTune`, `fineTune` | float/int/int | `pitch` is the effective tuning in semitones and already includes the fine part. MPCTK measured the device limits as coarse −24…+24 semitones and fine −90…+90 cents, and that setting `coarseTune` also rewrites `pitch` |
| `velocityStart`, `velocityEnd` | int | 0…127 |
| `sampleStart`, `sampleEnd` | int | sample-level trim (0 here) |
| `offset` | int | trigger offset in frames (0 here) |
| `loop`, `loopStart`, `loopEnd`, `loopMode`, `loopFineTune`, `loopCrossfadeLength` | | loop settings (`loop: false` here) |
| `layerLoopModeOverridesSliceLoopMode` | bool | |
| `mute` | bool | |
| `rootNote`, `keyTrackEnable` | int/bool | keytracking off |
| `sliceIndex` | int | **129 = "use this layer's own `sliceInfo`"**, see below |
| `direction` | int | **0 = forward, 1 = reversed** |
| `sliceInfo` | obj | `Start`, `End`, `LoopStart`, `LoopMode`, `PulsePosition`, `LoopCrossfadeLength`, `LoopCrossfadeType`, `TailLength`, `TailLoopPosition` |
| `pitchRandom`, `VolumeRandom`, `PanRandom`, `OffsetRandom` | float | 0 here |

**The playable region is `sliceInfo.Start … sliceInfo.End` (end exclusive).**
Several pads are trimmed well short of the file — pad 13: 14,319 of 62,259
frames; pad 18: 14,597 of 24,062; pad 27: 8,967 of 22,993. Extending any of them
to the whole file makes the render measurably worse, so the trim is applied
correctly.

### `sliceIndex` is a region selector, not a chop number

All 28 populated layers here carry `sliceIndex: 129`, and MPCTK — a third-party
writer whose output was loaded and played on hardware — writes 129 for a
full-file region too. So 129 means *"the region is this layer's own
`sliceInfo`"*, and it must **not** be fed to a slice lookup (129 would index
past the end of any `slic` chunk). Chopping is instead selected per note by the
Sample Slice modifier (slot 15, §"modifiers"), which is what `Pad.slice_index`
exposes. `128` also occurs on the device with unknown semantics; any other
value plausibly names a chop region we do not resolve, so `convert` warns.

### `sliceInfo.End` and the one-frame ambiguity

For every untrimmed sample in this project the device stores `End = n − 1`, so
the field is *either* an inclusive last frame index *or* an exclusive bound
that drops the final frame; the file alone cannot tell you which. Rendering it
inclusive against the device mixdown measures slightly **worse**
(re-measured on the current model: 0.18562 vs 0.18541), so the converter slices
exclusive (`zone[Start:End]`) and `tests/test_project.py` pins both the `n − 1`
convention and that decision. The audible difference is one frame per pad.

---

## 9. Envelopes

All three envelopes share one schema:

```
version, Attack, VelocityToAttack, Decay, Sustain, Release, Hold,
DecayFromEnd, AD, OneShot, AttackPhase, DecayPhase, DecayStart, Type,
Delay, AttackCurve, DecayCurve, ReleaseCurve, tempoSync, Looped, TimeScaling
```

Every stage value is a `{ "value0": x }` wrapper holding a **normalised 0..1**
value. `DecayFromEnd` selects whether the decay runs to the end of the sample;
`AD` / `OneShot` select one-shot behaviour; `*Curve` defaults to 0.375.

**[VALIDATED]** in this project the amp envelopes are effectively flat (`Attack`
is 0, and `Decay` is either 1.0 or 6/127 with `DecayFromEnd`). Sweeping the
envelope scale across its whole plausible range moves the render residual by
less than 0.001, so the envelope is not audible here.

---

## 10. Filter section

```jsonc
"filterData": {
  "value0": { "version":28, "filterKeytrack":0.0, "filterType":2,
              "filterCutoff":0.3307, "filterResonance":0.5827,
              "filterEnvelopeAmount":0.0, "afterTouchToFilter":0.0,
              "filterVelocity":0.0, "filterEnvelopeVelocity":0.0,
              "cutoffRandom":0.0, "resonanceRandom":0.0,
              "outputLevel":1.0, "pressureToFilter":0.0 },
  "value1": { …same schema; filterType 0 on all 128 pads… }
}
```

**[FW]** field order recovered from the deserializer at `0x1476240`:

```
filterKeytrack, filterType, filterCutoff, filterResonance,
filterEnvelopeAmount, afterTouchToFilter, filterVelocity, …
```

`filterType` is read as an **integer**; `filterCutoff` and `filterResonance` as
**raw floats**. No scaling happens in the parser.

**Values in this project.** `filterType` takes only three distinct values — `2`
on 117 pads, `11` on 6, `29` on 5. Only five pads have a non-open filter:

| pad | filterType | cutoff | resonance | warped? |
| --- | --- | --- | --- | --- |
| 13 | 2 | 0.3307 | 0.5827 | yes |
| 16 | 29 | 0.5354 | 0.1969 | no |
| 17 | 11 | 0.4567 | 0.1260 | no |
| 20 | 11 | 0.6457 | 0.1811 | yes |
| 27 | 11 | 0.6457 | 0.1811 | yes |

`filterEnvelopeAmount`, `filterVelocity`, `filterKeytrack`, `pressureToFilter`,
`afterTouchToFilter`, `cutoffRandom` and `resonanceRandom` are **0.0 on every
pad**, so each pad's cutoff is static.

### The `filterType` enum

**[FW] [VALIDATED] Recovered from the firmware.** The Sample-Mode filter combo
builds its list by binding each name to an explicit enum value
(code at `0xb83db8`–`0xb83e70`): each entry stores a literal (`mov w3, N`)
alongside its name pointer. The binding is:

| name | value |
| --- | --- |
| LPF2 | **2** |
| LPF4 | 3 |
| HPF2 | 7 |
| HPF4 | 8 |
| BPF2 | **11** |
| BPF4 | 12 |
| Classic | **29** |

`numFilterTypes` in the project is **30**, so the enum has 30 slots; the seven
above are the ones this product exposes, and values 0, 1, 4, 5, 6, 9, 10 and
13–28 are other (hidden) models.

The authoritative name table is at **`0x45bb4b0`**: 29 entries, each a full name
followed by a short one (`Lowpass 1 Pole` / `Low 1`, `Bandstop 4 Pole` / `BS 4P`,
`MPC3000 LPF`, ...). The enum values are the immediates in the name lookup at
`0xbb1700`, where each pair is emitted with `mov w2, N`:

```
1 Lowpass 1 Pole      6 Highpass 1 Pole     11 Bandpass 2 Pole    16 Bandstop 4 Pole   21 Bandboost 6 Pole   26 Vocal1
2 Lowpass 2 Pole      7 Highpass 2 Pole     12 Bandpass 4 Pole    17 Bandstop 6 Pole   22 Bandboost 8 Pole   27 Vocal2
3 Lowpass 4 Pole      8 Highpass 4 Pole     13 Bandpass 6 Pole    18 Bandstop 8 Pole   23 Model1             28 Vocal3
4 Lowpass 6 Pole      9 Highpass 6 Pole     14 Bandpass 8 Pole    19 Bandboost 2 Pole  24 Model2             29 MPC3000 LPF
5 Lowpass 8 Pole     10 Highpass 8 Pole     15 Bandstop 2 Pole    20 Bandboost 4 Pole  25 Model3
```

An earlier draft of this document read a *seven*-entry short-name list at
`0x45a2828` (`Classic, LPF2, LPF4, HPF2, HPF4, BPF2, BPF4`) as if it were the enum.
That list is a UI label set, it is not ordered like the enum, and it made value 29
look like a generic 4-pole low-pass. The 29-entry table above supersedes it.

Consequences for this project (values read from the pads themselves):

| pads | `filterType` | meaning | our model |
| --- | --- | --- | --- |
| 0, 1, 2, 9, 10, 14 | 2 | Lowpass 2 Pole, cutoff 1.0 → transparent | bypassed, correct |
| 13 | 2 | Lowpass 2 Pole, cutoff 0.3307, resonance 0.583 | 2-pole LP, Q = 1/√2 |
| 17, 20, 27 | 11 | Bandpass 2 Pole | 2-pole BP |
| 16 | 29 | **MPC3000 LPF**, cutoff 0.5354, resonance 0.197 | 2-pole LP (see below) |
| 18 | 29 | MPC3000 LPF, cutoff 1.0 | bypassed either way |

**[VALIDATED — supersedes the biquad model] The pad filter is a Chamberlin
state-variable filter** (§85): `low += f·band; high = x − low − qd·band;
band += f·high`, with the type selecting the tap. `mpc2stems` now renders that
topology by default; the JUCE biquad forms below remain the *response* model and
the `filter_topology="biquad"` option.

**[VALIDATED] The coefficient maths is the device's own** — `filters.juce_coefficients`
reproduces the `0xfcc950` form in float32 (`n = 1/tan(pi f/fs)`, `c1 = 1/(1+(1/Q)n+n²)`,
`lp: b = c1(1,2,1)`, `a1 = 2c1(1−n²)`, `a2 = c1(1−(1/Q)n+n²)`), pinned by a test.
Types 23–28 (`Model1..3`, `Vocal1..3`) are separate DSP inside the firmware and are
reported by `audit.py` rather than approximated.

**[VALIDATED] 29 is the MPC3000 emulation and it behaves as a 2-pole low-pass.**
Two independent measurements agree: against a pad-16 capture the band-shape spread
is 5.7 dB as 2-pole versus 7.7 dB as 4-pole and 10.6 dB unfiltered; against the
reference mixdown the residual is 0.17839 (2-pole), 0.18112 (4-pole) and 0.19675
(no filter at all).

### The audio filter is NOT the tanf builder — correction

**[FW] [VALIDATED, §82]** The tanf builders above produce the *frequency-response
curve* the UI draws. They are not what runs when a pad sounds: an `LD_PRELOAD`
interposer on `tanf`, proven to load and to fire on a test binary and to interpose
`malloc` inside `MPC Sample`, never fires in the app. The firmware's own strings
(`1-Pole multi`, `2-pole SVF multi`, `4-pole SVF multi`) point at a state-variable
filter for the audio path. Treat the biquad coefficients below as the response
model, not as the process.

### The response-curve maths is JUCE, with Q hardcoded to 1/√2

**[FW] [VALIDATED]** The voice filter is **`juce::dsp::IIR::Coefficients<float>`**,
resolved by walking a biquad object's vtable to its RTTI name (not by guessing
from nearby strings, which is the trap this document's earlier draft fell into):

```
object vtable 0x6d78840 -> type_info 0x6de60a8 -> name 0x496d4a8
"N4juce3dsp3IIR12CoefficientsIfEE"  ==  juce::dsp::IIR::Coefficients<float>
```

Five sibling coefficient builders sit at `0xfcc950…0xfccba0`. Each computes
`tan` once and assembles five floats; decoded from the instructions, `0xfcc950`
(materialising `π = 0x40490FDB` and `√2 = 0x3FB504F3`) is exactly JUCE's
`makeLowPass (sampleRate, frequency)`:

```
n  = 1 / tan(π·f/fs)
c1 = 1 / (1 + √2·n + n²)
b  = { c1, 2c1, c1 }
a1 = 2·c1·(1 − n²)
a2 = c1·(1 − √2·n + n²)
```

* `0xfcc950` and `0xfccad0` are 2-argument builders: **Q is the literal 1/√2**.
* `0xfcca10` and `0xfccba0` take a third float and use `invQ = 1/Q`, i.e. JUCE's
  3-argument `makeLowPass/HighPass (sr, f, Q)`.

So the filter *math* is open-source and known; what is still unknown is the
**normalised `filterCutoff`/`filterResonance` → (Hz, Q) mapping**, and it is not
identifiable from a single dense mixdown: with Q pinned, every cutoff scale from
1.0 to 1.75 lands within 0.0009 residual. `mpc2stems` therefore uses the natural
`20·(20000/20)^cutoff` Hz curve (un-fitted) and the firmware's `Q = 1/√2`, which
measures better than the previous invented `Q = 0.5 + 2·resonance`
(FINDINGS §73). `filterResonance` consequently does **not** influence Q.

`FilterS8::generateFrequencyResponseCurve(FILTER_TYPE, f, f, vector<float>&)` is
the remaining lead for the per-type responses (including Classic, value 29):
its coefficient code is named but has no RTTI (the mangled names in this binary
are a name registry, not `type_info` objects), so it has to be reached through
the `FilterS8::FILTER_TYPE` dispatch rather than through a vtable.

**[FW]** the field names used by the deserializer are also the registry names, in
order, at `0x459e180`: `filterType`, `filterCutoff`, `filterResonance`,
`filterEnvelopeAmount`, `afterTouchToFilter`, `filterVelocity`,
`filterEnvelopeVelocity`, `cutoffRandom`, `resonanceRandom`, `outputLevel`,
`pressureToFilter` — an independent confirmation of this section's field list.

---

## 11. Warp (time-stretch)

| key | type | meaning |
| --- | --- | --- |
| `warpEnable` | bool | warp on/off |
| `tempo` | float | tempo detected for the sample (85.0 or 98.75 here) |
| `bpmLock` | bool | "Seq" — lock to sequence tempo |
| `stretchPercentage` | int | 50…200 (%) |
| `userSelectableWarpPoolIndex` | int | algorithm slot (8 for warped pads, 13 otherwise) |

**[VALIDATED]** the rate used by this renderer is

```
rate = project_bpm / sample_tempo      (110/85 = 1.2941, 110/98.75 = 1.1139)
```

measured to 0.0 cents by pitch tracking, and independently confirmed by a
matched filter on the vocal pad — its sample peak lands exactly on the final note
onset only when the 1.294 rate is applied. Competing rules (stretchPercentage,
mixed bpmLock-aware, no warp) all score worse.

**[FW]** the stretcher is **zplane élastique Pro V3** (licence text plus the
`CElastiqueProV3`, `CElastiqueV3Mobile`, `PSOLASynthForElastiqueProV3` RTTI
names). It is pitch-preserving, so the rate changes duration only.

### The voice runs stretch → pitch → gate, in that order

**[VALIDATED]** two ordering facts, both measured against the mixdown (FINDINGS
§71). Neither is visible in the file; both are needed to render a warped pad.

1. **Stretch first, then pitch.** Pads can be warped *and* tuned — pad 13 is
   +480 cents through a 1.2941x stretch, 20/25/26 are −300, 24 is +300 and 27 is
   −900. Doing the pitch resample before the stretch renders 0.2163; stretching
   first renders 0.2093. So élastique works on the raw sample and pitch is
   applied to its output, which is also the sensible signal path.
2. **A warped voice is gated at its own note length.** An ordinary sample rings
   out past the note that triggered it, but a stretched one is cut when the note
   ends, so its tail is *not* heard and must not be rendered.
   `RenderParams.gate = "warped"` reproduces this: 0.2093 → 0.1954. Gating every
   pad instead collapses the mix (0.4301), so the gate really is specific to
   warped pads — that contrast is what identifies it rather than a level tweak.
   The gate length is exactly the note duration; a 5 ms cosine fade is applied
   only to avoid ending a stem on a click.

(The staged figures above are each measured against the model as it stood; the
current model measures **0.1854**, correlation 0.9427/0.9342 — FINDINGS §71–72.)

---

## 12. Chop and slices

```jsonc
"chopProperties": { "version", "chopMode", "chopThreshold", "chopMinSliceTime",
                    "chopRegions", "chopBars", "chopBeats", "chopTimeSignature" }
```

`chopMode` 0 = off, 2 = regions. **The region boundaries are in the WAV file,
not the project**: the RIFF `slic` chunk (`u16 version, u16 count`, then slice
points interleaved with small bookkeeping values). Offsets in range that increase
strictly are the boundaries; a value of a few samples is bookkeeping, and the
final point is usually `frames - 1`.

`Sample 030-Normalized-Trimmed-Normalized.wav` carries 4 slices
(0, 91973, 183946, 275919, 367892); `…Normalized001.wav` carries 16.

A pad whose sample has a `slic` chunk plays the slice selected by the note's
**Sample Slice** modifier (slot 15), not the whole `sliceInfo` region.

---

## 13. Events inside clips

### 13.1 Clip structure

```jsonc
"trackClipMaps": [ [ { "key": N, "value": {
    "eventList": { "events": [ … ] },
    "launchQuantisation", "startPulses", "endPulses", "loopStartPulses",
    "loopEndPulses", "loop", "legato", "launch", "name", "colour",
    "perClipParameterValues" } } ] ]
```

Each event:

```jsonc
{ "version", "time", "type", "channel", "selected", "muted", "invented", "note" }
```

`time` is in **pulses**. `type 3` is a note; other types are automation events.

### 13.2 Note payload

```
version, note, velocity, length, probability, ratchet, articulation,
modifierValue0 … modifierValue15,
modifierActiveState0 … modifierActiveState15,
EnumCerealisationWrapper(selectedModifierType)
```

* `note` — MIDI note; pad = `note - 36`.
* `velocity` — 0..1.
* `length` — pulses.
* `probability` — percent; `ratchet` — repeat count (1 here).

### 13.3 The 16 modifier slots

Authoritative names, in slot order, from `modifierPlaybackProperties` **[FW]**:

| # | name | # | name |
| --- | --- | --- | --- |
| 0 | Tuning (coarse) | 8 | Env Decay |
| 1 | Tuning (fine) | 9 | Env Release |
| 2 | Cutoff | 10 | Modifier 010 |
| 3 | Resonance | 11 | Modifier 011 |
| 4 | Filter Env Amount | 12 | Modifier 012 |
| 5 | Pan | 13 | Modifier 013 |
| 6 | Level | 14 | Sample Layer |
| 7 | Env Attack | 15 | Sample Slice |

**[VALIDATED]** normalisation is 0.5 = neutral, and

```
cents = (value - 0.5) * 24000      (slot 0)
slice = round(value * 127)         (slot 15)
```

In this project only slots **0** (tuning) and **15** (sample slice) are ever
active on any note; the rest are inactive everywhere.

---

## 14. Songs / arrangement

```jsonc
"items": [ { "type": 3, "value": { "type": "Sequence", "seq": 3, "repeat": 1 } }, … ]
"song": { "name", "ignoreTempo", "items" }
```

`type 3` is a sequence reference and `seq` is the sequence **key**. This
project's song is `[seq3, seq4, seq3]`, resolved by key — not by list position.

---

## 15. Mixer, routing, inserts

```
mixer: volume, input, sends, submixes[], outputs[], xFaderBreakpoint, xFaderCurve
```

`mixable` (on programs and strips):

```
version, audioRoute, volume, mute, solo, pan, automationFilter, sends[4], inserts
```

* `audioRoute.destination` — 1 = submix, 2 = output; `channelBitmap` carries the
  channel mask (3 = stereo pair).
* `volume` and `pan` are plain floats: **0.7079458 = −3 dB**. The program, both
  submixes and both outputs all sit at −3 dB in this project.
* `inserts.effects[]` entries carry `plugin.description.name` and a
  `plugin.state` blob. The blob is **plain base64 with `/` written as `.`** and is
  length-prefixed inside the string (`"119.AMjUSA…"` — 119 characters follow).
  Every blob decodes to a payload starting `00 c8 d4 48 0f ff ff` where unset
  parameters are long `0xff` runs: a **bit-packed** parameter stream, not
  byte-aligned floats, and not compressed (no deflate/zlib/lzma/bz2/gzip variant
  decodes it at any offset). Unpacking it needs the firmware's bit widths
  (FINDINGS §72); the alphabet itself is no longer an obstacle.
* `sends` are all 0; `program.transpose` and every strip `transpose` are 0.

Signal path for this project:

```
pads → destination 1 (Submix 1) → destination 2 (Outputs 1/2)
Outputs 1/2 inserts: "Pad-Controlled FXs", "Flex Beat Player", "Color Compressor"
```

Only the compressor is audible; vintage emulation is off (`"emulation": 0`).

The inserts are *plugin state blobs*, not parameters, so the bus stage cannot be
read out of the file. It was recovered by fitting instead: **attack ≈15 ms,
release ≈1 ms, −3 dB, 3:1, 5 ms RMS detector, automatic makeup**, on a plateau
that is flat to 0.0003 across attack 10–25 ms / release 0.5–4 ms. That is a fast
limiter, not the slow 72/119 ms compressor the front-panel knob positions
suggest — the measurement is in FINDINGS §71.

---

## 16. Samples

```jsonc
"samples": [ { "version", "name", "path", "loadImpl", "metadata": {…} }, … ]
```

`path`/`name` refer to the sibling folder `bapap1_[ProjectData]`. All 28 files
are 44.1 kHz — 9 mono, 19 stereo, 24-bit PCM or 32-bit float.

---

## 17. Value encodings, in one place

| quantity | storage | conversion |
| --- | --- | --- |
| pulse | int | `s = pulses / 960 * 60 / bpm` |
| pad | int 0…127 | `note = 36 + pad` |
| velocity | float 0…1 | level `≈ 1.80 · v^0.812` at `velocitySensitivity = 1` |
| envelope stage | float 0…1 | scaled to seconds by the DSP |
| cutoff / resonance | float 0…1 | `fc = f(cutoff)`, `Q = g(resonance)` |
| tuning modifier | float 0…1 | `cents = (v − 0.5) · 24000` |
| slice modifier | float 0…1 | `slice = round(v · 127)` |
| layer pitch | float semitones | resample ratio `2^(pitch/12)` |
| pan | float 0…1 | 0.5 = centre, constant-power law |
| volume / mixable.volume | float 0…1 | linear gain; 0.7079458 = −3 dB |

---

## 18. Firmware provenance

The MPC Sample rootfs (ext4 inside the `AZ0x` `Update.img`, XZ-compressed
partitions) ships `usr/bin/MPC Sample`, a 114 MB stripped ARM64 binary. It
contains the licence line *"This product incorporates elastique Pro V3 by
zplane.development"*, so **sample warping is zplane élastique Pro V3**. Plugin
effects run on the "H3" framework (`H3Part`, `wzParameterType*`).

Every key consumed by `mpc2stems` exists verbatim in the binary's string table,
and `modifierActiveState` / `modifierValue` exist once each as format strings —
confirming the 16-slot arrays are generated rather than stored literally.

### Working with the binary

| fact | value |
| --- | --- |
| path in rootfs | `usr/bin/MPC Sample` |
| size / SHA-256 | 119,243,440 B / `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508` |
| form | stripped AArch64 **PIE**, dynamic symbols only, **C++ RTTI retained**, not in the repo (`analysis/` is ignored) |
| extraction | `7z x firmware/extracted/part2 "usr/bin/MPC Sample"` (`part2` = the ext4 rootfs) |

RTTI is what makes the binary tractable: class names at `type_info` lead to
vtables and then to the exact function pointers. JS-free tooling for this lives in
`tools/re/` (see `docs/MODDING.md` §4.4.1):

```
tools/re/fwxref.py   # adrp/add/ldr scan: every reference to a string or global
tools/re/rtti.py     # RTTI name -> type_info -> vtable
tools/re/calls.py    # every BL call site to a given address
```

Addresses resolved so far, all from this build:

| what | address |
| --- | --- |
| project deserializer (named fields) | `0x1456418`, `0x1476240`, `0x1477fc0` |
| biquad coefficient builders (JUCE IIR) | `0xfcc950`, `0xfcca10`, `0xfccad0`, `0xfccba0` |
| biquad object vtable / type_info / RTTI name | `0x6d78840` / `0x6de60a8` / `0x496d4a8` |
| coefficient constructor (5 floats + vtable) | `0xfc5e60` |
| filter parameter-name registry | `0x459e180` |
| `filterType` combo enum binding | `0xb83db8`–`0xb83e70` |

See `docs/FINDINGS.md` §50–51, §58–59, §72–73 for the evidence trail.
