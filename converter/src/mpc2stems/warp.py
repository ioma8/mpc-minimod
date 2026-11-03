"""Time-stretch ("warp") used for pads with ``warpEnable``.

The device time-stretches warped samples so they follow the project tempo while
keeping their pitch.  The stretch *rate* is well determined from the reference
audio (see ``docs/FINDINGS.md``):

    rate = project_bpm / sample_tempo

The stretch *algorithm* is not published.  This module implements a phase
vocoder in the STFT domain -- the closest family measured against the device
output.  It is an approximation, not a bit-exact reproduction of the device.
"""

from __future__ import annotations

import numpy as np

N_FFT = 512
HOP = 64


def stretch_rate(
    project_bpm: float,
    sample_tempo: float,
    bpm_lock: bool,
    stretch_percentage: int,
) -> float:
    """Playback rate (input seconds per output second) for a warped pad."""
    if sample_tempo > 0:
        return project_bpm / sample_tempo
    return max(1e-6, stretch_percentage / 100.0)


def _window(n_fft: int) -> np.ndarray:
    return np.hanning(n_fft).astype(np.float64)


def _stft(x: np.ndarray, n_fft: int, hop: int) -> np.ndarray:
    win = _window(n_fft)
    pad = n_fft // 2
    xp = np.concatenate([np.zeros(pad), x, np.zeros(n_fft)])
    n_frames = 1 + (len(xp) - n_fft) // hop
    frames = np.stack([xp[i * hop : i * hop + n_fft] * win for i in range(n_frames)])
    return np.fft.rfft(frames, axis=1)


