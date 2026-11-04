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


def onset_density(x, rate):
    """Onsets per second - tells dense material from sustained material."""
    mag = stft_magnitude(x)
    diff = np.diff(np.log1p(mag), axis=0,
                   prepend=np.zeros((1, mag.shape[1]), np.float32))
    flux = np.maximum(diff, 0).sum(axis=1)
    half = max(1, int(0.5 * rate / HOP))
    med = np.array([np.median(flux[max(0, i - half):i + half + 1])
                    for i in range(len(flux))])
    thr = 0.1 * float(flux.mean()) + 2.5 * med
    peaks, last = [], -10 ** 9
    gap = max(1, int(0.040 * rate / HOP))
    for f in range(1, len(flux) - 1):
        if not (flux[f] > flux[f - 1] and flux[f] > flux[f + 1]):
            continue
        if flux[f] < thr[f] or f - last < gap:
            continue
        peaks.append(f)
        last = f
    return float(len(peaks)) / max(0.2, len(x) / rate)


def onsets(x, rate, max_slices=16, region_s=0.0, fraction=0.15):
    """Slices are regions: cut where the music changes identity.

    `region_s` is the shortest region the detector can see - the one knob. It
    is the checkerboard width, so the slice count follows from the material
    instead of from a target.
    """
    mag = stft_magnitude(x)
    if region_s <= 0:
        # dense material (drums) gets a shorter region, sustained material
        # (a vocal phrase) a longer one: one rule, no mode switch
        density = onset_density(x, rate)
        region_s = float(np.clip(0.9 / max(density, 0.4), 0.22, 0.75))
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
    density = 0.0 if args.region > 0 else None
    positions, detected, strength = onsets(
        x, rate, args.max, args.region, args.cut_fraction)
    region_used = args.region if args.region > 0 else float(
        np.clip(0.9 / max(onset_density(x, rate), 0.4), 0.22, 0.75))
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
