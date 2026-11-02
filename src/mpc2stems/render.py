"""Audio renderer: turn the project model into sample-accurate per-pad stems.

The renderer is an explicit model of one device voice.  Every assumption is a
field of :class:`RenderParams` so it can be calibrated against a reference
mixdown (see ``tools/validate.py``).

What is modelled
----------------
* note onset on the sequencer grid (960 PPQ, per-sequence BPM) plus a constant
  output latency,
* layer tuning (``coarseTune``/``fineTune`` cents -> playback rate),
* reversed playback (``direction != 0``),
* sample warping (``warpEnable`` / ``stretchPercentage``) as a resample fallback,
* monophonic voice cutting (all pads report ``polyphony: 1``),
* note ring-out past the end of the song,
* velocity -> gain, pad level, layer level, track fader, master fader and pan.
"""

from __future__ import annotations

from dataclasses import dataclass
from functools import lru_cache

import numpy as np

from . import envelope as envelope_mod
from . import filters as pad_filters
from . import warp
from .model import Layer, Pad, Project, PULSES_PER_QUARTER
from .wav import WavData, read_slices, read_wav


@dataclass(frozen=True)
class RenderParams:
    """All tunable assumptions of the voice model."""

    sample_rate: int = 44100
    #: constant device output latency measured from the reference mixdown
    latency: int = 91
    #: velocity -> amplitude exponent, calibrated on the reference mixdown
    velocity_exponent: float = 0.812
    #: global output gain relative to the raw fader product
    output_gain: float = 10.5
    #: stereo pan law
    pan_law: str = "constant_power"  # constant_power | linear
    #: how far past the song end notes may ring out
    tail_seconds: float = 3.0
    #: cut a voice when the same pad is retriggered
    monophonic: bool = True
    #: stop a voice after its note length (gate): "none" | "warped" | "all".
    #: "warped" is what the device does: a stretched pad only sounds for the
    #: note's own length, which is why the stretched tail is not heard.  This is
    #: worth 0.2163 -> 0.1997 on its own (FINDINGS 71).
    gate: str = "warped"
    #: multiplier on the gate length; 1.0 = exactly the note length
    gate_scale: float = 1.0
    #: fit for the device mixdown rather than for per-stem fidelity.  The gate
    #: truncates stretched pads to their note length, which is 3-21 dB quieter than
    #: the device on those pads (FINDINGS 78) but scores better against the mixdown
    #: because their phase cannot be reproduced offline.  ``--fidelity`` turns it
    #: off: the stems then carry the right energy and the model residual rises.
    fidelity: bool = False
    #: cosine fade at the gate, in seconds.  Its effect on the residual is under
    #: 0.001; it is here so a gated stem does not end on a click.
    gate_fade_seconds: float = 0.005
    #: filter after the pitch/warp stage (fixed Hz) instead of before
    filter_after_pitch: bool = False
    #: deterministic RNG seed for probabilistic notes
    seed: int = 0x5EED
    #: honour the amp/pitch/filter envelopes
    honour_envelopes: bool = True
    #: seconds per normalised unit for each envelope stage (0 = not modelled)
    envelope_scales: "envelope_mod.EnvelopeScales" = envelope_mod.EnvelopeScales()
    #: pitch envelope depth in semitones at full modulation
    pitch_envelope_semitones: float = 12.0
    #: LFO depth in semitones at full pitch modulation
    lfo_pitch_semitones: float = 12.0
    #: seconds per normalised unit for the LFO fade-in / delay
    lfo_fadein_seconds: float = 2.0
    #: select layers by velocity, apply velocity->start/pitch
    honour_velocity: bool = True
    #: honour layer loop regions
    honour_loops: bool = True
    #: how long a looping layer keeps repeating before the note cut applies
    loop_max_seconds: float = 8.0
    #: restore the power the phase vocoder loses to incoherent phases.  On this
    #: material that is -3.4 dB of broadband level, so leaving it on is the
    #: physically right choice, but it *raises* the residual: the device's
    #: stretched pads have essentially uncorrelated phase, so the metric prefers
    #: them quiet (0.1849 on / 0.1787 off).  On by default; see FINDINGS 72.
    warp_energy_match: bool = True
    #: stretch first, then pitch-shift.  The device works in this order: the pads
    #: that are both warped and tuned match measurably better this way -- residual
    #: 0.2093 vs 0.2163, and the gain holds in every half and third of the song
    #: (FINDINGS 71).  Only pads 13 (+4 st at 1.294x), 20/25/26 (-3 st) and
    #: 27 (-9 st) are affected.
    warp_before_pitch: bool = True
    #: honour the Sample Slice modifier (modifier 15)
    honour_slices: bool = True
    #: honour pad mute groups (pads in the same group cut each other)
    mute_groups: bool = True
    #: explicit set of pads to gate (overrides ``gate`` when not None)
    gate_pads: frozenset[int] | None = None
    #: play reversed when the layer direction is non-zero
    honour_direction: bool = True
    #: apply time-stretch to warped pads (approximate: phase vocoder)
    honour_warp: bool = True
    #: warp rate rule: "tempo" = project/sample tempo when bpmLock else stretch%
    warp_rule: str = "tempo"  # tempo | tempo_always | stretch
    #: apply the pad's low-pass filter section
    filter_enabled: bool = True
    #: mapping of the stored filterType value to a response family/slope
    #: filterType enum -> (kind, poles), recovered from the firmware (FINDINGS 69)
    #: filter topology: "biquad" (JUCE forms) or "svf" (the device's Chamberlin
    #: state-variable filter, FINDINGS 85).  SVF is the device topology.
    filter_topology: str = "svf"
    filter_types: tuple = tuple(
        (value, (kind, poles))
        for value, (_name, kind, poles) in pad_filters.FILTER_ENUM.items()
        if kind
    )
    #: Per-filter-model calibration: filterType -> (cutoff_scale, q_scale).
    #: LPF2 uses the natural 20 Hz-20 kHz log mapping and a much stronger
    #: resonance law; the other models keep the global fit because their
    #: response is approximated (Classic) rather than modelled. Validated by a
    #: phase-blind magnitude fit on pad 13 (corr 0.889 vs 0.737 for the global
    #: setting) and by ear.
    filter_type_calibration: tuple = ()
    #: per-pad cutoff-scale overrides (empty; kept for experiments)
    filter_cutoff_scale_overrides: tuple = ()
    #: per-pad Q overrides (empty; kept for experiments)
    filter_q_overrides: tuple = ()
    #: Q = filter_q_base + resonance * filter_q_scale.  The device builds its
    #: biquads with juce::dsp::IIR::Coefficients, and the 2-argument form
    #: (makeLowPass(sr, f)) hardcodes Q = 1/sqrt2 -- the constant is a literal in
    #: the coefficient code (FINDINGS 73).  With scale 0 the resonance knob stops
    #: affecting Q, which measures better: sweeping fixed Q has an interior
    #: minimum at 0.7071 (0.6 -> 0.18125, 0.7071 -> 0.18116, 0.9 -> 0.18228).
    filter_q_base: float = 0.70710678
    filter_q_scale: float = 0.0
    #: multiplier on the cutoff frequency mapping.  1.0 is the natural
    #: 20 * (20000/20)**cutoff curve; the measurement cannot distinguish 1.0 from
    #: 1.75 (0.18112 vs 0.18116, flat across the range) so it is not fitted.
    filter_cutoff_scale: float = 1.0
    #: amp envelope: seconds per full 0..127 range for attack and decay
    attack_scale_s: float = 1.2
    decay_scale_s: float = 0.0
    #: apply the track fader and project master fader
    track_gain: bool = True
    master_gain: bool = True


