#!/usr/bin/env python3
"""Spectral Chop, simple and precise.

    python3 tools/hpss/spectral_chop.py IN.wav [--max 16] [--sensitivity 2.5]
                                            [--png out.png]

The whole algorithm is four steps:

  1. STFT: 1024-sample Hann frames, hop 512.
  2. Onset strength: spectral flux per band. The bins are split into
     low (<250 Hz), mid (250-2000 Hz) and high (>2 kHz); each band's positive
     change is divided by that band's own magnitude and the three curves are
     averaged. A kick, a snare and a hat therefore count the same, and a quiet
     hit counts as much as a loud one, with no scale constants.
  3. Peaks: a frame is an onset when its flux is the local maximum and exceeds
     `sensitivity` times the median flux of +-0.5 s around it. No absolute
     threshold, so it adapts to the material instead of to its loudest hit.
  4. Position: inside the frame that carries the attack, the 1 ms envelope is
     measured over +-20 ms and the boundary goes where the rising edge passes
     15% of the transient's peak - the first moment the hit is visible, so the
     slice starts on the attack with no silence in front and no clipped front.

Slices are what the firmware builds: slice i spans [pos[i], pos[i+1]) and the
last runs to the end of the sample. At most 16 slices are made: the first 16
onsets are used and everything after the sixteenth stays on the last pad.

`tools/hpss/onsets.py` is the reference for the *shipped* detector; this file
is the tuning bed for its replacement.

On the B3 break (4.03 s, 177706 frames) it produces 16 slices: fifteen between
83 and 390 ms with the boundary on the attack, and a last one carrying
everything that follows the sixteenth onset, exactly as the pad layout wants.

Two rules keep it honest: the threshold is `delta + lambda * local median`
with `delta` a tenth of the file's mean novelty, which stops a quiet passage
from firing on noise; and a peak must stand out from the valley since the
previous onset by 0.8x the mean novelty, which stops the decaying tail of a
loud hit from counting as a new one.
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


def hit_novelty(mag, rate):
    """Positive log-magnitude flux per band, each band normalised by its mean.

    Bands are split at 250 Hz and 2 kHz so a kick, a snare and a hat count the
    same, and a quiet hit counts as much as a loud one.
    """
    edges = sorted({0, max(1, min(mag.shape[1] - 1, int(250 * N / rate))),
                    max(1, min(mag.shape[1] - 1, int(2000 * N / rate))),
                    mag.shape[1]})
    out = np.zeros(mag.shape[0], dtype=np.float32)
    for lo, hi in zip(edges[:-1], edges[1:]):
        band = mag[:, lo:hi]
        diff = np.diff(np.log1p(band), axis=0,
                       prepend=np.zeros((1, band.shape[1]), np.float32))
        flux = np.maximum(diff, 0).sum(axis=1)
        out += flux / (flux.mean() + 1e-9)
    return out


def log_bands(mag, rate, bands=48, lo=40.0, hi=16000.0):
    """Sum the spectrum into log-spaced bands and take logs."""
    freqs = np.linspace(0, rate / 2, mag.shape[1])
    edges = np.geomspace(lo, min(hi, rate / 2), bands + 1)
    out = np.zeros((mag.shape[0], bands), dtype=np.float32)
    for i in range(bands):
        sel = (freqs >= edges[i]) & (freqs < edges[i + 1])
        if sel.any():
            out[:, i] = mag[:, sel].sum(axis=1)
    return np.log1p(out)


def change_novelty(mag, rate):
    """Cosine distance between consecutive log-band spectra.

    A drum hit changes the band vector by adding broadband energy; a legato
    note changes it by moving energy between partials. One measure, both kinds.
    """
    bands = log_bands(mag, rate)
    a, b = bands[:-1], bands[1:]
    na = np.linalg.norm(a, axis=1) + 1e-9
    nb = np.linalg.norm(b, axis=1) + 1e-9
    nov = np.concatenate(([0.0], 1.0 - (a * b).sum(axis=1) / (na * nb)))
    k = 3
    pad = np.pad(nov, (k // 2, k // 2), mode='edge')
    return np.convolve(pad, np.ones(k) / k, mode='valid')[:len(nov)]


def pick(novelty, rate, gate, min_gap_ms, prominence, lam):
    half = max(1, int(0.5 * rate / HOP))
    gap = max(1, int(min_gap_ms / 1000.0 * rate / HOP))
    prom = prominence * float(novelty.mean())
    out, last, valley = [], -10 ** 9, np.inf
    for f in range(len(novelty)):
        lo, hi = max(0, f - half), min(len(novelty), f + half + 1)
        win = novelty[lo:hi]
        med = float(np.median(win))
        if gate == 'mean':
            threshold = 0.1 * float(novelty.mean()) + lam * med
        else:                       # robust: stands out of the local spread
            threshold = med + lam * 1.4826 * float(np.median(np.abs(win - med)))
        v = novelty[f]
        left = novelty[f - 1] if f else -1.0
        right = novelty[f + 1] if f + 1 < len(novelty) else -1.0
        if (v > left and v > right and v >= threshold
                and f - last >= gap and (not out or v - valley >= prom)):
            out.append(f)
            last, valley = f, np.inf
        else:
            valley = min(valley, v)
    return out


def place(x, rate, frame, fraction=0.15):
    """Snap onto an attack when there is one, otherwise keep the frame."""
    pos = frame * HOP
    a = max(0, pos - int(0.012 * rate))
    b = min(len(x), pos + int(0.012 * rate))
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


def onsets(x, rate, max_slices=16, sensitivity=0.8, fraction=0.15):
    """Every boundary is an energy attack or a spectral change that is not one.

    Percussive material is carried by the attack detector, melodic material by
    the change detector; a change within 60 ms of an attack is the same event
    and is dropped.
    """
    mag = stft_magnitude(x)
    frames = len(mag)
    hits = pick(hit_novelty(mag, rate), rate, 'mean', 40.0, sensitivity, 2.5)
    changes = pick(change_novelty(mag, rate), rate, 'robust', 150.0, 2.5, 3.0)
    keep = [c for c in changes
            if c + 2 < frames and all(abs(c - h) * HOP > 0.060 * rate for h in hits)]
    merged = sorted(hits + keep)[:max_slices]
    positions = []
    for f in merged:
        p = place(x, rate, f, fraction)
        if positions and p - positions[-1] < int(0.040 * rate):
            continue
        positions.append(p)
    if positions and positions[0] > int(0.050 * rate) and len(positions) < max_slices:
        positions = [0] + positions
    return positions, merged, hit_novelty(mag, rate)


def slices_from(positions, frames):
    return [(i, p, positions[i + 1] if i + 1 < len(positions) else frames)
            for i, p in enumerate(positions)]


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input")
    ap.add_argument("--max", type=int, default=16)
    ap.add_argument("--sensitivity", type=float, default=2.5)
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
        x, rate, args.max, args.sensitivity, args.cut_fraction)
    sl = slices_from(positions, frames)
    lengths = [b - a for _, a, b in sl]

    print(f"file        {args.input}")
    print(f"format      {rate} Hz, {ch} ch, {frames} frames, {frames / rate:.3f} s")
    print(f"analysis    1024/512 Hann, sensitivity {args.sensitivity} over +-0.5 s, "
          f"40 ms minimum spacing")
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
