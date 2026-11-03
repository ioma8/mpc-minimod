"""Pad filter modelling.

Each pad instrument carries a two-slot filter section::

    synthSection.filterData.value0 = {filterType, filterCutoff, filterResonance, ...}

``filterCutoff`` is normalised 0..1 where 1.0 is fully open (pads with cutoff 1.0
and resonance 0 are transparent, which is why they matched before the filter was
modelled).  The type enum is recovered from the firmware (see docs/FORMAT.md);
the mapping is documented in ``render.RenderParams.filter_types``.  The
values here are calibrated against the reference mixdown.
"""

from __future__ import annotations

import numpy as np

#: The firmware's filter-type enum, read from the name table at ``0x45bb4b0`` and
#: the immediate values in its name-lookup function (``0xbb1700``): value 1 is
#: "Lowpass 1 Pole", 2 is "Lowpass 2 Pole", 3 is "Lowpass 4 Pole", and so on.
#: Entries with ``kind=None`` are character filters the firmware implements
#: itself (MPC3000 emulation, Vocal, Model) and this renderer does not model.
FILTER_ENUM: dict[int, tuple[str, str | None, int]] = {
    1: ("Lowpass 1 Pole", "lp", 1),
    2: ("Lowpass 2 Pole", "lp", 2),
    3: ("Lowpass 4 Pole", "lp", 4),
    4: ("Lowpass 6 Pole", "lp", 6),
    5: ("Lowpass 8 Pole", "lp", 8),
    6: ("Highpass 1 Pole", "hp", 1),
    7: ("Highpass 2 Pole", "hp", 2),
    8: ("Highpass 4 Pole", "hp", 4),
    9: ("Highpass 6 Pole", "hp", 6),
    10: ("Highpass 8 Pole", "hp", 8),
    11: ("Bandpass 2 Pole", "bp", 2),
    12: ("Bandpass 4 Pole", "bp", 4),
    13: ("Bandpass 6 Pole", "bp", 6),
    14: ("Bandpass 8 Pole", "bp", 8),
    15: ("Bandstop 2 Pole", "bs", 2),
    16: ("Bandstop 4 Pole", "bs", 4),
    17: ("Bandstop 6 Pole", "bs", 6),
    18: ("Bandstop 8 Pole", "bs", 8),
    19: ("Bandboost 2 Pole", "bb", 2),
    20: ("Bandboost 4 Pole", "bb", 4),
    21: ("Bandboost 6 Pole", "bb", 6),
    22: ("Bandboost 8 Pole", "bb", 8),
    23: ("Model1", None, 0),
    24: ("Model2", None, 0),
    25: ("Model3", None, 0),
    26: ("Vocal1", None, 0),
    27: ("Vocal2", None, 0),
    28: ("Vocal3", None, 0),
    # MPC3000 LPF: an emulation, not a plain biquad.  Measured against a pad-16
    # capture and the reference mixdown it behaves as a 2-pole lowpass
    # (spread 5.7 dB / residual 0.17839) and not as 4-pole (7.7 dB / 0.18112).
    29: ("MPC3000 LPF", "lp", 2),
}

#: Types that are plain biquad responses, ready for :func:`apply_filter`.
MODELLED_FILTER_TYPES = {k: (name, kind, poles) for k, (name, kind, poles) in FILTER_ENUM.items() if kind}


def filter_name(filter_type: int) -> str:
    """Display name for a ``filterType`` value, or ``"Filter <n>"`` if unknown."""
    entry = FILTER_ENUM.get(int(filter_type))
    return entry[0] if entry else f"Filter {filter_type}"

#: cutoff 0..1 -> Hz, logarithmic over this range
MIN_HZ = 20.0
MAX_HZ = 20000.0


def cutoff_hz(normalised: float) -> float:
    c = min(max(float(normalised), 0.0), 1.0)
    return MIN_HZ * (MAX_HZ / MIN_HZ) ** c


