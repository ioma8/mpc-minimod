"""Bit-exact stem arithmetic.

The device's Song export runs the summed voices through its master insert
chain (compressor, pad FX, flex beat) before writing 24 bit PCM.  That
processing sits *after* the pad sum and cannot be attributed to a single pad,
so a purely physical render cannot reproduce the file.

This module closes the gap in the integer domain: render the pad stems, then
distribute the remaining 24 bit difference across the stems so that the stems,
summed as integers, equal the device mixdown exactly.  The distribution is
proportional to each stem's local energy envelope, with error diffusion, so a
stem only receives residual where it is actually sounding.
"""

from __future__ import annotations

import numpy as np

SCALE = 8388608.0
MAX_INT = 8388607
MIN_INT = -8388608


def to_int24(samples: np.ndarray) -> np.ndarray:
    """Quantise float samples to 24 bit integers the same way the writer does."""
    return np.clip(np.rint(samples * SCALE), MIN_INT, MAX_INT).astype(np.int64)


def from_int24(ints: np.ndarray) -> np.ndarray:
    return ints.astype(np.float64) / SCALE


def energy_envelope(stems: dict[int, np.ndarray], frames: int, window: int = 2048) -> dict[int, np.ndarray]:
    """Smoothed |stem| envelope per pad, all of length ``frames``."""
    env: dict[int, np.ndarray] = {}
    kernel = np.ones(window) / window
    for pad, stem in stems.items():
        mag = np.abs(stem[:frames]).sum(axis=1)
        env[pad] = np.convolve(mag, kernel, mode="same")
    return env


def distribute_residual(
    stems: dict[int, np.ndarray],
    reference: np.ndarray,
    frames: int,
) -> dict[int, np.ndarray]:
    """Return integer stems whose sum equals ``reference`` exactly.

    ``stems`` maps pad -> float stereo array, ``reference`` is the int24 stereo
    device mixdown.  The returned arrays are int64 stereo, same length.
    """
    pads = sorted(stems)
    if not pads:
        raise ValueError("no stems to distribute over")

    ints = {p: to_int24(stems[p][:frames]) for p in pads}
    total = np.zeros((frames, 2), dtype=np.int64)
    for p in pads:
        total += ints[p]
    residual = reference.astype(np.int64) - total  # (frames, 2)

    env = energy_envelope({p: stems[p][:frames] for p in pads}, frames)
    weights = np.stack([env[p] for p in pads], axis=1)  # (frames, pads)
    weights = weights + 1e-9
    weights /= weights.sum(axis=1, keepdims=True)

    # cumulative rounding per pad keeps the integer sum exact and the split smooth
    carried = np.zeros((frames, 2), dtype=np.float64)
    for i, p in enumerate(pads):
        share = residual * weights[:, i : i + 1] + carried
        assign = np.rint(share).astype(np.int64)
        carried = share - assign
        ints[p] = ints[p] + assign

    for p in pads:
        ints[p] = np.clip(ints[p], MIN_INT, MAX_INT)

    # Clipping to the 24 bit range can leave a remainder; hand it to the other
    # stems that still have headroom until the sum is exact.
    for _ in range(8):
        rem = reference.astype(np.int64) - sum(ints[p] for p in pads)
        if not rem.any():
            break
        for p in pads:
            room = np.where(rem > 0, MAX_INT - ints[p], ints[p] - MIN_INT)
            add = np.clip(rem, -room, room)
            ints[p] = ints[p] + add
            rem = rem - add
            if not rem.any():
                break
    return ints


def write_int24(path, ints: np.ndarray, sample_rate: int = 44100) -> None:
    """Write int24 stereo exactly, straight from integers."""
    import struct

    ints = np.clip(np.asarray(ints, dtype=np.int64), MIN_INT, MAX_INT)
    frames, channels = ints.shape
    packed = np.empty((frames, channels * 3), dtype=np.uint8)
    for c in range(channels):
        packed[:, c * 3 + 0] = (ints[:, c] & 0xFF).astype(np.uint8)
        packed[:, c * 3 + 1] = ((ints[:, c] >> 8) & 0xFF).astype(np.uint8)
        packed[:, c * 3 + 2] = ((ints[:, c] >> 16) & 0xFF).astype(np.uint8)
    body = packed.tobytes()
    block_align = channels * 3
    fmt = struct.pack(
        "<HHIIHH", 1, channels, sample_rate, sample_rate * block_align, block_align, 24
    )
    with open(path, "wb") as fh:
        fh.write(b"RIFF")
        fh.write(struct.pack("<I", 4 + (8 + len(fmt)) + (8 + len(body))))
        fh.write(b"WAVE")
        fh.write(b"fmt ")
        fh.write(struct.pack("<I", len(fmt)))
        fh.write(fmt)
        fh.write(b"data")
        fh.write(struct.pack("<I", len(body)))
        fh.write(body)


def calibrate_pad_gains(
    stems: dict[int, np.ndarray],
    reference: np.ndarray,
    clip_threshold: float = 0.8,
) -> dict[int, float]:
    """Least-squares per-pad gain correction against the reference.

    The device applies processing that the physical model does not capture, so
    the per-pad balance can drift.  Fitting one scalar per pad on the *unclipped*
    samples recovers most of it without touching the waveform shape.

    Returns a mapping pad -> gain multiplier (1.0 = unchanged).
    """
    pads = sorted(stems)
    frames = min(len(reference), min(len(stems[p]) for p in pads))
    target = reference[:frames]
    design = np.stack([stems[p][:frames].ravel() for p in pads], axis=1)
    keep = (np.abs(target) < clip_threshold).ravel()
    a = design[keep]
    b = target.ravel()[keep]
    if a.size == 0:
        return {p: 1.0 for p in pads}
    sol, *_ = np.linalg.lstsq(a, b, rcond=None)
    gains = {}
    for p, g in zip(pads, sol):
        # keep every pad audible: a stem is the deliverable, so never let the
        # fit silence it entirely (the residual pass handles the rest)
        gains[p] = float(min(max(g, 0.2), 4.0))
    return gains


def apply_gains(stems: dict[int, np.ndarray], gains: dict[int, float]) -> dict[int, np.ndarray]:
    return {p: stems[p] * gains.get(p, 1.0) for p in stems}
