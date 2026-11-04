#!/usr/bin/env python3
"""Spectral Chop, exactly as the mod runs it, plus a report on the slicing.

    python3 tools/hpss/spectral_chop.py IN.wav [--max 16] [--png out.png]

This is a line-by-line port of `spectral_onsets()` in `mod/src/spectral.c`
(STFT 1024 / hop 512, symmetric Hann, positive spectral flux normalised by its
own maximum, adaptive median threshold over +-0.5 s, local maximum, 60 ms
minimum spacing, strongest 16 kept by strength then re-sorted, position 0 added
when the first hit is later than 50 ms, tail guard).

It reports the slices the firmware would end up with: slice i spans
[pos[i], pos[i+1]) and the last slice runs to the end of the sample. The
diagnostics underneath are the point of the script - they say what is wrong
with the result, not just what it is.
"""
import argparse
import math
import sys

import numpy as np

from wavio import read_wav

N = 1024
HOP = 512
BINS = N // 2 + 1


def detect(data, rate, ch, max_positions=16, frames=None):
    """Port of spectral_onsets(). Returns (positions, candidates, flux, radius)."""
    x = np.asarray(data, dtype=np.float32).reshape(-1, ch).mean(axis=1)
    if frames is None:
        frames = len(x)
    nf = (frames + HOP - 1) // HOP
    win = np.array([0.5 - 0.5 * math.cos(2.0 * math.pi * j / (N - 1))
                    for j in range(N)], dtype=np.float32)
    flux = np.zeros(nf, dtype=np.float32)
    prev = np.zeros(BINS, dtype=np.float32)
    for f in range(nf):
        seg = np.zeros(N, dtype=np.float32)
        a = f * HOP
        b = min(frames, a + N)
        if b > a:
            seg[:b - a] = x[a:b]
        mag = np.abs(np.fft.rfft(seg * win)).astype(np.float32)
        if f:
            d = mag - prev
            flux[f] = np.float32(d[d > 0].sum())
        prev = mag
    peak = float(flux.max())
    if peak > 0:
        flux = (flux / np.float32(peak)).astype(np.float32)

    radius = max(1, int(0.5 * rate / HOP + 0.5))
    gap = max(1, int(math.ceil(0.060 * rate / HOP)))

    candidates = []          # (pos, strength, frame_index)
    last = None
    for f in range(1, nf - 1):
        lo = f - radius if f > radius else 0
        hi = f + radius if f + radius < nf - 1 else nf - 1
        window = np.sort(flux[lo:hi + 1])
        median = float(window[len(window) // 2])
        threshold = median * 1.8
        if threshold < 0.30:
            threshold = 0.35
        if flux[f] < threshold or flux[f] < flux[f - 1] or flux[f] <= flux[f + 1]:
            continue
        pos = f * HOP + N // 2
        if pos + N // 2 >= frames:
            continue
        if last is not None and pos - last < gap * HOP:
            continue
        candidates.append((pos, float(flux[f]), f))
        last = pos

    detected = len(candidates)
    chosen = sorted(candidates, key=lambda c: c[1], reverse=True)[:max_positions]
    if chosen and chosen[0][0] > int(0.050 * rate + 0.5):
        if len(chosen) == max_positions:
            chosen.pop()
        chosen.append((0, 0.0, -1))
    chosen.sort(key=lambda c: c[0])
    return [c[0] for c in chosen], candidates, flux, radius, detected


def slices_from(positions, frames):
    out = []
    for i, p in enumerate(positions):
        end = positions[i + 1] if i + 1 < len(positions) else frames
        out.append((i, p, end, end - p))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input")
    ap.add_argument("--max", type=int, default=16)
    ap.add_argument("--png", default="")
    args = ap.parse_args()

    data, rate, ch = read_wav(args.input)
    frames = len(data) // ch
    positions, candidates, flux, radius, detected = detect(
        data, rate, ch, args.max, frames)
    dur = frames / rate

    print(f"file      {args.input}")
    print(f"format    {rate} Hz, {ch} ch, {frames} frames, {dur:.3f} s")
    print(f"analysis  1024/512, radius {radius} frames "
          f"({2 * radius * HOP / rate:.2f} s median window), "
          f"gap {int(math.ceil(0.060 * rate / HOP))} frames")
    print(f"onsets    {detected} candidates, {len(positions)} kept (cap {args.max})")
    print()

    print("kept onsets")
    print(f"{'#':>2} {'frame':>7} {'time':>8} {'ms':>7} {'strength':>8}")
    for i, p in enumerate(positions):
        st = next((c[1] for c in candidates if c[0] == p), 0.0)
        print(f"{i:>2} {p:>7} {p / rate:>8.4f} {p / rate * 1000:>7.1f} {st:>8.3f}")
    print()

    print("resulting slices")
    print(f"{'#':>2} {'start':>7} {'end':>7} {'frames':>7} {'ms':>7}")
    for i, a, b, n in slices_from(positions, frames):
        print(f"{i:>2} {a:>7} {b:>7} {n:>7} {n / rate * 1000:>7.1f}")
    print()

    dropped = [c for c in candidates if c[0] not in positions]
    print("diagnostics")
    if dropped:
        by_time = {}
        for pos, st, _ in dropped:
            by_time.setdefault(int(pos / rate), 0)
            by_time[int(pos / rate)] += 1
        print(f"  {len(dropped)} of {detected} candidates dropped by the "
              f"{args.max}-slice cap; their strength range "
              f"{min(c[1] for c in dropped):.3f}..{max(c[1] for c in dropped):.3f}")
        print("  dropped per second: "
              + ", ".join(f"{s}s:{n}" for s, n in sorted(by_time.items())))
        first = min(c[0] for c in dropped)
        print(f"  the earliest dropped hit is at frame {first} "
              f"({first / rate:.3f} s) - bar starts can lose their downbeat")
    short = [(i, n) for i, a, b, n in slices_from(positions, frames)
             if 0 < n < int(0.040 * rate)]
    if short:
        print("  slices shorter than 40 ms (inaudible stubs): "
              + ", ".join(f"#{i}={n / rate * 1000:.0f}ms" for i, n in short))
    silent = [(a, b) for i, a, b, n in slices_from(positions, frames)
              if n > int(1.0 * rate)]
    if silent:
        print(f"  {len(silent)} slice(s) longer than 1 s - audio in them will be "
              f"played from its start, not at the hit")
    if positions and positions[0] != 0:
        print(f"  first slice starts at frame {positions[0]} "
              f"({positions[0] / rate:.3f} s), not at 0")
    if len(positions) >= 2:
        gaps = np.diff(positions)
        print(f"  slice spacing: min {gaps.min() / rate * 1000:.0f} ms, "
              f"median {np.median(gaps) / rate * 1000:.0f} ms, "
              f"max {gaps.max() / rate * 1000:.0f} ms")
    hits = sorted(c[0] for c in candidates)
    if hits:
        covered = sum(1 for h in hits if any(
            a <= h < b for _, a, b, _ in slices_from(positions, frames)))
        print(f"  {covered} of {len(hits)} detected hits land inside a slice; "
              f"the rest are inside audio that some slice already covers")
    print()

    if args.png:
        draw(args.png, data, ch, frames, rate, positions, candidates, flux, dur)
        print(f"wrote {args.png}")


def draw(path, data, ch, frames, rate, positions, candidates, flux, dur):
    from PIL import Image, ImageDraw
    W, H = 1400, 420
    left, right = 10, W - 10
    wave_top, wave_h = 20, 250
    flux_top, flux_h = wave_top + wave_h + 30, 100

    x = np.asarray(data, dtype=np.float32).reshape(-1, ch).mean(axis=1)
    per = max(1, frames // (right - left))
    env = np.zeros(right - left, dtype=np.float32)
    for i in range(right - left):
        seg = x[i * per:(i + 1) * per]
        if len(seg):
            env[i] = np.abs(seg).max()
    env = env / max(1e-9, env.max())

    img = Image.new("RGB", (W, H), (12, 12, 14))
    d = ImageDraw.Draw(img)
    mid = wave_top + wave_h // 2
    d.rectangle([left, wave_top, right, wave_top + wave_h], outline=(60, 60, 66))
    for i, v in enumerate(env):
        h = int(v * (wave_h / 2 - 2))
        d.line([(left + i, mid - h), (left + i, mid + h)], fill=(210, 170, 40))
    d.rectangle([left, flux_top, right, flux_top + flux_h], outline=(60, 60, 66))
    step = max(1, len(flux) // (right - left))
    for i in range(right - left):
        v = float(flux[i * step:(i + 1) * step].max()) if i * step < len(flux) else 0.0
        h = int(v * (flux_h - 4))
        d.line([(left + i, flux_top + flux_h - 1),
                (left + i, flux_top + flux_h - 1 - h)], fill=(90, 140, 210))

    def xof(frame):
        return left + int(frame / max(1, frames) * (right - left))

    for pos, st, _ in candidates:
        xx = xof(pos)
        d.line([(xx, wave_top), (xx, wave_top + wave_h)], fill=(70, 70, 76))
    for p in positions:
        xx = xof(p)
        d.line([(xx, wave_top), (xx, flux_top + flux_h)], fill=(230, 230, 235), width=2)
    for s in range(0, int(dur) + 1):
        xx = xof(int(s * rate))
        d.line([(xx, wave_top + wave_h), (xx, wave_top + wave_h + 6)], fill=(120, 120, 126))
        d.text((xx + 3, wave_top + wave_h + 8), f"{s}s", fill=(150, 150, 156))
    img.save(path)


if __name__ == "__main__":
    sys.exit(main())
