#!/usr/bin/env python3
"""Compare per-pad device captures against the model.

The lab's audio tap is phase-unstable (it drops chunks when nobody drains the host
port), so waveforms cannot be compared.  Magnitude can: the tap applies one fixed
gain and tilt to every take, so per-pad *band shape* and *envelope* are meaningful
even though absolute level is not.  Each pad is therefore scored on

  * envelope correlation after the best lag (does the pad breathe the same way?)
  * per-octave band level relative to the model, with the mean offset removed
    (is the spectral tilt right?) -- the offset itself is reported separately.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from mpc2stems import master, model, render, wav  # noqa: E402

BANDS = ((30, 60), (60, 120), (120, 250), (250, 500), (500, 1000),
         (1000, 2000), (2000, 4000), (4000, 8000), (8000, 16000))
HOP = 441


def envelope(x: np.ndarray) -> np.ndarray:
    mono = x.mean(axis=1) if x.ndim > 1 else x
    n = len(mono) // HOP
    return np.sqrt((mono[: n * HOP].reshape(n, HOP) ** 2).mean(axis=1) + 1e-20)


def band_levels(x: np.ndarray) -> np.ndarray:
    mono = x.mean(axis=1) if x.ndim > 1 else x
    spec = np.abs(np.fft.rfft(mono)) ** 2
    freqs = np.fft.rfftfreq(len(mono), 1 / 44100)
    norm = float(np.sum(mono ** 2)) or 1e-20
    out = []
    for lo, hi in BANDS:
        mask = (freqs >= lo) & (freqs < hi)
        out.append(float(spec[mask].sum()) / norm)
    return np.array(out)


def best_lag(env_dev: np.ndarray, env_mod: np.ndarray, max_lag: int = 400) -> int:
    n = min(len(env_dev), len(env_mod))
    a, b = env_mod[:n], env_dev[:n]
    a = a - a.mean()
    b = b - b.mean()
    lags = range(-max_lag, max_lag + 1)
    best, best_lag = -2.0, 0
    for lag in lags:
        if lag >= 0:
            x, y = a[: n - lag], b[lag:]
        else:
            x, y = a[-lag:], b[: n + lag]
        if len(x) < 20:
            continue
        denom = float(np.linalg.norm(x) * np.linalg.norm(y)) or 1e-20
        c = float(np.dot(x, y)) / denom
        if c > best:
            best, best_lag = c, lag
    return best_lag


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--captures", type=Path, default=Path("out-capture"))
    args = ap.parse_args(argv[1:])

    audio = args.captures / "audio"
    projects = args.captures / "projects"
    rows = []
    print(f"{'pad':>4} {'env corr':>9} {'lag ms':>7} {'level dB':>9} {'tilt dB':>8}  worst bands")
    for project in sorted(projects.glob("*-seq*.xpj")):
        pad = int(project.name.split("-")[1])
        stem = project.name.split("-seq")[0]
        take = audio / f"{stem}.wav"
        if not take.is_file():
            continue
        proj = model.load(project)
        stems, _ = render.render_pad_stems(proj, 0, pad_subset={pad})
        stems = master.apply_to_stems(stems, master.CompressorParams())
        mod = stems[pad]

        dev = wav.read_wav(take).samples.astype(float)
        if np.abs(dev).max() < 1e-6:
            print(f"{pad:4d} {'(silent capture)':>28}")
            continue

        lag = best_lag(envelope(dev), envelope(mod))
        env_d, env_m = envelope(dev), envelope(mod)
        if lag >= 0:
            x, y = env_m[: len(env_d) - lag], env_d[lag:]
        else:
            x, y = env_m[-lag:], env_d[: len(env_d) + lag]
        n = min(len(x), len(y))
        env_corr = float(np.corrcoef(x[:n], y[:n])[0, 1])

        bd, bm = band_levels(dev), band_levels(mod)
        db = 10 * np.log10((bd + 1e-12) / (bm + 1e-12))
        offset = float(np.median(db))
        tilt = db - offset
        spread = float(np.sqrt(np.mean(tilt ** 2)))
        worst = np.argsort(np.abs(tilt))[::-1][:2]
        where = ", ".join(f"{BANDS[i][0]}-{BANDS[i][1]}Hz {tilt[i]:+.1f}" for i in worst)
        rows.append((pad, env_corr, lag, offset, tilt, spread))
        print(f"{pad:4d} {env_corr:9.3f} {lag*HOP/44.1:7.1f} {offset:9.1f} {spread:8.1f}  {where}")

    if rows:
        print()
        print(f"mean envelope correlation {np.mean([r[1] for r in rows]):.3f}, "
              f"mean band spread {np.mean([r[5] for r in rows]):.1f} dB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
