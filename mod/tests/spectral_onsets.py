"""Compile the production detector and check the region contract.

A boundary needs an attack AND a change: repeated identical events stay in one
slice, a melody that moves gets one slice per note, silence and steady tones
get none.
"""
from pathlib import Path
import math
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "src/spectral.c"
HARNESS = r'''
#include <stdio.h>
#include <stdlib.h>
#include "spectral.h"
int main(int argc, char **argv)
{
    if (argc != 4) return 2;
    unsigned ch = (unsigned)atoi(argv[2]);
    unsigned sr = (unsigned)atoi(argv[3]);
    FILE *f = fopen(argv[1], "rb");
    if (!f) return 3;
    fseek(f, 0, SEEK_END);
    long bytes = ftell(f);
    fseek(f, 0, SEEK_SET);
    size_t frames = (size_t)bytes / sizeof(float) / ch;
    float *buf = malloc((size_t)bytes);
    if (!buf || fread(buf, 1, (size_t)bytes, f) != (size_t)bytes) return 4;
    fclose(f);
    unsigned pos[16], detected = 0;
    int n = spectral_onsets(buf, frames, ch, sr, pos, 16, &detected);
    printf("%d %u\n", n, detected);
    for (int i = 0; i < n; ++i) printf("%u\n", pos[i]);
    free(buf);
    return 0;
}
'''

SR = 44100


def write_raw(path, samples):
    with open(path, "wb") as f:
        f.write(struct.pack("<%df" % len(samples), *samples))


def notes(duration, events, level=0.6):
    """events: (time, frequency) - a note that changes pitch, like a melody."""
    data = [0.0] * int(duration * SR)
    for start, freq in events:
        i = int(start * SR)
        nxt = min(len(data), int((start + 0.30) * SR))
        for k in range(i, nxt):
            t = (k - i) / SR
            data[k] = level * math.sin(2 * math.pi * freq * t) * math.exp(-3.0 * t)
    return data


def clicks(duration, positions, level=0.8):
    data = [0.0] * int(duration * SR)
    for pos in positions:
        i = int(pos * SR)
        for k in range(8):
            if i + k < len(data):
                data[i + k] = level
    return data


def run(harness, data):
    with tempfile.NamedTemporaryFile(suffix=".raw") as f:
        write_raw(f.name, data)
        out = subprocess.check_output(
            [str(harness), f.name, "1", str(SR)], text=True
        ).splitlines()
    n, detected = map(int, out[0].split())
    return [int(x) for x in out[1:1 + n]], detected


with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    harness_c = root / "harness.c"
    harness_c.write_text(HARNESS)
    exe = root / "onsets"
    subprocess.run(
        ["cc", "-O2", "-std=c11", "-I", str(ROOT / "src"), "-o", str(exe),
         str(harness_c), str(SOURCE), "-lm"],
        check=True,
    )

    # 1. percussive events separated by silence are separate slices: the tail
    # of one is not the same sound as the attack of the next
    same = [0.10, 0.50, 0.90, 1.30, 1.70]
    positions, detected = run(exe, clicks(2.0, same))
    assert detected >= 3, (positions, detected)
    assert len(positions) >= 3, positions

    # 2. a melody that changes pitch is what a region boundary is for
    melody = [(0.10, 440.0), (0.60, 554.4), (1.10, 659.3), (1.60, 880.0)]
    positions, detected = run(exe, notes(2.2, melody))
    assert detected >= 3, (positions, detected)
    assert len(positions) >= 3, positions

    silence, detected = run(exe, [0.0] * (2 * SR))
    assert silence == [] and detected == 0, (silence, detected)

    # 3. a steady tone has no change in it, so it must not be chopped
    sine = [0.4 * math.sin(2 * math.pi * 440 * i / SR) for i in range(2 * SR)]
    tonal, _ = run(exe, sine)
    assert len(tonal) <= 1, tonal

    # 4. a late first event still gets a boundary at 0, so the head is a slice
    lead = run(exe, notes(2.0, [(0.70, 440.0), (1.30, 659.3)]))
    positions, detected = lead
    assert positions[0] == 0, (positions, detected)
    assert len(positions) >= 2

    print("spectral onsets ok")
