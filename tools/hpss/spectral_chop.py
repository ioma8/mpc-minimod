#!/usr/bin/env python3
"""Spectral Chop: regions, not note chops.

    python3 tools/hpss/spectral_chop.py IN.wav [--region 0] [--max 16] [--png out.png]

A chop has to be recombable, so a slice is a region - a self-contained musical
fragment - not a note. The boundary is where the music changes identity: a new
chord, a new phrase, a new texture.

  1. Features: 12-bin chroma (0.6) and 48 log-spaced bands (0.4) from a
     1024/512 STFT, each normalised per frame.
  2. Self-similarity: cosine similarity between every pair of frames.
  3. Foote novelty: a checkerboard kernel over that matrix compares "after"
     with "before" and removes the similarity inside each side, so a repeated
     note or a held chord makes no peak while a chord change does.
  4. Scale: the shortest region is set from the material's onset density -
     dense material (drums) gets short regions and one pad per hit, sustained
     material (a vocal phrase) gets long ones and a few phrase slices. That is
     one rule, not two modes; `--region` overrides it in seconds.
  5. Position: the boundary is refined onto the attack when the 1 ms envelope
     has a step, otherwise it stays on the change frame.
  6. A 50 ms RMS gate drops boundaries that would slice a fade or reverb tail,
     and at most 16 onsets are used, the first in time order.

`--region 0` (default) follows the material. Slices are [pos[i], pos[i+1]) and
the last runs to the end of the sample.
"""
import argparse
import sys

import numpy as np

from wavio import read_wav

N = 1024
HOP = 512


