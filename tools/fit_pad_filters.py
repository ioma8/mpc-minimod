"""Jointly identify each filtered pad's filter against the reference mixdown.

Per-pad fitting is degenerate here: the five filtered pads are quiet relative to
the model's error, so the best single-pad "filter" is simply to mute the pad.
Instead we coordinate-descend on the *global* residual with all other pads fixed.
"""

from __future__ import annotations

import numpy as np
from scipy.signal import sosfiltfilt

from mpc2stems import model, render
from mpc2stems.wav import read_wav

ROOT = "project1/bapap1.xpj"
MIX = "project1-mixdown/Song1.wav"
SR = 44100
FILTERED = (13, 16, 17, 20, 27)


def biquad(kind: str, fc: float, q: float, fs: int = SR) -> np.ndarray:
    fc = min(max(fc, 20.0), fs * 0.45)
    q = max(q, 0.3)
    w0 = 2.0 * np.pi * fc / fs
    cw, sw = np.cos(w0), np.sin(w0)
    alpha = sw / (2.0 * q)
    if kind == "lp":
        b = np.array([(1 - cw) / 2, 1 - cw, (1 - cw) / 2])
    elif kind == "hp":
        b = np.array([(1 + cw) / 2, -(1 + cw), (1 + cw) / 2])
    else:
        b = np.array([alpha, 0.0, -alpha])
    a = np.array([1 + alpha, -2 * cw, 1 - alpha])
    return np.array([[b[0] / a[0], b[1] / a[0], b[2] / a[0], 1.0, a[1] / a[0], a[2] / a[0]]])


def filtered(x: np.ndarray, kind: str, poles: int, fc: float, q: float) -> np.ndarray:
    sos = biquad(kind, fc, q)
    if poles == 4:
        sos = np.vstack([sos, sos])
    return sosfiltfilt(sos, x)


KINDS = [
    ("off", None),
    ("classic", ("lp", 2)),
    ("lpf2", ("lp", 2)),
    ("lpf4", ("lp", 4)),
    ("hpf2", ("hp", 2)),
    ("hpf4", ("hp", 4)),
    ("bpf2", ("bp", 2)),
    ("bpf4", ("bp", 4)),
]
CUTOFFS = np.geomspace(60, 16000, 13)
QS = (0.5, 1.0, 2.0, 4.0, 8.0)


def main() -> None:
    proj = model.load(ROOT)
    ref = read_wav(MIX).samples[:, 0]
    P = render.RenderParams(gate="none", filter_enabled=False)
    stems, n = render.render_pad_stems(proj, 0, P)
    m = min(min(v.shape[0] for v in stems.values()), len(ref))
    target = ref[:m]
    ok = np.abs(target) < 0.9
    pad_signal = {p: stems[p][:m, 0] for p in FILTERED}
    fixed = sum(stems[p][:m, 0] for p in stems if p not in FILTERED)

    choice = {p: ("off", 0.0, 0.0) for p in FILTERED}
    cache = {p: pad_signal[p].copy() for p in FILTERED}

    def build(p, spec, fc, q):
        if spec is None:
            return pad_signal[p]
        return filtered(pad_signal[p], spec[0], spec[1], fc, q)

    def score(y):
        r = target[ok] - y[ok]
        return float(np.sqrt(np.mean(r * r)))

    start = score(fixed + sum(cache.values()))
    print(f"start residual (no pad filters): {start:.5f}")
    for sweep in range(2):
        for p in FILTERED:
            others = fixed + sum(cache[q] for q in FILTERED if q != p)
            best = (score(others + cache[p]), choice[p])
            for name, spec in KINDS:
                grid = [(0.0, 0.0)] if spec is None else [(float(fc), q) for fc in CUTOFFS for q in QS]
                for fc, q in grid:
                    y = build(p, spec, fc, q)
                    r = score(others + y)
                    if r < best[0]:
                        best = (r, (name, fc, q))
            choice[p] = best[1]
            cache[p] = build(p, dict(KINDS)[choice[p][0]], choice[p][1], choice[p][2])
        print(f"sweep {sweep+1}: residual {score(fixed + sum(cache.values())):.5f}")

    print()
    for p in FILTERED:
        st = proj.pads[0][p]
        name, fc, q = choice[p]
        print(f"pad {p:2d} stored type {(st.filter or {}).get('filterType')}: best {name:8s} "
              f"cutoff {fc:6.0f} Hz  Q {q:.1f}   {st.unmuted_layers[0].sample_file[:30]}")


if __name__ == "__main__":
    main()
