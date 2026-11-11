"""Render a Bass C2 on a zero-sample project already open in the emulator.

Start the mod with Sample Mode visible and an empty project loaded. The test
checks all pad slots, output geometry and pitch, the native modal, and PID.
"""
import argparse
import json
import math
from pathlib import Path
import struct
import subprocess
import sys
import time
import urllib.request

import panel_screen

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    parser.add_argument('--pad', type=int, choices=range(1, 17), default=1)
    args = parser.parse_args()
    sys.path.insert(0, str(args.lab))
    import lab

    def ssh(command):
        return subprocess.check_output(['ssh', *lab.ssh_base(), 'root@' + lab.HOST,
                                        command], timeout=20)

    def control(name, action, value=0):
        request = urllib.request.Request(
            args.panel + '/control', data=json.dumps(dict(name=name, action=action,
                                                          value=value)).encode(),
            headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            assert json.load(response)['ok']
        time.sleep(.15)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    symbols = {}
    for line in subprocess.check_output(['nm', '-a', str(ROOT / 'build/minimod.elf')],
                                        text=True).splitlines():
        fields = line.split()
        if len(fields) == 3:
            symbols[fields[2]] = int(fields[0], 16)
    pid = int(ssh('pidof "MPC Sample"'))
    maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
    base = int(next(line for line in maps if line.endswith('/minimod.so')).split('-')[0], 16)
    restarts = ssh('systemctl show ac50 -p NRestarts')

    def read(address, size):
        result = ssh(f'dd if=/proc/{pid}/mem bs=1 skip={address} count={size} 2>/dev/null')
        assert len(result) == size, 'firmware exited or memory read failed'
        return result

    def qword(address):
        return int.from_bytes(read(address, 8), 'little')

    surface = qword(base + symbols['g_surface'])
    first, end = qword(surface + 0x2808), qword(surface + 0x2810)
    assert first and end - first >= 128 * 8, 'pad table unavailable'

    def populated():
        result = []
        for pad in range(128):
            state = qword(first + 8 * pad)
            if state:
                layers = qword(state + 0x22e8)
                if layers and qword(layers + 0x4a8):
                    result.append(pad)
        return result

    assert not populated(), 'test requires all pads empty'
    tap(f'PAD {args.pad}')
    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    for _ in range(5):
        control('DATA', 'turn', 1)
    tap('DATA')
    tap('DATA')
    status_address = base + symbols['g_hpss_status']
    deadline = time.monotonic() + 12
    status = ''
    while time.monotonic() < deadline:
        status = read(status_address, 96).split(b'\0', 1)[0].decode()
        if status == 'Synth: copy created':
            break
        time.sleep(.2)
    assert status == 'Synth: copy created', status
    time.sleep(1.5)  # Let the menu close and the Sample page repaint.
    panel_screen.assert_no_modal(args.panel, 'Synth')
    screen = qword(base + symbols['g_screen'])
    state = qword(qword(screen + 0x178) + 0x60)
    destination_state = qword(first + (args.pad - 1) * 8)
    layer = qword(destination_state + 0x22e8)
    assert layer, 'destination pad has no layer'
    active = read(layer + 0xb18, 1)[0]
    assert active == 1, f'destination layer is inactive: {active}'
    sample = qword(layer + 0x4a8)
    assert sample, 'selected pad has no sample'
    metadata = qword(sample + 0x130)
    channels, frames, rate = struct.unpack('<HxxII', read(metadata, 12))
    assert (channels, frames, rate) == (2, 33075, 44100), (channels, frames, rate)
    holder = qword(sample + 0x170)
    buffer = qword(qword(holder + 0x28))
    values = struct.unpack('<' + 'f' * (frames * channels),
                           read(buffer, frames * channels * 4))
    rms = (sum(value * value for value in values) / len(values)) ** .5
    assert rms > 1e-4 and max(values) - min(values) > 1e-3, f'silent or DC: {rms}'
    mono = [(left + right) * .5 for left, right in zip(values[::2], values[1::2])]
    powers = []
    for hz in range(50, 86):
        coefficient = 2 * math.cos(2 * math.pi * hz / rate)
        previous = previous2 = 0.0
        for index, value in enumerate(mono):
            value *= .5 - .5 * math.cos(2 * math.pi * index / (frames - 1))
            current = value + coefficient * previous - previous2
            previous2, previous = previous, current
        powers.append((previous**2 + previous2**2 - coefficient * previous * previous2, hz))
    pitch = max(powers)[1]
    assert abs(pitch - 65.406) < 5, f'expected C2, measured {pitch} Hz'
    assert populated() == [args.pad - 1], 'another pad was populated'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'firmware restarted'
    assert ssh('systemctl show ac50 -p NRestarts') == restarts, 'firmware restarted'
    print(f'PASS: pad {args.pad}, active layer, {frames} stereo frames at {rate} Hz, '
          f'RMS {rms:.5f}, peak {pitch} Hz, no restart')


if __name__ == '__main__':
    main()
