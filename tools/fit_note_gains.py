"""Estimate per-note gains by Gauss-Seidel fitting against the mixdown."""

from __future__ import annotations

import numpy as np

from mpc2stems import model
from mpc2stems.render import _load, _resample
from mpc2stems.wav import read_wav

ROOT = "project1/bapap1.xpj"
MIX = "project1-mixdown/Song1.wav"
LATENCY = 91


def main() -> None:
    proj = model.load(ROOT)
    mix = read_wav(MIX).samples
    by = proj.sequence_by_index
    sr = 44100

    layout: list[tuple[int, int]] = []
    cur = 0
    for si in proj.song:
        layout.append((si, cur))
        cur += by[si].length_pulses

    # gather notes in song order
    events: list[tuple[int, int, float, int]] = []  # (abs_pulse, pad, velocity, seq)
    for si, off in layout:
        for n in by[si].notes:
            events.append((off + n.time_pulses, n.pad_index, n.velocity, si))
    events.sort()

    n_frames = int(round(len(mix)))
    voices: list[tuple[int, np.ndarray]] = []
    meta: list[tuple[int, float, int]] = []
    per_pad_last: dict[int, int] = {}
    cache: dict[int, np.ndarray] = {}

    for idx, (pulse, pad, vel, si) in enumerate(events):
        pm = proj.pads[0][pad]
        if not pm.unmuted_layers:
            continue
        L = pm.unmuted_layers[0]
        key = pad
        if key not in cache:
            s = _load(str(proj.root / L.sample_file)).samples
            zone = s[L.zone_start : L.zone_end]
            if zone.shape[1] == 1:
                zone = np.repeat(zone, 2, axis=1)
            rate = 2.0 ** (L.cents / 1200.0)
            if rate == 1.0:
                cache[key] = zone
            else:
                cache[key] = _resample(zone, rate, int(np.ceil(zone.shape[0] / rate)), "linear")
        voice = cache[key]
        onset = int(round(pulse / 1760 * sr)) + LATENCY
        end = onset + len(voice)
        # monophonic: cut at next note on the same pad
        for j in range(idx + 1, len(events)):
            if events[j][1] == pad:
                end = min(end, int(round(events[j][0] / 1760 * sr)) + LATENCY)
                break
        end = min(end, n_frames)
        if end <= onset:
            continue
        voices.append((onset, voice[: end - onset].copy()))
        meta.append((pad, vel, onset))

    print(f"voices: {len(voices)}")

    target = mix[:, :2]
    mask = np.abs(target) < 0.9
    gains = np.ones(len(voices))

    # initial residual = target (unclipped only)
    for it in range(12):
        for i, (onset, v) in enumerate(voices):
            ln = len(v)
            seg = slice(onset, onset + ln)
            m = mask[seg]
            if m.sum() < 200:
                continue
            a = v[m]
            b = target[seg][m]
            denom = np.sum(a * a)
            if denom <= 0:
                continue
            num = np.sum(a * b)
            # remove current contribution from b
            b = b - a * gains[i]
            g = num / denom
            gains[i] = max(0.0, g)
        # report
        pred = np.zeros_like(target)
        for (onset, v), g in zip(voices, gains):
            pred[onset : onset + len(v)] += v * g
        m = mask
        c = np.corrcoef(pred[m].ravel(), target[m].ravel())[0, 1]
        print(f"iter {it}: corr(unclipped)={c:.4f}")

    np.save("/tmp/mpcwork/note_gains.npy", gains)
    rows = []
    for (pad, vel, onset), g in zip(meta, gains):
        pm = proj.pads[0][pad]
        L = pm.unmuted_layers[0]
        model_g = vel * L.volume * pm.volume * proj.track_mixes[0].volume * proj.master_volume * 0.7071
        rows.append((pad, vel, g, model_g, g / model_g if model_g else 0))
    rows.sort()
    print(f"{'pad':>3} {'vel':>6} {'gain':>8} {'model':>8} {'ratio':>7}  file")
    for pad, vel, g, mg, ratio in rows[:60]:
        L = proj.pads[0][pad].unmuted_layers[0]
        print(f"{pad:3d} {vel:6.3f} {g:8.3f} {mg:8.4f} {ratio:7.2f}  {L.sample_file[:34]}")


if __name__ == "__main__":
    main()
