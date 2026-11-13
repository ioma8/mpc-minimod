"""Verify PLUS shifts a physical 16 Levels pad by eight semitones."""
import json
import struct
import subprocess
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LAB = Path('/Users/jakubkolcar/projects/customs/virtual-mpc-sample')


def main():
    import sys
    sys.path.insert(0, str(LAB))
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

    def tap(name):
        for action in ('press', 'release'):
            request = urllib.request.Request(
                f'http://127.0.0.1:{lab.PORTS["panel"]}/control',
                json.dumps({'name': name, 'action': action, 'value': 100}).encode(),
                {'Content-Type': 'application/json'},
            )
            urllib.request.urlopen(request, timeout=5).read()
            time.sleep(.2)

    tap('SAMPLE')
    tap('PAD 1')
    if read('g_midi', '<i', 4) == 0:  # first field is the dispatcher flag; level flag follows at +4
        tap('16 LEVELS')
    before_calls = read('g_levels_pitch_calls', '<I')
    tap('PAD 1')
    baseline = read('g_levels_pitch_result', '<i')
    assert read('g_levels_pitch_calls', '<I') > before_calls, 'physical pad did not reach pitch hook'

    tap('PLUS')
    before_calls = read('g_levels_pitch_calls', '<I')
    tap('PAD 1')
    shifted = read('g_levels_pitch_result', '<i')
    assert read('g_levels_pitch_calls', '<I') > before_calls, 'shifted pad did not reach pitch hook'
    assert shifted - baseline == 8, f'PLUS should add 8 semitones: {baseline} -> {shifted}'

    tap('MINUS')
    before_calls = read('g_levels_pitch_calls', '<I')
    tap('PAD 1')
    lowered = read('g_levels_pitch_result', '<i')
    assert read('g_levels_pitch_calls', '<I') > before_calls, 'lowered pad did not reach pitch hook'
    assert lowered == baseline, f'MINUS should undo PLUS: {shifted} -> {lowered}'
    print(f'PASS: physical PAD 1 pitch offset, PLUS +8 and MINUS -8 ({baseline}, {shifted}, {lowered})')


if __name__ == '__main__':
    main()
