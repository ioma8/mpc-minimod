"""Fit the MPC master compressor using the documented parameter ranges.

From the MPC manual:
  Attack  0.100 - 150 ms   (K1)
  Release 3.0 - 300 ms     (K2)
  Amount  0.00 - 100 %     (K3)  -- combines threshold and ratio
  In Boost (SHIFT+K3)      -- 0 in this project
  Color    parallel bass boost + saturation + pitch instability
  Makeup gain is computed automatically from input level and settings.
"""

from __future__ import annotations

import numpy as np

from mpc2stems import model, render
from mpc2stems.wav import read_wav

ROOT = "project1/bapap1.xpj"
MIX = "project1-mixdown/Song1.wav"
SR = 44100
ATTACK_MIN, ATTACK_MAX = 0.1, 150.0
RELEASE_MIN, RELEASE_MAX = 3.0, 300.0


def knob_log(lo: float, hi: float, pct: float) -> float:
    return lo * (hi / lo) ** (pct / 100.0)


def knob_lin(lo: float, hi: float, pct: float) -> float:
    return lo + (hi - lo) * (pct / 100.0)


def box_rms(x: np.ndarray, w: int) -> np.ndarray:
    w = max(8, int(w))
    cs = np.concatenate(([0.0], np.cumsum(x * x)))
    n = len(x)
    i = np.arange(n)
    lo = np.maximum(i - w, 0)
    hi = np.minimum(i + 1, n)
    return np.sqrt((cs[hi] - cs[lo]) / np.maximum(hi - lo, 1) + 1e-18)


def gain_reduction(x: np.ndarray, thr: float, ratio: float, atk: float, rel: float) -> np.ndarray:
    level = 20.0 * np.log10(box_rms(x, int(0.005 * SR)) + 1e-12)
    target = np.maximum(0.0, level - thr) * (1.0 - 1.0 / ratio)
    a = np.exp(-1.0 / (max(atk, 0.01) * 1e-3 * SR))
    r = np.exp(-1.0 / (max(rel, 0.01) * 1e-3 * SR))
    out = np.empty_like(target)
    g = 0.0
    for i in range(len(target)):
        t = target[i]
        c = a if t > g else r
        g = c * g + (1.0 - c) * t
        out[i] = g
    return out


def apply_comp(dry, target, thr, ratio, atk, rel, automatic_makeup=True):
    gr = gain_reduction(dry, thr, ratio, atk, rel)
    makeup = float(gr.mean()) if automatic_makeup else 0.0
    y = dry * (10.0 ** ((-gr + makeup) / 20.0))
    ok = np.abs(target) < 0.9
    scale = float(np.sum(y[ok] * target[ok]) / np.sum(y[ok] * y[ok]))
    return np.clip(y * scale, -1, 1), scale, gr


def main() -> None:
    proj = model.load(ROOT)
    ref = read_wav(MIX).samples[:, 0]
    params = render.RenderParams(gate="none", filter_cutoff_scale=1.0, filter_q_scale=0.0)
    stems, n = render.render_pad_stems(proj, 0, params)
    m = min(min(v.shape[0] for v in stems.values()), len(ref))
    dry = sum(stems[p][:m, 0] for p in stems)
    target = ref[:m]
    ok = np.abs(target) < 0.9
    plain = float(np.sqrt(np.mean((dry * np.sum(dry * target) / np.sum(dry * dry) - target) ** 2)))
    print(f"plain scaling residual {plain:.5f}")

    attacks = {
        "attack 90% log ": knob_log(ATTACK_MIN, ATTACK_MAX, 90),
        "attack 90% lin ": knob_lin(ATTACK_MIN, ATTACK_MAX, 90),
    }
    releases = {
        "release 80% log": knob_log(RELEASE_MIN, RELEASE_MAX, 80),
        "release 80% lin": knob_lin(RELEASE_MIN, RELEASE_MAX, 80),
    }
    print({k: round(v, 1) for k, v in attacks.items()}, {k: round(v, 1) for k, v in releases.items()})

    best = (plain, None)
    for aname, atk in attacks.items():
        for rname, rel in releases.items():
            for thr in (-40, -30, -24, -18, -12, -6, 0):
                for ratio in (1.5, 2, 3, 4, 6, 8, 12, 20):
                    y, scale, gr = apply_comp(dry, target, thr, ratio, atk, rel)
                    r = float(np.sqrt(np.mean((y - target) ** 2)))
                    if r < best[0]:
                        best = (r, (aname, rname, thr, ratio, atk, rel, scale, float(gr.mean()), float(gr.max())))
    if best[1]:
        an, rn, thr, ratio, atk, rel, scale, gm, gx = best[1]
        print(f"\nbest residual {best[0]:.5f}")
        print(f"  {an} = {atk:.1f} ms, {rn} = {rel:.1f} ms, threshold {thr} dB, ratio {ratio}")
        print(f"  auto makeup {gm:.2f} dB, extra gain {20*np.log10(scale):+.2f} dB, max GR {gx:.1f} dB")


if __name__ == "__main__":
    main()
