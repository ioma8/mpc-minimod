"""Compare a rendered model against the device mixdown, concisely."""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np

from mpc2stems import model
from mpc2stems.render import RenderParams, render_pad_stems
from mpc2stems.wav import read_wav

ROOT = Path(__file__).resolve().parents[1]
MIXDOWN = ROOT / "project1-mixdown" / "Song1.wav"
PROJECT = ROOT / "project1" / "bapap1.xpj"


def mixdown() -> np.ndarray:
    return read_wav(MIXDOWN).samples


def report(rendered: np.ndarray, target: np.ndarray, label: str = "") -> None:
    n = min(len(rendered), len(target))
    a = rendered[:n]
    b = target[:n]
    corr = []
    for c in range(2):
        d = np.dot(a[:, c], b[:, c])
        denom = np.sqrt(np.dot(a[:, c], a[:, c]) * np.dot(b[:, c], b[:, c]))
        corr.append(d / denom if denom else 0.0)
    scale = np.dot(a.ravel(), b.ravel()) / np.dot(a.ravel(), a.ravel())
    rms_a = np.sqrt(np.mean(a**2))
    rms_b = np.sqrt(np.mean(b**2))
    resid = a * scale - b
    print(
        f"{label:24s} corr L={corr[0]:.5f} R={corr[1]:.5f} rms(model)={rms_a:.4f} "
        f"rms(mix)={rms_b:.4f} best_scale={scale:.4f} resid_rms={np.sqrt(np.mean(resid**2)):.4f}"
    )


def main(argv: list[str]) -> int:
    proj = model.load(PROJECT)
    print("container", proj.container.version, proj.container.platform, "bpm", proj.bpm)
    print("song", proj.song, "sequences", [(s.index, s.bpm, s.length_pulses, len(s.notes)) for s in proj.sequences])
    print("pads with samples:", sorted(p.index for p in proj.pads[0].values() if p.unmuted_layers))
    target = mixdown()
    params = RenderParams()
    stems, n = render_pad_stems(proj, 0, params)
    total = sum(stems.values())
    report(total, target, "naive")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