def _istft(spec: np.ndarray, n_fft: int, hop: int, length: int) -> np.ndarray:
    win = _window(n_fft)
    n_out = (spec.shape[0] - 1) * hop + n_fft
    out = np.zeros(n_out)
    wsum = np.zeros(n_out)
    for i in range(spec.shape[0]):
        out[i * hop : i * hop + n_fft] += np.fft.irfft(spec[i], n_fft) * win
        wsum[i * hop : i * hop + n_fft] += win**2
    out /= np.maximum(wsum, 1e-10)
    return out[n_fft // 2 : n_fft // 2 + length]


def _phase_vocoder(spec: np.ndarray, rate: float, n_fft: int, hop: int) -> np.ndarray:
    """Standard STFT phase vocoder: output frame t samples input frame t*rate."""
    n_frames, n_bins = spec.shape
    time_steps = np.arange(0.0, n_frames - 1, rate)
    if len(time_steps) < 2:
        return spec[:1]

    phi_advance = 2.0 * np.pi * hop * np.arange(n_bins) / n_fft
    mag = np.abs(spec)
    phase = np.angle(spec)

    out = np.zeros((len(time_steps), n_bins), dtype=complex)
    phi = phase[0].copy()
    out[0] = mag[0] * np.exp(1j * phi)

    for t in range(1, len(time_steps)):
        step = int(time_steps[t])
        frac = time_steps[t] - step
        step = min(step, n_frames - 2)
        a = mag[step] * (1.0 - frac) + mag[step + 1] * frac
        dphase = phase[step + 1] - phase[step] - phi_advance
        dphase = (dphase + np.pi) % (2.0 * np.pi) - np.pi
        phi = phi + dphase + phi_advance
        out[t] = a * np.exp(1j * phi)
    return out


def _expected_power(x: np.ndarray, n_out: int) -> np.ndarray:
    """Power envelope the stretched signal should have, on a coarse grid.

    A phase vocoder keeps the *magnitudes* of the analysis frames but rewrites
    their phases, so the overlap-add no longer sums coherently and the output
    loses energy -- measured at -3.4 dB on broadband material.  The input power
    envelope, resampled to the output length, is what the stretch should
    preserve, so it is used as the reference for the correction below.
    """
    n = len(x)
    block = max(1, n // 512)
    m = n // block
    if m < 2:
        return np.full(n_out, float(np.mean(x**2)) if n else 0.0)
    p = np.array([np.mean(x[i * block : (i + 1) * block] ** 2) for i in range(m)])
    return np.interp(
        np.linspace(0.0, 1.0, n_out),
        np.linspace(0.0, 1.0, m),
        p,
    )


def _match_power(y: np.ndarray, expected: np.ndarray, hop: int) -> np.ndarray:
    """Scale ``y`` towards ``expected`` with a slowly varying, smoothed gain."""
    n = len(y)
    if n == 0:
        return y
    window = max(hop * 8, 1024)
    if window >= n:
        windows = 1
    else:
        windows = max(1, n // window)
    edges = np.linspace(0, n, windows + 1).astype(int)
    gain = np.ones(n)
    for i in range(windows):
        lo, hi = edges[i], edges[i + 1]
        if hi <= lo:
            continue
        actual = float(np.mean(y[lo:hi] ** 2))
        want = float(np.mean(expected[lo:hi]))
        if actual > 1e-20 and want > 1e-20:
            gain[lo:hi] = min(np.sqrt(want / actual), 4.0)
    # smooth so the correction does not modulate transients
    kernel = np.hanning(max(3, window // 4))
    kernel /= kernel.sum()
    return y * np.convolve(gain, kernel, mode="same")


def _stretch_channel(
    x: np.ndarray, rate: float, n_fft: int, hop: int, energy_match: bool = True
) -> np.ndarray:
    length = max(1, int(round(len(x) / rate)))
    spec = _stft(x, n_fft, hop)
    moved = _phase_vocoder(spec, rate, n_fft, hop)
    out = _istft(moved, n_fft, hop, length)
    if not energy_match:
        return out
    return _match_power(out, _expected_power(x, length), hop)


def time_stretch(
    x: np.ndarray,
    rate: float,
    n_fft: int = N_FFT,
    hop: int = HOP,
    energy_match: bool = True,
) -> np.ndarray:
    """Phase-vocoder time stretch. ``rate > 1`` makes the signal shorter.

    ``energy_match`` restores the power the overlap-add loses to incoherent
    phases (-3.4 dB on broadband material).  Leaving it off reproduces the
    earlier, quieter output; ``RenderParams.warp_energy_match`` drives this.
    """
    if abs(rate - 1.0) < 1e-6:
        return x
    x = np.asarray(x, dtype=np.float64)
    if x.ndim == 1:
        return _stretch_channel(x, rate, n_fft, hop, energy_match)
    return np.stack(
        [_stretch_channel(x[:, c], rate, n_fft, hop, energy_match) for c in range(x.shape[1])],
        axis=1,
    )


def _wsola_channel(x: np.ndarray, rate: float, frame: int, hop: int, tol: int) -> np.ndarray:
    """One channel of WSOLA (waveform-similarity overlap-add)."""
    length = max(1, int(round(len(x) / rate)))
    if abs(rate - 1.0) < 1e-6 or len(x) < frame + tol + hop:
        # too short to search: fall back to plain linear resampling
        n_out = length
        pos = np.arange(n_out) * rate
        return np.interp(pos, np.arange(len(x)), x)

    win = np.hanning(frame + 2)[1:-1]
    xp = np.concatenate([np.zeros(frame + tol), x, np.zeros(frame + tol)])
    base = frame + tol  # index of x[0] inside xp
    n_hops = int(np.ceil((length + frame) / hop))
    out = np.zeros((n_hops + 1) * hop + frame)
    wsum = np.zeros_like(out)

    # the first frame is taken verbatim, so the attack is not smeared
    prev = 0
    starts = [0]
    for k in range(1, n_hops + 1):
        nominal = int(round(k * hop * rate))
        # template = the natural continuation of the previously placed frame
        tpl = xp[base + prev + hop : base + prev + hop + hop]
        lo = max(0, nominal - tol)
        hi = min(len(x) - hop, nominal + tol)
        if hi <= lo or tpl.size < hop:
            best = nominal
        else:
            cand = np.lib.stride_tricks.sliding_window_view(
                xp[base + lo : base + hi + hop], hop
            )
            scores = cand @ tpl
            best = lo + int(np.argmax(scores))
        best = int(np.clip(best, 0, max(0, len(x) - 1)))
        seg = xp[base + best : base + best + frame]
        if seg.size < frame:
            seg = np.pad(seg, (0, frame - seg.size))
        at = k * hop
        out[at : at + frame] += seg * win
        wsum[at : at + frame] += win
        prev = best
        starts.append(best)

    out = out / np.maximum(wsum, 1e-9)
    return out[frame // 2 : frame // 2 + length]


def wsola(x: np.ndarray, rate: float, frame: int = 2048, hop: int = 512, tol: int = 256) -> np.ndarray:
    """Waveform-similarity overlap-add stretch. ``rate > 1`` makes it shorter.

    Unlike a phase vocoder this keeps the *waveform* of each partial intact:
    every output frame is a verbatim copy of an input frame, chosen so that it
    continues the previous one without a phase jump.  For the modest ratios this
    device uses (1.0…1.3x) that preserves both transients and tonal phase, which
    a phase vocoder does not.
    """
    if abs(rate - 1.0) < 1e-6:
        return x
    x = np.asarray(x, dtype=np.float64)
    chans = [x] if x.ndim == 1 else [x[:, c] for c in range(x.shape[1])]
    out = [_wsola_channel(np.ascontiguousarray(c), rate, frame, hop, tol) for c in chans]
    return out[0] if x.ndim == 1 else np.stack(out, axis=1)


def _phase_locked_vocoder(spec: np.ndarray, rate: float, n_fft: int, hop: int) -> np.ndarray:
    """Identity phase locking (Laroche-Dolson).

    Bins in the region of influence of a spectral peak inherit that peak's phase
    advance while keeping their relative phase, which keeps partials coherent
    across the stretched frames.
    """
    n_frames, n_bins = spec.shape
    time_steps = np.arange(0.0, n_frames - 1, rate)
    if len(time_steps) < 2:
        return spec[:1]

    omega = 2.0 * np.pi * hop * np.arange(n_bins) / n_fft
    mag = np.abs(spec)
    phase = np.angle(spec)
    out = np.zeros((len(time_steps), n_bins), dtype=complex)

    phi = phase[0].copy()
    prev_phase = phase[0].copy()
    out[0] = mag[0] * np.exp(1j * phi)

    for k in range(1, len(time_steps)):
        step = min(int(time_steps[k]), n_frames - 2)
        frac = time_steps[k] - step
        m = mag[step] * (1.0 - frac) + mag[step + 1] * frac
        p = phase[step]

        # per-bin instantaneous frequency
        dphi = p - prev_phase - omega
        dphi = (dphi + np.pi) % (2.0 * np.pi) - np.pi
        advance = dphi + omega

        # spectral peaks: local maxima above a fraction of the frame maximum
        peak = np.zeros(n_bins, dtype=bool)
        if n_bins > 2:
            peak[1:-1] = (m[1:-1] > m[:-2]) & (m[1:-1] >= m[2:])
            peak &= m > 0.01 * (m.max() + 1e-20)
        idx = np.flatnonzero(peak)

        if len(idx) == 0:
            phi = phi + advance
        else:
            # region of influence: split the spectrum at the minima between peaks
            bounds = [0]
            for a, b in zip(idx[:-1], idx[1:]):
                seg = m[a : b + 1]
                bounds.append(a + int(np.argmin(seg)))
            bounds.append(n_bins)
            new_phi = phi + advance
            for i, pk in enumerate(idx):
                lo = bounds[i]
                hi = bounds[i + 1] if i + 1 < len(bounds) else n_bins
                base = (phi[pk] + advance[pk]) - p[pk]
                new_phi[lo:hi] = base + p[lo:hi]
            phi = new_phi
        prev_phase = p
        out[k] = m * np.exp(1j * phi)
    return out


def time_stretch_locked(x: np.ndarray, rate: float, n_fft: int = N_FFT, hop: int = HOP) -> np.ndarray:
    """Time stretch with identity phase locking (measured worse than plain)."""
    if abs(rate - 1.0) < 1e-6:
        return x
    x = np.asarray(x, dtype=np.float64)
    chans = [x] if x.ndim == 1 else [x[:, c] for c in range(x.shape[1])]
    out = []
    for ch in chans:
        length = max(1, int(round(len(ch) / rate)))
        spec = _stft(ch, n_fft, hop)
        out.append(_istft(_phase_locked_vocoder(spec, rate, n_fft, hop), n_fft, hop, length))
    return out[0] if x.ndim == 1 else np.stack(out, axis=1)
