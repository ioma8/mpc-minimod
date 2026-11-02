"""Master bus processing recovered from the reference mixdown.

The MPC Song export runs the summed voices through the insert chain on the
output strip.  In this project only the ``Color Compressor`` is active (the user
confirmed Flex Beat and the pad FXs were not used).

The device manual documents the controls:

    Attack   0.100 - 150 ms   (K1)
    Release  3.0 - 300 ms     (K2)
    Amount   0.00 - 100 %     (K3)  -- combines threshold and ratio
    In Boost (SHIFT+K3)
    Color    parallel bass boost + saturation + pitch instability
    Makeup   computed automatically from input level and settings

Fitted against the reference mixdown (knob positions attack 90 %, release 80 %,
amount 30 %, in gain 0)::

    attack 72.2 ms (= 0.1 * (150/0.1)**0.90)
    release 119.4 ms (= 3.0 * (300/3.0)**0.80)
    threshold -6 dB, ratio 2:1, automatic makeup

A bus compressor applies one gain curve to the whole mix, so the same curve can
be applied to every stem and the stems still sum to the compressed mix.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np


@dataclass(frozen=True)
class CompressorParams:
    threshold_db: float = -3.0
    ratio: float = 3.0
    #: fast: the bus stage behaves like a limiter, not like the slow 72 ms
    #: compressor first fitted.  Recovered as a plateau (attack 10...25 ms,
    #: release 0.5...4 ms); FINDINGS 71.
    attack_ms: float = 15.0
    release_ms: float = 1.0
    #: RMS detector window used by the fit
    detector_ms: float = 5.0
    #: "makeup gain is computed automatically" -> compensate the mean reduction
    automatic_makeup: bool = True
    enabled: bool = True


def _box_rms(x: np.ndarray, window: int) -> np.ndarray:
    window = max(8, int(window))
    cs = np.concatenate(([0.0], np.cumsum(x * x)))
    idx = np.arange(len(x))
    lo = np.maximum(idx - window, 0)
    hi = np.minimum(idx + 1, len(x))
    return np.sqrt((cs[hi] - cs[lo]) / np.maximum(hi - lo, 1) + 1e-18)


def gain_curve(x: np.ndarray, params: CompressorParams, sample_rate: int = 44100) -> np.ndarray:
    """Linear gain to apply to the bus for every sample."""
    if not params.enabled:
        return np.ones(len(x))
    level = 20.0 * np.log10(_box_rms(x, int(params.detector_ms * 1e-3 * sample_rate)) + 1e-12)
    target = np.maximum(0.0, level - params.threshold_db) * (1.0 - 1.0 / params.ratio)
    a = np.exp(-1.0 / (max(params.attack_ms, 0.01) * 1e-3 * sample_rate))
    r = np.exp(-1.0 / (max(params.release_ms, 0.01) * 1e-3 * sample_rate))
    gr = np.empty_like(target)
    g = 0.0
    for i in range(len(target)):
        t = target[i]
        c = a if t > g else r
        g = c * g + (1.0 - c) * t
        gr[i] = g
    makeup = float(gr.mean()) if params.automatic_makeup else 0.0
    return 10.0 ** ((-gr + makeup) / 20.0)


def apply_to_stems(
    stems: dict[int, np.ndarray], params: CompressorParams, sample_rate: int = 44100
) -> dict[int, np.ndarray]:
    """Apply the bus gain curve to every stem (they still sum to the bus)."""
    if not params.enabled or not stems:
        return stems
    total = sum(stems.values())
    gain = gain_curve(total.sum(axis=1), params, sample_rate)[:, None]
    return {p: s * gain for p, s in stems.items()}