def pan_gains(pan: float, law: str = "constant_power") -> tuple[float, float]:
    """Return (left, right) gains for a pan position in [0, 1] (0.5 == centre)."""
    p = min(max(float(pan), 0.0), 1.0)
    if law == "linear":
        return 1.0 - p, p
    theta = p * 0.5 * np.pi
    return float(np.cos(theta)), float(np.sin(theta))


def pulses_to_seconds(pulses: float, bpm: float) -> float:
    return pulses / (bpm * PULSES_PER_QUARTER) * 60.0


def sequence_layout(project: Project) -> list[tuple[int, int]]:
    """Song items as ``(sequence_index, start_pulse)``."""
    by_index = project.sequence_by_index
    layout: list[tuple[int, int]] = []
    cursor = 0
    for seq_index in project.song:
        layout.append((seq_index, cursor))
        cursor += by_index[seq_index].length_pulses
    return layout


def total_pulses(project: Project) -> int:
    by_index = project.sequence_by_index
    return sum(by_index[s].length_pulses for s in project.song)


@lru_cache(maxsize=256)
def _load(path: str) -> WavData:
    return read_wav(path)


def voice_rate(
    pad: Pad,
    layer: Layer,
    params: RenderParams,
    extra_cents: float = 0.0,
    project_bpm: float = 120.0,
) -> float:
    """Playback rate (input samples per output sample) for one pad layer."""
    return 2.0 ** ((layer.cents + extra_cents) / 1200.0)