def biquad_lowpass(x: np.ndarray, fc: float, q: float, sr: int) -> np.ndarray:
    """RBJ cookbook low-pass, applied with zero phase (forward-backward)."""
    fc = min(max(fc, 10.0), sr * 0.49)
    w0 = 2.0 * np.pi * fc / sr
    cos_w0 = np.cos(w0)
    alpha = np.sin(w0) / (2.0 * max(q, 1e-3))
    b0 = (1.0 - cos_w0) / 2.0
    b1 = 1.0 - cos_w0
    b2 = (1.0 - cos_w0) / 2.0
    a0 = 1.0 + alpha
    a1 = -2.0 * cos_w0
    a2 = 1.0 - alpha
    b = np.array([b0, b1, b2]) / a0
    a = np.array([1.0, a1 / a0, a2 / a0])

    y = np.empty_like(x)
    for c in range(x.shape[1] if x.ndim == 2 else 1):
        sig = x[:, c] if x.ndim == 2 else x
        fwd = _lfilter(b, a, sig)
        out = _lfilter(b, a, fwd[::-1])[::-1]
        if x.ndim == 2:
            y[:, c] = out
        else:
            y = out
    return y


def juce_coefficients(
    kind: str, fc: float, q: float, sr: int = 44100
) -> tuple[float, float, float, float, float]:
    """Biquad coefficients for one section, in float32.

    Low-pass, high-pass and band-pass use the device's own JUCE forms, decoded at
    ``0xfcc950``..``0xfccba0``::

        n  = 1 / tanf(pi * f / fs)
        c1 = 1 / (1 + (1/Q) n + n*n)
        lp: b = c1 (1, 2, 1)          hp: b = c1 n*n (1, -2, 1)      bp: b = c1 (1/Q) n (1, 0, -1)
        a1 = 2 c1 (1 - n*n)           a2 = c1 (1 - (1/Q) n + n*n)

    Notch and peak use the standard RBJ numerators on the same pole polynomial;
    the firmware's own forms for those two were not decoded, so they are the
    conventional ones rather than the device's.

    Everything is computed in float32 to match the device's precision.
    """
    sr32 = np.float32(sr)
    f = np.float32(min(max(float(fc), 10.0), float(sr) * 0.45))
    q = np.float32(max(float(q), 1e-3))
    n = np.float32(1.0) / np.float32(np.tan(np.float32(np.pi) * f / sr32))
    n2 = np.float32(n * n)
    inv_q = np.float32(np.float32(1.0) / q)
    c1 = np.float32(np.float32(1.0) / np.float32(np.float32(1.0) + inv_q * n + n2))
    a1 = np.float32(np.float32(2.0) * c1 * np.float32(np.float32(1.0) - n2))
    a2 = np.float32(c1 * np.float32(np.float32(1.0) - inv_q * n + n2))
    two_c1 = np.float32(np.float32(2.0) * c1)

    if kind == "lp":
        b0, b1, b2 = c1, two_c1, c1
    elif kind == "hp":
        g = np.float32(c1 * n2)
        b0, b1, b2 = g, np.float32(-2.0) * g, g
    elif kind == "bp":
        g = np.float32(c1 * inv_q * n)
        b0, b1, b2 = g, np.float32(0.0), np.float32(-g)
    elif kind in ("bs", "bb"):
        w0 = np.float32(np.float32(2.0) * np.float32(np.pi) * f / sr32)
        cw = np.float32(np.cos(w0))
        alpha = np.float32(np.sin(w0) / (np.float32(2.0) * q))
        if kind == "bs":                       # notch: unity away, zero at fc
            a0 = np.float32(np.float32(1.0) + alpha)
            b0, b1, b2 = np.float32(1.0), np.float32(-2.0) * cw, np.float32(1.0)
            a1r = np.float32(-2.0) * cw
            a2r = np.float32(np.float32(1.0) - alpha)
        else:                                  # peak: unity away, boost at fc
            gain = np.float32(q)
            a0 = np.float32(np.float32(1.0) + alpha / gain)
            b0 = np.float32(np.float32(1.0) + alpha * gain)
            b1 = np.float32(-2.0) * cw
            b2 = np.float32(np.float32(1.0) - alpha * gain)
            a1r = np.float32(-2.0) * cw
            a2r = np.float32(np.float32(1.0) - alpha / gain)
        return (
            float(b0 / a0), float(b1 / a0), float(b2 / a0),
            float(a1r / a0), float(a2r / a0),
        )
    else:
        raise ValueError(f"unknown filter kind {kind!r}")
    return (float(b0), float(b1), float(b2), float(a1), float(a2))


