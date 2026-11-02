"""Model-guided refinement of rendered stems against the reference mix.

The renderer reproduces the device's *magnitude* per pad but not the phase of
the zplane élastique time-stretch (see docs/FINDINGS.md §51).  A Wiener mask
built from the rendered stems recovers the missing content from the reference
mix while keeping each stem on its own part:

    w_p(f, t) = |M_p(f, t)|^2 / sum_q |M_q(f, t)|^2
    S_p       = iSTFT( w_p * Y_reference )

Because the masks sum to 1, the refined stems sum back to the reference by
construction.  The rendered stems decide *which* pad owns each time-frequency
cell; the reference decides the fine detail inside it.
"""

from __future__ import annotations

import numpy as np

from .warp import _istft, _stft

DEFAULT_N_FFT = 2048
DEFAULT_HOP = 512


def _masks(magnitudes: list[np.ndarray], floor: float = 1e-9) -> list[np.ndarray]:
    """Wiener masks that sum to exactly 1 in every cell.

    Each pad gets ``|M_p|^2``; ``floor`` is spread evenly so that silent cells
    (where every stem is zero) still partition the reference instead of losing
    it.  Without that term the masks sum to less than one in silence and the
    refined stems no longer reconstruct the mix.
    """
    total = np.zeros_like(magnitudes[0])
    for mag in magnitudes:
        total += mag * mag
    share = floor / len(magnitudes)
    denom = total + floor
    return [(mag * mag + share) / denom for mag in magnitudes]


def refine(
    stems: dict[int, np.ndarray],
    reference: np.ndarray,
    n_fft: int = DEFAULT_N_FFT,
    hop: int = DEFAULT_HOP,
) -> dict[int, np.ndarray]:
    """Return stems whose sum reproduces `reference` via Wiener masking."""
    if not stems:
        return {}
    pads = sorted(stems)
    n_frames = min(reference.shape[0], min(stems[p].shape[0] for p in pads))
    channels = reference.shape[1]
    out = {p: np.zeros((reference.shape[0], channels)) for p in pads}

    for ch in range(channels):
        specs = [_stft(stems[p][:n_frames, ch], n_fft, hop) for p in pads]
        ref_spec = _stft(reference[:n_frames, ch], n_fft, hop)
        mags = [np.abs(s) for s in specs]
        for pad, mask in zip(pads, _masks(mags)):
            part = _istft(mask * ref_spec, n_fft, hop, n_frames)
            out[pad][:n_frames, ch] = part
    return out


def residual(stems: dict[int, np.ndarray], reference: np.ndarray) -> float:
    """RMS of ``sum(stems) - reference`` over the shorter length."""
    n = min(reference.shape[0], min(v.shape[0] for v in stems.values()))
    total = sum(v[:n] for v in stems.values())
    return float(np.sqrt(np.mean((total - reference[:n]) ** 2)))
