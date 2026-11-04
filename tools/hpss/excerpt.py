#!/usr/bin/env python3
"""Cut a WAV excerpt, optionally fade and normalise it, write 24-bit PCM.

    python3 tools/hpss/excerpt.py IN.wav OUT.wav START DUR [--normalize] [--fade MS]

Used to build the manual HPSS demo pads from the Project1 mixdown.
"""
import argparse

from wavio import fade_edges, normalize, read_wav, write_wav24


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("start", type=float, help="seconds")
    ap.add_argument("duration", type=float, help="seconds")
    ap.add_argument("--normalize", action="store_true", help="peak to -1 dBFS")
    ap.add_argument("--fade", type=float, default=10.0, help="fade length in ms")
    args = ap.parse_args()

    data, sr, ch = read_wav(args.input)
    frames = len(data) // ch
    a = max(0, int(args.start * sr))
    b = min(frames, a + int(args.duration * sr))
    if b <= a:
        raise SystemExit("empty excerpt")
    out = data[a * ch:b * ch]
    out = fade_edges(out, ch, b - a, int(args.fade * sr / 1000.0))
    if args.normalize:
        out = normalize(out)
    write_wav24(args.output, out, sr, ch)
    print(f"wrote {args.output}: {(b - a) / sr:.3f}s {sr}Hz {ch}ch 24-bit")


if __name__ == "__main__":
    main()
