#!/usr/bin/env python3
"""Measure the lab's own audio-path response, so per-pad captures can be compared.

Every take goes through one fixed path (rear routing gain 50 plus whatever the
bridge does), and that path is not flat.  Using the one case where the model is
already validated -- the full mix of the reference project -- the path response is

    H(f) = |device mix| / |model mix|

and dividing it out of every per-pad take leaves a comparison that only reflects
pad rendering, not the tap.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from mpc2stems import master, model, render, wav  # noqa: E402

EDGES = np.array([20, 40, 60, 80, 120, 160, 250, 400, 630, 1000, 1600, 2500,
                  4000, 6300, 10000, 16000, 20000], dtype=float)


def band_spectrum(x: np.ndarray, edges: np.ndarray = EDGES) -> np.ndarray:
    mono = x.mean(axis=1) if x.ndim > 1 else x
    spec = np.abs(np.fft.rfft(mono * np.hanning(len(mono)))) ** 2
    freqs = np.fft.rfftfreq(len(mono), 1 / 44100)
    out = []
    for lo, hi in zip(edges[:-1], edges[1:]):
        mask = (freqs >= lo) & (freqs < hi)
        out.append(float(spec[mask].sum()))
    return np.array(out)


def main(argv: list[str]) -> int:
    mix_take = Path(argv[1] if len(argv) > 1 else "/tmp/orig20.wav")
    project = Path("project1/bapap1.xpj")

    device = wav.read_wav(mix_take).samples.astype(float)
    proj = model.load(project)
    stems, _ = render.render_pad_stems(proj, 0)
    stems = master.apply_to_stems(stems, master.CompressorParams())
    mix = sum(stems.values())

    d = band_spectrum(device)
    m = band_spectrum(mix)
    # ignore bands the model barely puts energy in: their ratio is noise
    keep = m > m.max() * 1e-7
    db = 10 * np.log10((d + 1e-20) / (m + 1e-20))
    centres = np.sqrt(EDGES[:-1] * EDGES[1:])
    print(f"{'band Hz':>12} {'relay dB':>9}")
    for c, v, k in zip(centres, db, keep):
        if k:
            print(f"{c:12.0f} {v:9.1f}")
    Path("out-capture/relay-response.json").write_text(
        json.dumps({"centres_hz": [float(c) for c, k in zip(centres, keep) if k],
                    "gain_db": [float(v) for v, k in zip(db, keep) if k]}, indent=1)
    )
    print("\nwrote out-capture/relay-response.json")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
