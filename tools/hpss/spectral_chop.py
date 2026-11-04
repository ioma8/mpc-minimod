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
  4. Position: the largest energy jump in a 1 ms envelope within +-30 ms of the
     flux frame, minus a 2 ms pre-roll, so the boundary sits on the attack
     instead of on the analysis frame (which is up to 11 ms late).

Slices are what the firmware builds: slice i spans [pos[i], pos[i+1]) and the
last runs to the end of the sample. At most 16 slices are made: the first 16
onsets are used and everything after the sixteenth stays on the last pad.

`tools/hpss/onsets.py` is the reference for the *shipped* detector; this file
is the tuning bed for its replacement.

On the B3 break (4.03 s, 177706 frames) it produces 16 slices: fifteen between
139 and 390 ms with the boundary on the attack, and a last one carrying
everything that follows the sixteenth onset, exactly as the pad layout wants.
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


def band_flux(mag, rate, edges=((0, 250), (250, 2000), (2000, None))):
    """One flux curve per band, each normalised by its own magnitude.

    A kick lives under 250 Hz, a snare in the mids, a hat above 2 kHz. Summing
    raw magnitudes lets the loudest band hide the others, so each band is
    normalised separately and then averaged.
    """
    freqs = np.linspace(0, rate / 2, mag.shape[1])
    out = []
    for lo, hi in edges:
        sel = (freqs >= lo) & (freqs < hi if hi else np.ones_like(freqs, bool))
        band = mag[:, sel]
        diff = np.diff(band, axis=0, prepend=np.zeros((1, band.shape[1]), np.float32))
        out.append(np.maximum(diff, 0).sum(axis=1) / (band.sum(axis=1) + 1e-9))
    return np.mean(out, axis=0)


def pick_peaks(strength, rate, sensitivity, min_gap_ms=40.0):
    half = max(1, int(0.5 * rate / HOP))
    local = np.array([np.median(strength[max(0, i - half):i + half + 1])
                      for i in range(len(strength))])
    gap = max(1, int(min_gap_ms / 1000.0 * rate / HOP))
    padded = np.concatenate(([-np.inf], strength, [-np.inf]))
    peaks, last = [], -10 ** 9
    for f in range(len(strength)):          # frame 0 can be the onset
        v = padded[f + 1]
        if v <= padded[f] or v <= padded[f + 2]:
            continue
        if v < sensitivity * local[f]:
            continue
        if f - last < gap:
            continue
        peaks.append(f)
        last = f
    return peaks


def snap_to_attack(x, rate, frame):
    """Put the boundary at the steepest energy rise near the flux peak.

    The ODF peak sits inside the first frame that contains the attack, so the
    exact sample is found from a 1 ms envelope: the largest jump in a +-30 ms
    window, minus a 2 ms pre-roll so the slice keeps the whole transient. A
    plain "first sample above a threshold" rule was jumping back onto the
    decaying tail of the previous hit.
    """
    pos = frame * HOP
    a = max(0, pos - int(0.030 * rate))
    b = min(len(x), pos + int(0.030 * rate))
    if b - a < 8:
        return pos
    hop = max(1, int(0.001 * rate))
    n = (b - a) // hop
    if n < 4:
        return pos
    env = np.array([float(np.abs(x[a + i * hop:a + (i + 1) * hop]).max())
                    for i in range(n)], dtype=np.float32)
    # rise over ~3 ms minus the preceding ~6 ms
    k = max(1, int(0.003 * rate / hop))
    m = max(1, int(0.006 * rate / hop))
    best, best_i = -1.0, 0
    for i in range(m, n):
        rise = env[i:i + k].max() - env[max(0, i - m):i].min()
        if rise > best:
            best, best_i = rise, i
    return max(0, a + best_i * hop - int(0.002 * rate))


def choose(frames, max_slices):
    """The first `max_slices` onsets, in time order.

    Everything after the last one stays inside the last slice - that is the
    point: sixteen chops from the start of the sample, the remainder on the
    sixteenth pad.
    """
    return frames[:max_slices]


def onsets(x, rate, max_slices=16, sensitivity=2.5):
    strength = band_flux(stft_magnitude(x), rate)
    detected = pick_peaks(strength, rate, sensitivity)
    frames = choose(detected, max_slices)
    ranked = frames
    positions = []
    for f in sorted(ranked):
        p = snap_to_attack(x, rate, f)
        if positions and p - positions[-1] < int(0.040 * rate):
            continue        # snapping can pull two peaks together
        positions.append(p)
    # cover a quiet head, but only when there is a slice to spare
    if positions and positions[0] > int(0.050 * rate) and len(positions) < max_slices:
        positions = [0] + positions
    return positions, detected, strength


def slices_from(positions, frames):
    return [(i, p, positions[i + 1] if i + 1 < len(positions) else frames)
            for i, p in enumerate(positions)]


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input")
    ap.add_argument("--max", type=int, default=16)
    ap.add_argument("--sensitivity", type=float, default=2.5)
    ap.add_argument("--png", default="")
    args = ap.parse_args()

    data, rate, ch = read_wav(args.input)
    x = np.asarray(data, dtype=np.float32).reshape(-1, ch).mean(axis=1)
    frames = len(x)
    positions, detected, strength = onsets(x, rate, args.max, args.sensitivity)
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
            p = snap_to_attack(x, rate, f)
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

    per = max(1, frames // (right - left))
    env = np.zeros(right - left, dtype=np.float32)
    for i in range(right - left):
        seg = x[i * per:(i + 1) * per]
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