def warp_rate(pad: Pad, params: RenderParams, project_bpm: float) -> float:
    """Time-stretch rate for a warped pad, or 1.0 when warp is inactive."""
    if not params.honour_warp or not pad.warp_enable:
        return 1.0
    if params.warp_rule == "stretch":
        return max(1e-6, pad.stretch_percentage / 100.0)
    if params.warp_rule == "tempo_always":
        return warp.stretch_rate(project_bpm, pad.sample_tempo, True, pad.stretch_percentage)
    return warp.stretch_rate(
        project_bpm, pad.sample_tempo, pad.bpm_lock, pad.stretch_percentage
    )


def _resample(voice: np.ndarray, rate: float) -> np.ndarray:
    """Resample ``voice`` by ``rate`` (output samples per input sample)."""
    if rate == 1.0:
        return voice
    n_out = int(np.ceil(voice.shape[0] / rate))
    pos = np.arange(n_out) * rate
    src = np.clip(pos.astype(np.int64), 0, voice.shape[0] - 1)
    frac = (pos - src)[:, None]
    nxt = np.clip(src + 1, 0, voice.shape[0] - 1)
    return voice[src] + (voice[nxt] - voice[src]) * frac


def _filter_stage(zone, pad, params, slot=0, cutoff_octaves=0.0):
    """Apply one filter slot to a voice buffer."""
    if not params.filter_enabled:
        return zone
    data = pad.filter if slot == 0 else pad.filter_slot1
    if not data:
        return zone
    cutoff = float(data.get("filterCutoff", 1.0))
    resonance = float(data.get("filterResonance", 0.0))
    kind_filter = int(data.get("filterType", 0))
    if kind_filter == 0 and slot == 1:
        return zone
    if slot == 0 and cutoff >= 0.999 and resonance <= 0.001:
        return zone
    pad_index = getattr(pad, "index", -1)
    scale = dict(params.filter_cutoff_scale_overrides).get(pad_index, params.filter_cutoff_scale)
    q_scale = params.filter_q_scale
    cal = dict(params.filter_type_calibration).get(kind_filter)
    if cal is not None:
        scale, q_scale = cal
    fc = pad_filters.cutoff_hz(cutoff) * scale
    if cutoff_octaves:
        fc = float(np.clip(fc * (2.0 ** cutoff_octaves), 10.0, params.sample_rate * 0.45))
    q_override = dict(params.filter_q_overrides).get(pad_index)
    if q_override is not None:
        q = q_override
    else:
        q = pad_filters.q_from_resonance(
            resonance, base=params.filter_q_base, scale=q_scale
        )
    table = dict(params.filter_types or ())
    if kind_filter not in table:
        # A character filter (Vocal, Model) or an enum value this build has never
        # seen.  Rendering it as a lowpass would be an invention, so leave the
        # signal alone and let audit.py report it.
        return zone
    kind, poles = table[kind_filter]
    if params.filter_topology == "svf":
        y = pad_filters.svf_states(zone, kind, fc, q, params.sample_rate)
        stages = max(1, int(poles) // 2)
        for _ in range(stages - 1):
            y = pad_filters.svf_states(y, kind, fc, q, params.sample_rate)
        return y
    return pad_filters.apply_filter(zone, kind, poles, fc, q, params.sample_rate)


def _apply_pad_filter(zone, pad, params, cutoff_octaves=0.0):
    """Apply both filter slots, series or blended, to a voice buffer."""
    if not params.filter_enabled:
        return zone
    slot1_active = int((pad.filter_slot1 or {}).get("filterType", 0)) != 0
    if not slot1_active:
        return _filter_stage(zone, pad, params, 0, cutoff_octaves)
    first = _filter_stage(zone, pad, params, 0, cutoff_octaves)
    second = _filter_stage(zone, pad, params, 1, cutoff_octaves)
    if pad.filter_serial:
        return _filter_stage(first, pad, params, 1, cutoff_octaves)
    blend = float(np.clip(pad.filter_blend, 0.0, 1.0))
    return first * (1.0 - blend) + second * blend


def _loop_zone(zone: np.ndarray, layer: Layer, extra_frames: int = 0) -> np.ndarray:
    """Repeat ``loop_start .. loop_end`` past the end of the sample.

    A looping layer has no natural end, so the buffer is extended by
    `extra_frames` worth of repeats; the note length, monophonic cut or mute
    group decides where playback actually stops.
    """
    if not layer.loop or layer.loop_end <= layer.loop_start or extra_frames <= 0:
        return zone
    start = max(0, layer.loop_start)
    end = min(layer.loop_end, zone.shape[0])
    if start >= end:
        return zone
    body = zone[start:end]
    if body.shape[0] == 0:
        return zone
    repeats = int(np.ceil(extra_frames / body.shape[0]))
    return np.concatenate([zone[:end], np.tile(body, (repeats, 1))])


def _resample_curve(voice: np.ndarray, base_rate: float, semitones: np.ndarray | None) -> np.ndarray:
    """Resample with a possibly time-varying playback rate."""
    if semitones is None or voice.shape[0] == 0:
        return _resample(voice, base_rate)
    ratio = base_rate * np.power(2.0, semitones / 12.0)
    ratio = np.clip(ratio, 1e-3, 1e4)
    # positions in the source for each output frame
    pos = np.cumsum(ratio)[:-1]
    pos = pos[pos < voice.shape[0] - 1]
    if pos.size == 0:
        return _resample(voice, base_rate)
    src = pos.astype(np.int64)
    frac = (pos - src)[:, None]
    nxt = np.clip(src + 1, 0, voice.shape[0] - 1)
    return voice[src] + (voice[nxt] - voice[src]) * frac


def build_voice(
    project: Project,
    pad: Pad,
    params: RenderParams,
    extra_cents: float = 0.0,
    project_bpm: float = 120.0,
    slice_index: int | None = None,
    velocity: float = 1.0,
    layer: Layer | None = None,
) -> np.ndarray | None:
    """Materialise one pad layer's (possibly reversed, tuned, warped) sample."""
    if layer is None:
        layer = pad.unmuted_layers[0] if pad.unmuted_layers else None
    if layer is None:
        return None

    wav = _load(str(project.root / layer.sample_file))
    start, end = layer.zone_start, layer.zone_end
    if params.honour_slices and slice_index is not None and slice_index >= 0:
        slices = read_slices(wav)
        if slices:
            idx = min(slice_index, len(slices) - 1)
            start, end = slices[idx]
            start = max(start, layer.zone_start)
            end = min(end, layer.zone_end) if layer.zone_end else end
    zone = wav.samples[start:end]
    if zone.shape[1] == 1:
        zone = np.repeat(zone, 2, axis=1)
    elif zone.shape[1] > 2:
        zone = zone[:, :2]

    if params.honour_velocity and pad.velocity_to_start:
        n = int(round(pad.velocity_to_start * (1.0 - velocity) * zone.shape[0] * 0.5))
        if 0 < n < zone.shape[0]:
            zone = zone[n:]

    if params.honour_direction and layer.direction:
        zone = zone[::-1].copy()
    if layer.offset:
        zone = zone[layer.offset :]

    if params.honour_loops:
        zone = _loop_zone(zone, layer, int(params.loop_max_seconds * params.sample_rate))

    # ---- pitch: layer tuning + pad + keytracking + velocity + envelope ----
    cents = layer.cents + extra_cents
    if layer.key_track:
        cents += (pad.note - layer.root_note) * 100.0
    if params.honour_velocity and pad.velocity_to_pitch:
        cents += pad.velocity_to_pitch * (velocity - 0.5) * 100.0

    semitones = None
    amount = float(pad.pitch_envelope_amount) - 0.5
    if params.honour_envelopes and abs(amount) > 1e-9 and pad.pitch_envelope:
        env = envelope_mod.shape(
            pad.pitch_envelope,
            zone.shape[0],
            params.sample_rate,
            params.envelope_scales,
        )
        semitones = amount * params.pitch_envelope_semitones * env

    # ---- LFO (pitch and amplitude; filter/pan LFOs are static-offsets) ----
    lfo = (pad.lfo or {}).get("value0", {}) if isinstance(pad.lfo, dict) else {}
    lfo_pitch = float(lfo.get("lfoPitch", 0.0)) if isinstance(lfo, dict) else 0.0
    lfo_amp = float(lfo.get("lfoAmpLevel", 0.0)) if isinstance(lfo, dict) else 0.0
    if params.honour_envelopes and (abs(lfo_pitch) > 1e-9 or abs(lfo_amp) > 1e-9):
        rate_hz = 0.1 * (100.0 ** float(lfo.get("lfoRate", 0.5)))
        wave = envelope_mod.lfo_curve(
            zone.shape[0],
            params.sample_rate,
            rate_hz,
            int(lfo.get("lfoWaveformType", 0)),
            fade_in=float(lfo.get("lfoFadein", 0.0)) * params.lfo_fadein_seconds,
            delay=float(lfo.get("lfoDelay", 0.0)) * params.lfo_fadein_seconds,
        )
        if abs(lfo_pitch) > 1e-9:
            modulation = lfo_pitch * params.lfo_pitch_semitones * wave
            semitones = modulation if semitones is None else semitones + modulation
        if abs(lfo_amp) > 1e-9:
            zone = zone * (1.0 + lfo_amp * wave * 0.5)[:, None]

    # ---- filters, with envelope / keytrack / velocity on the cutoff ----
    cutoff_octaves = 0.0
    if params.honour_envelopes and pad.filter_envelope:
        depth = float((pad.filter or {}).get("filterEnvelopeAmount", 0.0))
        if abs(depth) > 1e-9:
            env = envelope_mod.shape(
                pad.filter_envelope,
                zone.shape[0] // 2 + 1,
                params.sample_rate // 2,
                params.envelope_scales,
            )
            cutoff_octaves += (depth - 0.5) * 8.0 * float(env.mean())
    keytrack = float((pad.filter or {}).get("filterKeytrack", 0.0))
    if keytrack:
        cutoff_octaves += keytrack * (pad.note - 60) / 12.0
    fvel = float((pad.filter or {}).get("filterVelocity", 0.0))
    if fvel:
        cutoff_octaves += fvel * (velocity - 0.5) * 4.0

    if params.filter_enabled and not params.filter_after_pitch:
        zone = _apply_pad_filter(zone, pad, params, cutoff_octaves)

    # ---- amplitude envelope ----
    if params.honour_envelopes:
        env = envelope_mod.shape(
            pad.amp_envelope, zone.shape[0], params.sample_rate, params.envelope_scales
        )
        zone = zone * env[:, None]

    rate = warp_rate(pad, params, project_bpm)
    base_rate = 2.0 ** (cents / 1200.0)
    if params.warp_before_pitch:
        if rate != 1.0:
            zone = warp.time_stretch(zone, rate, energy_match=params.warp_energy_match)
        zone = _resample_curve(zone, base_rate, semitones)
    else:
        zone = _resample_curve(zone, base_rate, semitones)
        if rate != 1.0:
            zone = warp.time_stretch(zone, rate, energy_match=params.warp_energy_match)
    if params.filter_after_pitch:
        zone = _apply_pad_filter(zone, pad, params, cutoff_octaves)
    return zone


def note_events(project: Project) -> list[tuple[int, int, float, float, int, int | None, int, int]]:
    """Notes as ``(pulse, pad, velocity, cents, length, slice, probability, ratchet)``."""
    by_index = project.sequence_by_index
    events: list[tuple[int, int, float, float, int, int | None, int, int]] = []
    for seq_index, start_pulse in sequence_layout(project):
        for note in by_index[seq_index].notes:
            events.append(
                (
                    start_pulse + note.time_pulses,
                    note.pad_index,
                    note.velocity,
                    note.tuning_cents,
                    note.length_pulses,
                    note.slice_index,
                    note.probability,
                    note.ratchet,
                )
            )
    events.sort()
    return events


def _layer_for_velocity(pad: Pad, velocity: float) -> Layer | None:
    """Pick the layer whose velocity range contains this note."""
    layers = pad.unmuted_layers
    if not layers:
        return None
    v127 = int(round(max(0.0, min(1.0, velocity)) * 127))
    for layer in layers:
        if layer.velocity_start <= v127 <= layer.velocity_end:
            return layer
    return layers[0]


def _expand_ratchets(events):
    """Turn ratcheted notes into their repeats."""
    out = []
    for pulse, pad_i, vel, cents, length, slice_i, prob, ratchet in events:
        if ratchet > 1 and length > 0:
            step = length / ratchet
            for k in range(ratchet):
                out.append(
                    (pulse + int(round(k * step)), pad_i, vel, cents, int(round(step)), slice_i, prob)
                )
        else:
            out.append((pulse, pad_i, vel, cents, length, slice_i, prob))
    return out


def _render(
    project: Project,
    track: int,
    params: RenderParams,
    pad_subset: set[int] | None,
    apply_gain: bool,
) -> tuple[dict[int, np.ndarray], int]:
    sr = params.sample_rate
    pad_map = project.pads[track]
    track_mix = project.track_mixes[track]

    n_frames = (
        int(round(pulses_to_seconds(total_pulses(project), project.bpm) * sr))
        + int(params.tail_seconds * sr)
        + params.latency
        + 1
    )

    events = _expand_ratchets(note_events(project))

    # mute groups: a note silences every other voice in its group
    group_cut: dict[int, int] = {}
    last_in_group: dict[int, int] = {}
    for i, (pulse_i, pad_i, *_rest) in enumerate(events):
        grp = pad_map[pad_i].mute_group if pad_i in pad_map else 0
        if not grp:
            continue
        if grp in last_in_group:
            group_cut[last_in_group[grp]] = pulse_i
        last_in_group[grp] = i

    # probabilistic notes: deterministic per event so renders are reproducible
    rng = np.random.default_rng(params.seed)
    keep: list[bool] = []
    for _pulse, _pad, _vel, _cents, _len, _slice, prob in events:
        keep.append(prob >= 100 or rng.random() * 100.0 < prob)

    voices: dict[tuple, np.ndarray] = {}
    stems: dict[int, np.ndarray] = {}

    def to_sample(pulse: int) -> int:
        return int(round(pulses_to_seconds(pulse, project.bpm) * sr)) + params.latency

    for index, (pulse, pad_index, velocity, cents, length_pulses, slice_index, _prob) in enumerate(events):
        if not keep[index]:
            continue
        if pad_subset is not None and pad_index not in pad_subset:
            continue
        pad = pad_map.get(pad_index)
        if pad is None or pad.mute:
            continue
        layer = _layer_for_velocity(pad, velocity)
        if layer is None:
            continue

        velocity_dependent = bool(
            params.honour_velocity
            and (pad.velocity_to_pitch or pad.velocity_to_start or len(pad.unmuted_layers) > 1)
        )
        vel_key = round(velocity * 32) if velocity_dependent else 0
        key = (pad_index, round(cents, 4), slice_index, id(layer), vel_key)
        if key not in voices:
            voice = build_voice(
                project, pad, params, cents, project.bpm, slice_index,
                velocity=velocity, layer=layer,
            )
            if voice is None:
                continue
            voices[key] = voice
        voice = voices[key]

        onset = to_sample(pulse)
        end = onset + voice.shape[0]
        if params.monophonic:
            for j in range(index + 1, len(events)):
                if events[j][1] == pad_index:
                    end = min(end, to_sample(events[j][0]))
                    break
        if params.mute_groups and index in group_cut:
            end = min(end, to_sample(group_cut[index]))
        should_gate = (not params.fidelity) and (
            (params.gate_pads is not None and pad_index in params.gate_pads)
            or (
                params.gate_pads is None
                and (params.gate == "all" or (params.gate == "warped" and pad.warp_enable))
            )
        )
        if should_gate:
            gate_len = pulses_to_seconds(length_pulses, project.bpm) * params.gate_scale
            end = min(end, onset + int(round(gate_len * sr)))
        end = min(end, n_frames)
        if end <= onset:
            continue

        seg = voice[: end - onset]
        if should_gate and params.gate_fade_seconds > 0.0:
            n_fade = int(round(params.gate_fade_seconds * sr))
            n_fade = min(n_fade, len(seg) // 2)
            if n_fade > 0:
                seg = seg.copy()
                ramp = 0.5 * (1.0 + np.cos(np.linspace(0.0, np.pi, n_fade)))
                seg[-n_fade:] *= ramp[:, None]
        if apply_gain:
            gain = params.output_gain * (velocity ** params.velocity_exponent)
            gain *= layer.volume * pad.volume
            if params.honour_velocity and pad.velocity_sensitivity != 1.0:
                gain *= velocity ** (params.velocity_exponent * (1.0 / pad.velocity_sensitivity - 1.0))
            if params.track_gain:
                gain *= track_mix.volume
            if params.master_gain:
                gain *= project.master_volume

            pan = pad.pan
            if params.honour_velocity and pad.velocity_to_pan:
                pan = float(np.clip(pan + pad.velocity_to_pan * (velocity - 0.5), 0.0, 1.0))
            lp, rp = pan_gains(pan, params.pan_law)
            ll, rl = pan_gains(layer.pan, params.pan_law)
            lt, rt = pan_gains(track_mix.pan, params.pan_law)

            out = np.empty_like(seg)
            out[:, 0] = seg[:, 0] * (gain * lp * ll * lt)
            out[:, 1] = seg[:, 1] * (gain * rp * rl * rt)
        else:
            out = seg

        if pad_index not in stems:
            stems[pad_index] = np.zeros((n_frames, 2), dtype=np.float64)
        stems[pad_index][onset:end] += out

    return stems, n_frames


def render_pad_stems(
    project: Project,
    track: int,
    params: RenderParams = RenderParams(),
    pad_subset: set[int] | None = None,
) -> tuple[dict[int, np.ndarray], int]:
    """Render one stereo array per pad, plus the total frame count."""
    return _render(project, track, params, pad_subset, apply_gain=True)


def render_voice_stems(
    project: Project,
    track: int,
    params: RenderParams = RenderParams(),
    pad_subset: set[int] | None = None,
) -> tuple[dict[int, np.ndarray], int]:
    """Like :func:`render_pad_stems` but without any gain/fader applied."""
    return _render(project, track, params, pad_subset, apply_gain=False)
