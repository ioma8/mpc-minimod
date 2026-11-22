# /// script
# requires-python = ">=3.10"
# ///
"""Verify the microtonal 16 Levels grid, toggled by holding 16 LEVELS and turning DATA.

The firmware's pitch setter writes one property slot per call: slot 0 carries the
semitones and slot 1 the fine tune, whose step is a tenth of a semitone. Halving
the grid therefore cannot show up as a fraction of a semitone: the pad's offset
loses one semitone and gains five fine steps, so the row spans half the semitones
it normally does.

The expected values are derived from the same pads measured on the whole-semitone
grid first, so the case does not depend on where the 16 Levels anchor sits.

The fine slot is what the app's Tune page's K2 knob drives, and the pitch it
produces was measured through the lab's PCM stream: five steps is an audible half
step (ratio 1.029), while 50 steps is a tritone.
"""
import argparse
import json
import struct
import subprocess
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PADS = ('PAD 1', 'PAD 2', 'PAD 3')


def half(value):
    """The (semitones, fine steps) one whole-semitone offset becomes when halved."""
    return (value // 2, (value % 2) * 5)


def half_steps(pitch):
    """Total half steps of a (semitones, fine steps) pair; five steps are a half."""
    semitones, fine = pitch
    return 2 * semitones + fine // 5


def main():
    import sys
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, default=Path(__file__).resolve().parents[3] / 'virtual-mpc-sample')
    args = parser.parse_args()
    sys.path.insert(0, str(args.lab))
    import lab

    pid = int(lab.guest('pidof "MPC Sample"').decode().split()[0])
    maps = lab.guest(f'cat /proc/{pid}/maps').decode().splitlines()
    first = next(line for line in maps if line.endswith('/run/mpclab-preload/minimod.so'))
    base = int(first.split('-')[0], 16)
    symbols = {}
    for line in subprocess.check_output(['nm', '-a', str(ROOT / 'build/minimod.elf')], text=True).splitlines():
        fields = line.split()
        if len(fields) >= 3:
            symbols[fields[-1]] = int(fields[0], 16)

    def read(name, fmt, offset=0):
        address = base + symbols[name] + offset
        raw = lab.guest(f'dd if=/proc/{pid}/mem bs=1 skip={address} count=4 2>/dev/null')
        return struct.unpack(fmt, raw)[0]

    def control(name, action, value=0):
        request = urllib.request.Request(
            f'http://127.0.0.1:{lab.PORTS["panel"]}/control',
            json.dumps({'name': name, 'action': action, 'value': value}).encode(),
            {'Content-Type': 'application/json'},
        )
        urllib.request.urlopen(request, timeout=5).read()
        time.sleep(.15)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def microtonal(direction):
        """Hold 16 LEVELS and turn DATA: +1 halves the grid, -1 restores it."""
        control('16 LEVELS', 'press', 127)
        control('DATA', 'turn', direction)
        control('16 LEVELS', 'release')

    def pitch_of(pad):
        """The semitone and cents slots the pitch hook produced for one pad."""
        before = read('g_levels_pitch_calls', '<I')
        tap(pad)
        deadline = time.monotonic() + 3
        while read('g_levels_pitch_calls', '<I') == before:
            if time.monotonic() > deadline:
                raise AssertionError(f'{pad} did not reach the pitch hook')
            time.sleep(.05)
        return (read('g_levels_pitch_result', '<i'),
                read('g_levels_fine_result', '<i'))

    def grid():
        return {pad: pitch_of(pad) for pad in PADS}

    tap('SAMPLE')
    tap('PAD 1')
    if read('g_midi', '<i', 4) == 0:  # the 16 Levels flag follows the dispatcher flag
        tap('16 LEVELS')

    # Start from the whole-semitone grid whatever the previous run left behind:
    # the off direction normalises the mode, and an already-off grid is a no-op.
    microtonal(-1)
    whole = grid()
    assert all(fine == 0 for _, fine in whole.values()), f'the whole grid has no fine tune: {whole}'
    assert whole['PAD 2'][0] - whole['PAD 1'][0] == 1, f'one semitone per pad: {whole}'

    microtonal(1)
    micro = grid()
    for pad in PADS:
        expected = half(whole[pad][0])
        assert micro[pad] == expected, (
            f'microtonal {pad}: expected {expected} for a whole offset of '
            f'{whole[pad][0]}, got {micro[pad]}')
    assert any(fine == 5 for _, fine in micro.values()), (
        f'the halved grid has to land on a half semitone: {micro}')
    assert half_steps(micro['PAD 2']) - half_steps(micro['PAD 1']) == 1, (
        f'two adjacent pads are half a semitone apart: {micro}')
    assert half_steps(micro['PAD 3']) - half_steps(micro['PAD 2']) == 1, (
        f'two adjacent pads are half a semitone apart: {micro}')

    microtonal(-1)
    back = grid()
    for pad in PADS:
        assert back[pad] == whole[pad], (
            f'the other turn restores whole semitones on {pad}: {whole[pad]} -> {back[pad]}')
    print(f'PASS: hold 16 LEVELS + DATA halves the grid {whole} -> {micro} -> {back}')


if __name__ == '__main__':
    main()
