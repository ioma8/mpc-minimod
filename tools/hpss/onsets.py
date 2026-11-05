#!/usr/bin/env python3
"""Reference onset detector for Spectral Chop.

    python3 tools/hpss/onsets.py IN.wav [--max 16] [--start S] [--dur S]

Uses numpy.  The shipped C version is mod/src/spectral.c; this script is the
readable reference for checking counts and positions.
"""
import argparse
import math

import numpy as np

from wavio import read_wav


def detect(data, sr, ch, max_positions=16):
    x = np.asarray(data, dtype=np.float32).reshape(-1, ch).mean(axis=1)
    n_fft, hop = 1024, 512
    win = np.hanning(n_fft).astype(np.float32)
    frames = (len(x) + hop - 1) // hop
    flux = np.zeros(frames, dtype=np.float32)
    prev = np.zeros(n_fft // 2 + 1, dtype=np.float32)
    for f in range(frames):
        seg = np.zeros(n_fft, dtype=np.float32)
        a = f * hop
        b = min(len(x), a + n_fft)
        seg[:b - a] = x[a:b]
        mag = np.abs(np.fft.rfft(seg * win))
        if f:
            flux[f] = np.maximum(0.0, mag - prev).sum()
        prev = mag
    peak = float(flux.max())
    if peak > 0:
        flux /= peak

    radius = max(1, int(round(0.5 * sr / hop)))
    gap = max(1, int(math.ceil(0.060 * sr / hop)))
    peaks = []
    last = None
    for f in range(1, frames - 1):
        lo, hi = max(0, f - radius), min(frames - 1, f + radius)
        threshold = max(float(np.median(flux[lo:hi + 1])) * 1.8, 0.35)
        if flux[f] < threshold or flux[f] < flux[f - 1] or flux[f] <= flux[f + 1]:
            continue
        pos = f * hop + n_fft // 2
        if pos + n_fft // 2 >= len(x):
            continue  # end-of-buffer padding makes a fake transient
        if last is not None and pos - last < gap * hop:
            continue
        peaks.append((float(flux[f]), pos))
        last = pos

    detected = len(peaks)
    chosen = sorted(peaks, key=lambda p: p[0], reverse=True)[:max_positions]
    if chosen and chosen[0][1] > 0.050 * sr:
        if len(chosen) == max_positions:
            chosen.pop()
        chosen.append((0.0, 0))
    chosen.sort(key=lambda p: p[1])
    return [p for _, p in chosen], detected


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("--max", type=int, default=16)
    ap.add_argument("--start", type=float, default=0.0)
    ap.add_argument("--dur", type=float, default=0.0)
    args = ap.parse_args()
    data, sr, ch = read_wav(args.input)
    frames = len(data) // ch
    a = max(0, int(args.start * sr))
    b = frames if not args.dur else min(frames, a + int(args.dur * sr))
    positions, detected = detect(data[a * ch:b * ch], sr, ch, args.max)
    print(f"onsets={len(positions)} detected={detected} rate={sr} ch={ch}")
    for p in positions:
        print(f"{a + p}\t{(a + p) / sr:.4f}s")


if __name__ == "__main__":
    main()
