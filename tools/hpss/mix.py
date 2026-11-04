#!/usr/bin/env python3
"""Sum several aligned WAVs into one 24-bit mix.

    python3 tools/hpss/mix.py OUT.wav IN1.wav IN2.wav ... \
        [--start S] [--duration S] [--normalize] [--fade MS]

All inputs must share sample rate and channel count. Used to build the
drums+melody demo pads from the Project1 stems.
"""
import argparse

from wavio import fade_edges, normalize, read_wav, write_wav24


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("output")
    ap.add_argument("inputs", nargs="+")
    ap.add_argument("--start", type=float, default=0.0, help="seconds")
    ap.add_argument("--duration", type=float, default=0.0,
                    help="seconds; 0 means to the shortest input")
    ap.add_argument("--normalize", action="store_true", help="peak to -1 dBFS")
    ap.add_argument("--fade", type=float, default=10.0, help="fade length in ms")
    args = ap.parse_args()

    tracks = []
    sr = ch = None
    for path in args.inputs:
        data, s, c = read_wav(path)
        if sr is None:
            sr, ch = s, c
        elif (s, c) != (sr, ch):
            raise SystemExit(f"{path}: expected {sr}Hz/{ch}ch, got {s}Hz/{c}ch")
        tracks.append(data)

    total = min(len(t) for t in tracks) // ch
    a = max(0, int(args.start * sr))
    b = total if not args.duration else min(total, a + int(args.duration * sr))
    if b <= a:
        raise SystemExit("empty mix")

    out = [0.0] * ((b - a) * ch)
    for track in tracks:
        for i, v in enumerate(track[a * ch:b * ch]):
            out[i] += v
    out = fade_edges(out, ch, b - a, int(args.fade * sr / 1000.0))
    if args.normalize:
        out = normalize(out)
    write_wav24(args.output, out, sr, ch)
    print(f"wrote {args.output}: {(b - a) / sr:.3f}s {sr}Hz {ch}ch 24-bit")


if __name__ == "__main__":
    main()
