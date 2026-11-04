#!/usr/bin/env python3
"""Compare two WAVs sample by sample.

    python3 tools/hpss/wavdiff.py A.wav B.wav

Prints max error and RMS error. Used to check the C port against hpss.py and to
measure the native producer's float32 rounding.
"""
import argparse
import math
import sys

from wavio import read_wav


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    args = ap.parse_args()
    a, sa, ca = read_wav(args.a)
    b, sb, cb = read_wav(args.b)
    if (sa, ca) != (sb, cb):
        sys.exit(f"format mismatch: {sa}Hz/{ca}ch vs {sb}Hz/{cb}ch")
    if len(a) != len(b):
        print(f"length mismatch: {len(a)} vs {len(b)} samples; comparing the common part")
    n = min(len(a), len(b))
    if not n:
        sys.exit("no samples")
    mx = 0.0
    ss = 0.0
    for x, y in zip(a[:n], b[:n]):
        d = x - y
        if abs(d) > mx:
            mx = abs(d)
        ss += d * d
    rms = math.sqrt(ss / n)
    print(f"frames={n // ca} ch={ca} rate={sa}")
    print(f"max_error={mx:.9g} rms_error={rms:.9g}")


if __name__ == "__main__":
    main()