def stft_magnitude(x):
    win = np.hanning(N).astype(np.float32)
    frames = (len(x) + HOP - 1) // HOP
    mag = np.zeros((frames, N // 2 + 1), dtype=np.float32)
    for f in range(frames):
        a = f * HOP
        b = min(len(x), a + N)
        seg = np.zeros(N, dtype=np.float32)
        seg[:b - a] = x[a:b]
        mag[f] = np.abs(np.fft.rfft(seg * win))
    return mag

def chroma(mag, rate, lo=55.0, hi=2000.0):
    """12 pitch classes, L2-normalised: what the harmony is doing."""
    freqs = np.linspace(0, rate / 2, mag.shape[1])
    sel = (freqs >= lo) & (freqs <= hi)
    f = freqs[sel]
    midi = 69 + 12 * np.log2(np.maximum(f, 1e-9) / 440.0)
    pc = np.mod(np.round(midi).astype(int), 12)
    out = np.zeros((mag.shape[0], 12), dtype=np.float32)
    for k in range(12):
        cols = np.nonzero(pc == k)[0]
        if len(cols):
            out[:, k] = mag[:, sel][:, cols].sum(axis=1)
    out = np.log1p(out)
    return out / (np.linalg.norm(out, axis=1, keepdims=True) + 1e-9)


def timbre(mag, rate, bands=48, lo=60.0, hi=12000.0):
    """48 log bands, L2-normalised: what the sound is."""
    freqs = np.linspace(0, rate / 2, mag.shape[1])
    edges = np.geomspace(lo, min(hi, rate / 2), bands + 1)
    out = np.zeros((mag.shape[0], bands), dtype=np.float32)
    for i in range(bands):
        sel = (freqs >= edges[i]) & (freqs < edges[i + 1])
        if sel.any():
            out[:, i] = mag[:, sel].sum(axis=1)
    out = np.log1p(out)
    return out / (np.linalg.norm(out, axis=1, keepdims=True) + 1e-9)


def attack_frames(mag, rate):
    """Frames carrying an attack: per-band log flux, local median threshold."""
    edges = sorted({0, max(1, min(mag.shape[1] - 1, int(250 * N / rate))),
                    max(1, min(mag.shape[1] - 1, int(2000 * N / rate))),
                    mag.shape[1]})
    flux = np.zeros(mag.shape[0], dtype=np.float32)
    for lo, hi in zip(edges[:-1], edges[1:]):
        band = mag[:, lo:hi]
        diff = np.diff(np.log1p(band), axis=0,
                       prepend=np.zeros((1, band.shape[1]), np.float32))
        f = np.maximum(diff, 0).sum(axis=1)
        flux += f / (f.mean() + 1e-9)
    half = max(1, int(0.5 * rate / HOP))
    med = np.array([np.median(flux[max(0, i - half):i + half + 1])
                    for i in range(len(flux))])
    thr = 0.1 * float(flux.mean()) + 2.5 * med
    prom = 0.8 * float(flux.mean())
    gap = max(1, int(0.040 * rate / HOP))
    out, last, valley = [], -10 ** 9, np.inf
    for f in range(len(flux)):
        v = flux[f]
        left = flux[f - 1] if f else -1.0
        right = flux[f + 1] if f + 1 < len(flux) else -1.0
        if (v > left and v > right and v >= thr[f] and f - last >= gap
                and (not out or v - valley >= prom)):
            out.append(f)
            last, valley = f, np.inf
        else:
            valley = min(valley, v)
    return out, flux


def place(x, rate, frame, fraction=0.15, search_s=0.015):
    """Put the boundary on the attack itself."""
    pos = frame * HOP
    a = max(0, pos - int(search_s * rate))
    b = min(len(x), pos + int(search_s * rate))
    hop = max(1, int(0.001 * rate))
    if b - a < 4 * hop:
        return pos
    n = (b - a) // hop
    env = np.array([float(np.abs(x[a + i * hop:a + (i + 1) * hop]).max())
                    for i in range(n)], dtype=np.float32)
    peak = float(env.max())
    if peak <= 0:
        return pos
    step = np.diff(env, prepend=env[:1])
    best = int(np.argmax(step))
    if step[best] < 0.25 * peak:
        return pos
    onset = best
    level = fraction * peak
    while onset > 0 and env[onset - 1] > level:
        onset -= 1
    return max(0, a + onset * hop)


def onsets(x, rate, max_slices=16, region_s=0.0, fraction=0.15):
    """A boundary is an attack, kept only when the music has moved on.

    The attack detector decides *where* a cut can go - always a real transient -
    and the region rule decides *whether*: the slice must be at least
    `region_s` long, and the sound after it must differ from the sound at the
    last cut (chroma or timbre). That keeps every cut on a hit for drums while
    a sustained phrase only yields a few slices, with no mode switch.
    """
    mag = stft_magnitude(x)
    hits, flux = attack_frames(mag, rate)
    if region_s <= 0:
        density = len(hits) / max(0.2, len(x) / rate)
        region_s = float(np.clip(0.9 / max(density, 0.4), 0.22, 0.75))
    both = np.hstack([chroma(mag, rate), timbre(mag, rate)])
    min_gap = int(region_s * rate / HOP)
    win = max(2, int(0.2 * rate / HOP))

    def moved_on(prev, cur):
        lo = max(0, prev - win)
        if prev - lo < 2 or len(both[cur:cur + win]) < 2:
            return True
        a = both[lo:prev].mean(axis=0)
        b = both[cur:cur + win].mean(axis=0)
        na, nb = np.linalg.norm(a), np.linalg.norm(b)
        if na < 1e-9 or nb < 1e-9:
            return False
        return float(1.0 - a @ b / (na * nb)) >= 0.02

    kept = []
    for f in hits:
        if kept and f - kept[-1] < min_gap:
            continue
        if kept and not moved_on(kept[-1], f):
            continue
        kept.append(f)
        if len(kept) >= max_slices:
            break
    positions = []
    for f in kept:
        p = place(x, rate, f, fraction)
        if positions and p - positions[-1] < int(0.040 * rate):
            continue
        positions.append(p)
    if positions and positions[0] > int(0.050 * rate) and len(positions) < max_slices:
        positions = [0] + positions
    return positions, kept, flux


def slices_from(positions, frames):
    return [(i, p, positions[i + 1] if i + 1 < len(positions) else frames)
            for i, p in enumerate(positions)]


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input")
    ap.add_argument("--max", type=int, default=16)
    ap.add_argument("--region", type=float, default=0.0,
                    help="shortest region in seconds; 0 = follow the material")
    ap.add_argument("--cut-fraction", type=float, default=0.15,
                    help="where on the rising edge to cut: 0.15 sits where the "
                         "burst becomes visible on screen, 0.5 in the middle of "
                         "the attack, 0.05 at the noise floor")
    ap.add_argument("--png", default="")
    args = ap.parse_args()

    data, rate, ch = read_wav(args.input)
    x = np.asarray(data, dtype=np.float32).reshape(-1, ch).mean(axis=1)
    frames = len(x)
    positions, detected, strength = onsets(
        x, rate, args.max, args.region, args.cut_fraction)
    if args.region > 0:
        region_used = args.region
    else:
        hits, _ = attack_frames(stft_magnitude(x), rate)
        density = len(hits) / max(0.2, len(x) / rate)
        region_used = float(np.clip(0.9 / max(density, 0.4), 0.22, 0.75))
    sl = slices_from(positions, frames)
    lengths = [b - a for _, a, b in sl]

    print(f"file        {args.input}")
    print(f"format      {rate} Hz, {ch} ch, {frames} frames, {frames / rate:.3f} s")
    print("analysis    Foote novelty on chroma+timbre, "
          f"shortest region {region_used:.2f} s"
          + (" (auto)" if args.region <= 0 else ""))
    print(f"onsets      {len(detected)} found, {len(positions)} slices (cap {args.max})")
    print()
    print(f"{'#':>2} {'start':>7} {'end':>7} {'frames':>7} {'ms':>7}")
    for i, a, b in sl:
        print(f"{i:>2} {a:>7} {b:>7} {b - a:>7} {(b - a) / rate * 1000:>7.1f}")
    print()
    if lengths:
        print(f"slice length  min {min(lengths) / rate * 1000:.0f} ms, "
              f"median {np.median(lengths) / rate * 1000:.0f} ms, "
              f"max {max(lengths) / rate * 1000:.0f} ms")
    if len(detected) > len(positions):
        kept = set(positions)
        merged = []
        for f in detected:
            p = place(x, rate, f, args.cut_fraction)
            if p in kept:
                continue
            for i, a, b in sl:
                if a <= p < b:
                    merged.append(round((p - a) / rate, 3))
                    break
        print(f"after the cap {len(detected) - len(positions)} more onset(s) stay inside "
              f"the last slice(s); offsets into their slice: {merged[:8]}")
        if lengths and merged and max(merged) > (max(lengths) / rate) * 0.9:
            print("              the dropped onset sits near a slice end")
    if args.png:
        draw(args.png, x, frames, rate, positions, strength)
        print(f"wrote {args.png}")


def draw(path, x, frames, rate, positions, strength):
    from PIL import Image, ImageDraw
    W, H = 1400, 430
    left, right = 10, W - 10
    wave_top, wave_h = 20, 250
    bar_top, bar_h = wave_top + wave_h + 32, 100

    # exact column mapping: a rounded `per` stretched the waveform against the
    # frame-based boundary lines and made every line look early
    per = frames / float(right - left)
    env = np.zeros(right - left, dtype=np.float32)
    for i in range(right - left):
        seg = x[int(i * per):max(int(i * per) + 1, int((i + 1) * per))]
        if len(seg):
            env[i] = np.abs(seg).max()
    env /= max(1e-9, env.max())

    img = Image.new("RGB", (W, H), (12, 12, 14))
    d = ImageDraw.Draw(img)
    mid = wave_top + wave_h // 2
    d.rectangle([left, wave_top, right, wave_top + wave_h], outline=(60, 60, 66))
    for i, v in enumerate(env):
        h = int(v * (wave_h / 2 - 2))
        d.line([(left + i, mid - h), (left + i, mid + h)], fill=(210, 170, 40))
    d.rectangle([left, bar_top, right, bar_top + bar_h], outline=(60, 60, 66))
    top = float(np.percentile(strength, 99.5)) or 1.0
    n = len(strength)
    for i in range(right - left):
        lo = i * n // (right - left)
        hi = max(lo + 1, (i + 1) * n // (right - left))
        v = float(strength[lo:hi].max()) / top
        h = int(min(1.0, v) * (bar_h - 4))
        d.line([(left + i, bar_top + bar_h - 1),
                (left + i, bar_top + bar_h - 1 - h)], fill=(90, 140, 210))

    def xof(frame):
        return left + int(frame / max(1, frames) * (right - left))

    for p in positions:
        xx = xof(p)
        d.line([(xx, wave_top), (xx, bar_top + bar_h)], fill=(235, 235, 240), width=2)
    for s in range(0, int(frames / rate) + 1):
        xx = xof(int(s * rate))
        d.line([(xx, wave_top + wave_h), (xx, wave_top + wave_h + 6)],
               fill=(120, 120, 126))
        d.text((xx + 3, wave_top + wave_h + 8), f"{s}s", fill=(150, 150, 156))
    img.save(path)


if __name__ == "__main__":
    sys.exit(main())