def biquad(kind: str, fc: float, q: float, sr: int) -> np.ndarray:
    """One SOS row in the device's own coefficient form (see juce_coefficients)."""
    b0, b1, b2, a1, a2 = juce_coefficients(kind, fc, q, sr)
    return np.array([[b0, b1, b2, 1.0, a1, a2]], dtype=np.float32)



def svf_states(x: np.ndarray, kind: str, fc: float, q: float, sr: int = 44100) -> np.ndarray:
    """Chamberlin state-variable filter — the device's actual pad filter topology.

    Decompiled from ``FilterBlock``'s processor (``0x274e720``, FINDINGS 85).  Per
    channel and per sample the firmware runs::

        low  += f * band
        high  = x - low - qd * band
        band += f * high

    and the filter type selects which of ``low`` / ``band`` / ``high`` is emitted, so
    every type is the *same* filter with a different tap.  ``f`` and ``qd`` are
    ramped by the caller; here they are constant for the whole note.
    """
    f = np.float32(2.0 * np.sin(np.pi * min(max(fc, 10.0), sr * 0.45) / sr))
    qd = np.float32(1.0 / max(q, 1e-3))
    mono = x.ndim == 1
    data = np.asarray(x, dtype=np.float32).reshape(-1, 1) if mono else np.asarray(x, dtype=np.float32)
    out = np.empty_like(data)
    low = np.zeros(data.shape[1], dtype=np.float32)
    band = np.zeros(data.shape[1], dtype=np.float32)
    for i in range(data.shape[0]):
        low = np.float32(low + f * band)
        high = np.float32(data[i] - low - qd * band)
        band = np.float32(band + f * high)
        if kind == "lp":
            out[i] = low
        elif kind == "hp":
            out[i] = high
        elif kind == "bp":
            out[i] = band
        elif kind == "bs":
            out[i] = np.float32(low + high)
        else:                                   # bandboost -> notch + 2*band
            out[i] = np.float32(low + high + np.float32(2.0) * band)
    return out[:, 0] if mono else out

def apply_filter(
    x: np.ndarray, kind: str, poles: int, fc: float, q: float, sr: int = 44100, zero_phase: bool = False
) -> np.ndarray:
    """Filter with the given response family and slope.

    Causal (forward-only) by default: the device's filters are real IIR sections,
    and using a zero-phase fit measurably decorrelates the result (0.2086 ->
    0.2076 residual).
    """
    from scipy.signal import sosfilt, sosfiltfilt

    if kind == "off" or fc >= sr * 0.45:
        return x
    # n poles means n/2 biquad sections; 1 pole is a first-order section.  The
    # cascade count used to be a single `vstack`, which made 6- and 8-pole
    # filters silently identical to 4-pole ones.
    if poles <= 1:
        sos = _first_order(kind, float(fc), sr)
    else:
        sos = np.vstack([biquad(kind, float(fc), q, sr)] * (int(poles) // 2 or 1))
    return sosfiltfilt(sos, x, axis=0) if zero_phase else sosfilt(sos, x, axis=0)


def _first_order(kind: str, fc: float, sr: int) -> np.ndarray:
    """One-pole lowpass/highpass as a single SOS row."""
    a = float(np.exp(-2.0 * np.pi * min(max(fc, 10.0), sr * 0.45) / sr))
    if kind == "hp":
        b0, b1 = (1.0 + a) / 2.0, -(1.0 + a) / 2.0
    else:
        b0, b1 = 1.0 - a, 0.0
    return np.array([[b0, b1, 0.0, 1.0, -a, 0.0]])


def q_from_resonance(resonance: float, base: float = 0.5, scale: float = 2.0) -> float:
    """Map a normalised resonance (0..1) to a biquad Q."""
    return max(0.05, base + float(resonance) * scale)
