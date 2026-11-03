"""Modulation envelopes.

Each `.xpj` envelope stores every stage as a normalised 0..1 value (`Attack`,
`Decay`, `Sustain`, `Release`, `Hold`) plus `DecayFromEnd`, `AD`, `OneShot` and
three curve amounts.

The firmware converts those normalised times to seconds; that mapping is not
recoverable from the file or the binary, so each stage gets an explicit
seconds-per-unit scale.  `attack` is calibrated against the reference render
(1.2 s per unit); the others default to 0, i.e. *not modelled*, because applying
them with a guessed scale measurably degraded the match — see
`docs/FINDINGS.md` §57.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import numpy as np


def stage(envelope: dict[str, Any], name: str, default: float = 0.0) -> float:
    """Read a normalised stage value from a ``{"value0": x}`` wrapper."""
    entry = envelope.get(name)
    if isinstance(entry, dict):
        return float(entry.get("value0", default))
    if isinstance(entry, (int, float)) and not isinstance(entry, bool):
        return float(entry)
    return default


def flag(envelope: dict[str, Any], name: str, default: bool = False) -> bool:
    """Read a boolean stage flag from a ``{"value0": x}`` wrapper."""
    entry = envelope.get(name)
    if isinstance(entry, dict):
        return bool(entry.get("value0", default))
    if isinstance(entry, bool):
        return entry
    return default


def curve(x: np.ndarray, amount: float) -> np.ndarray:
    """Shape a 0..1 ramp. 0.5 is linear, <0.5 convex, >0.5 concave."""
    if x.size == 0 or abs(amount - 0.5) < 1e-6:
        return x
    exponent = float(np.exp((0.5 - amount) * 4.0))
    return np.power(np.clip(x, 0.0, 1.0), exponent)


@dataclass(frozen=True)
class EnvelopeScales:
    """Seconds per normalised unit, per stage.

    Zero means "do not model this stage".
    """

    attack: float = 1.2
    decay: float = 0.0
    release: float = 0.0
    hold: float = 0.0
    delay: float = 0.0

    def seconds(self, value: float, scale: float) -> float:
        return max(0.0, float(value)) * max(0.0, scale)


def shape(
    envelope: dict[str, Any],
    n_frames: int,
    sample_rate: int,
    scales: EnvelopeScales | None = None,
    release_frames: int | None = None,
) -> np.ndarray:
    """Render an envelope as a per-sample gain curve of length `n_frames`.

    * `AD` (attack/decay) ignores `Sustain` and decays to zero.
    * `DecayFromEnd` stretches the decay so it lands on the end of the sample.
    * `release_frames` marks where a gated note is released.
    * A stage whose scale is 0 is not applied at all.
    """
    scales = scales or EnvelopeScales()
    if n_frames <= 0:
        return np.ones(0)

    sustain = float(np.clip(stage(envelope, "Sustain", 1.0), 0.0, 1.0))
    if flag(envelope, "AD"):
        sustain = 0.0

    attack = scales.seconds(stage(envelope, "Attack"), scales.attack)
    decay = scales.seconds(stage(envelope, "Decay"), scales.decay)
    hold = scales.seconds(stage(envelope, "Hold"), scales.hold)
    release = scales.seconds(stage(envelope, "Release"), scales.release)
    delay = scales.seconds(stage(envelope, "Delay"), scales.delay)

    if flag(envelope, "DecayFromEnd") and decay > 0.0:
        decay = max(decay, n_frames / sample_rate)

    env = np.ones(n_frames)
    pos = 0

    delay_n = min(int(delay * sample_rate), n_frames)
    if delay_n:
        env[:delay_n] = 0.0
        pos = delay_n
    if pos >= n_frames:
        return env

    attack_n = min(int(attack * sample_rate), n_frames - pos)
    if attack_n > 1:
        env[pos : pos + attack_n] = curve(
            np.linspace(0.0, 1.0, attack_n, endpoint=False), stage(envelope, "AttackCurve", 0.5)
        )
        pos += attack_n
    if pos >= n_frames:
        return env

    hold_n = min(int(hold * sample_rate), n_frames - pos)
    if hold_n:
        env[pos : pos + hold_n] = 1.0
        pos += hold_n

    decay_n = min(int(decay * sample_rate), n_frames - pos)
    if decay_n > 1:
        env[pos : pos + decay_n] = 1.0 + (sustain - 1.0) * curve(
            np.linspace(0.0, 1.0, decay_n, endpoint=False), stage(envelope, "DecayCurve", 0.5)
        )
        pos += decay_n
        if pos < n_frames:
            env[pos:] = sustain
    # a decay whose scale is 0 is "not modelled": the envelope holds at 1.0
    # rather than jumping to the sustain level

    if release_frames is not None and 0 <= release_frames < n_frames and release > 0.0:
        release_n = min(int(release * sample_rate), n_frames - release_frames)
        if release_n > 1:
            level = env[release_frames - 1] if release_frames else env[0]
            env[release_frames : release_frames + release_n] = level * curve(
                np.linspace(1.0, 0.0, release_n, endpoint=False),
                stage(envelope, "ReleaseCurve", 0.5),
            )

    return np.clip(env, 0.0, 1.0)


#: LFO waveform ids observed in `lfoWaveformType` (0 in this project).  The
#: numbering is an assumption; only used when a destination is non-zero.
_WAVEFORMS = ("sine", "triangle", "square", "saw_up", "saw_down")


def lfo_curve(
    n_frames: int,
    sample_rate: int,
    rate_hz: float,
    waveform: int = 0,
    fade_in: float = 0.0,
    delay: float = 0.0,
) -> np.ndarray:
    """A unipolar-ish LFO in -1..1 with optional fade-in and delay."""
    if n_frames <= 0:
        return np.ones(0)
    t = np.arange(n_frames) / sample_rate
    phase = (t * max(rate_hz, 1e-6)) % 1.0
    name = _WAVEFORMS[waveform % len(_WAVEFORMS)]
    if name == "sine":
        wave = np.sin(2.0 * np.pi * phase)
    elif name == "triangle":
        wave = 4.0 * np.abs(phase - 0.5) - 1.0
    elif name == "square":
        wave = np.where(phase < 0.5, 1.0, -1.0)
    elif name == "saw_up":
        wave = 2.0 * phase - 1.0
    else:
        wave = 1.0 - 2.0 * phase
    if delay > 0:
        wave = np.where(t < delay, 0.0, wave)
    if fade_in > 0:
        wave *= np.clip((t - delay) / fade_in, 0.0, 1.0)
    return wave
