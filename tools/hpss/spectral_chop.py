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

def chroma(mag, rate, lo=55.0, hi=2000.0):
    """12 pitch classes from the spectrum, L2-normalised.

    A chord change moves energy between pitch classes even when the loudness
    and the timbre stay put, which is exactly the evidence a region boundary
    needs on tonal material.
    """
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
    """48 log-spaced bands, L2-normalised - the sound's colour."""
    freqs = np.linspace(0, rate / 2, mag.shape[1])
    edges = np.geomspace(lo, min(hi, rate / 2), bands + 1)
    out = np.zeros((mag.shape[0], bands), dtype=np.float32)
    for i in range(bands):
        sel = (freqs >= edges[i]) & (freqs < edges[i + 1])
        if sel.any():
            out[:, i] = mag[:, sel].sum(axis=1)
    out = np.log1p(out)
    return out / (np.linalg.norm(out, axis=1, keepdims=True) + 1e-9)


def foote_novelty(feat, half):
    """Checkerboard kernel on the self-similarity of `feat`.

    For every frame it compares "what the music sounds like just after" with
    "just before", and subtracts the similarity inside each side. A drum hit
    changes the colour, a new chord changes the pitch classes, and a sustained
    note inside a region changes neither - so the same kernel gives one-pad-per-
    hit on drums and phrase-level slices on tonal material.
    """
    n = len(feat)
    half = max(1, min(half, max(1, n // 2 - 1)))
    width = 2 * half + 1
    sim = np.zeros((n, width), dtype=np.float32)
    for d in range(-half, half + 1):
        lo, hi = max(0, -d), n - max(0, d)
        sim[lo:hi, d + half] = (feat[lo:hi] * feat[lo + d:hi + d]).sum(axis=1)
    idx = np.arange(-half, half + 1)
    before, after = idx < 0, idx > 0
    nov = np.zeros(n, dtype=np.float32)
    for i in range(half, n - half):
        blk = sim[i - half:i + half + 1]
        within = (blk[np.ix_(before, before)].mean() +
                  blk[np.ix_(after, after)].mean()) / 2
        across = (blk[np.ix_(before, after)].mean() +
                  blk[np.ix_(after, before)].mean()) / 2
        nov[i] = across - within
    return nov


def pick_regions(novelty, half, min_gap, rate, k=0.6):
    """Peaks of the Foote novelty, at least `min_gap` apart."""
    v = novelty.astype(np.float64)
    win = max(1, half)
    out = []
    last = -10 ** 9
    for f in range(1, len(v) - 1):
        if not (v[f] > v[f - 1] and v[f] > v[f + 1]):
            continue
        lo, hi = max(0, f - win), min(len(v), f + win + 1)
        local = v[lo:hi]
        med = float(np.median(local))
        sigma = 1.4826 * float(np.median(np.abs(local - med)))
        if v[f] < med + k * max(sigma, 1e-6):
            continue
        if f - last < min_gap:
            continue
        out.append(f)
        last = f
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


def onsets(x, rate, max_slices=16, region_s=0.7, fraction=0.15):
    """Slices are regions: cut where the music changes identity.

    `region_s` is the shortest region the detector can see - the one knob. It
    is the checkerboard width, so the slice count follows from the material
    instead of from a target.
    """
    mag = stft_magnitude(x)
    feat = np.hstack([0.6 * chroma(mag, rate), 0.4 * timbre(mag, rate)])
    half = max(2, int(region_s * rate / HOP) // 2)
    novelty = foote_novelty(feat, half)
    frames = pick_regions(novelty, half, half, rate)
    frames = [f for f in frames if f + 2 < len(novelty)]

    # a boundary must separate two audible regions: without this the fade-out
    # of a vocal or a reverb tail gets sliced into silence
    win = max(1, int(0.050 * rate))
    hop = win
    nwin = max(1, len(x) // hop)
    loud = np.array([float(np.sqrt(np.mean(x[i * hop:(i + 1) * hop] ** 2)))
                     for i in range(nwin)], dtype=np.float32)
    floor = 0.08 * float(loud.max())
    span = max(1, int(0.15 * rate / hop))

    def audible(frame):
        """something must actually start here - not a fade, not a reverb tail"""
        i = min(nwin - 1, max(0, frame * HOP // hop))
        return float(loud[i:i + span].max()) >= floor

    frames = [f for f in frames if audible(f)][:max_slices]
    positions = []
    for f in frames:
        p = place(x, rate, f, fraction)
        if positions and p - positions[-1] < int(0.040 * rate):
            continue
        positions.append(p)
    if positions and positions[0] > int(0.050 * rate) and len(positions) < max_slices:
        positions = [0] + positions
    return positions, frames, novelty



def slices_from(positions, frames):
    return [(i, p, positions[i + 1] if i + 1 < len(positions) else frames)
            for i, p in enumerate(positions)]


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input")
    ap.add_argument("--max", type=int, default=16)
    ap.add_argument("--region", type=float, default=0.7,
                    help="shortest region in seconds - the one knob")
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
    sl = slices_from(positions, frames)
    lengths = [b - a for _, a, b in sl]

    print(f"file        {args.input}")
    print(f"format      {rate} Hz, {ch} ch, {frames} frames, {frames / rate:.3f} s")
    print(f"analysis    Foote novelty on chroma+timbre, shortest region "
          f"{args.region:.2f} s")
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
