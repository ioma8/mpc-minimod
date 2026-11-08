"""Check the external MIDI chromatic hook in the running MPC emulator.

Preconditions: the mod is preloaded; MIDI Config has MIDI Port=External and
Pad MIDI In=On; a populated sample pad is selected in Sample Mode. The test
uses the panel's external-MIDI input endpoint, then reads the mod's internal
verification counters through /proc/<pid>/mem.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time
import urllib.request
import struct


MOD = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    args = parser.parse_args()
    sys.path.insert(0, str(args.lab))
    import lab

    def ssh(command):
        return subprocess.check_output(
            ['ssh', *lab.ssh_base(), 'root@' + lab.HOST, command], timeout=20)

    def control(name, action, value=None):
        body = {'name': name, 'action': action}
        if value is not None:
            body['value'] = value
        req = urllib.request.Request(
            args.panel + '/control', json.dumps(body).encode(),
            {'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=5) as response:
            assert json.load(response)['ok']
        time.sleep(.15)

    def tap(name):
        control(name, 'press', 127)
        control(name, 'release')

    def midi(hex_bytes):
        req = urllib.request.Request(
            args.panel + '/midi', json.dumps({'hex': hex_bytes}).encode(),
            {'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=5) as response:
            assert json.load(response)['ok']
        time.sleep(.12)

    pid = int(ssh('pidof "MPC Sample"'))
    maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
    app_base = int(next(line for line in maps if line.endswith('/MPC Sample')).split('-')[0], 16)
    mod_base = int(next(line for line in maps if line.endswith('/minimod.so')).split('-')[0], 16)
    symbols = {}
    for line in subprocess.check_output(['nm', '-a', str(MOD / 'build/minimod.elf'),
                                         ], text=True).splitlines():
        fields = line.split()
        if len(fields) >= 3:
            symbols[fields[-1]] = int(fields[0], 16)

    def read(address, size):
        data = ssh(f'dd if=/proc/{pid}/mem bs=1 skip={address} count={size} 2>/dev/null')
        assert len(data) == size, 'MPC process exited or memory read failed'
        return data

    def levels():
        """The 16 Levels flag: the second i32 of the mod's note-hook state."""
        return struct.unpack('<i', read(mod_base + symbols['g_midi'] + 4, 4))[0]

    def chromatic(field, signed=False):
        """A diagnostic counter at the tail of the same state."""
        offset = 160 + field * 4
        return struct.unpack('<i', read(mod_base + symbols['g_midi'] + offset, 4))[0] \
            if not signed else struct.unpack('<i', read(mod_base + symbols['g_midi'] + offset, 4))[0]

    def integer(name, size=4, signed=False):
        return int.from_bytes(read(mod_base + symbols[name], size), 'little', signed=signed)

    screen = integer('g_screen', 8)
    assert screen and int.from_bytes(read(screen, 8), 'little') == app_base + 0x6e212a0, \
        'Open Sample Mode before running this check'

    # The MIDI config is per boot. Keep the selected pad and make 16 Levels
    # active if the user has not already done so.
    tap('SAMPLE')
    tap('PAD 1')
    if not levels():
        tap('16 LEVELS')
    assert levels() == 1, '16 Levels did not become active'

    before = chromatic(0)
    for message in ('90 3c 64', '80 3c 00', '90 3d 64', '80 3d 00',
                    '90 43 64', '80 43 00'):
        midi(message)
    after = chromatic(0)
    assert after - before == 3, (
        f'Expected three transformed note-ons, got {after - before}; '
        'check that Pad MIDI In is On and MIDI Port is External')
    note = chromatic(1, signed=True)
    delta = chromatic(2, signed=True)
    assert (note, delta) == (67, 7), (note, delta)

    # The hook should pass notes through outside 16 Levels.
    tap('16 LEVELS')
    assert levels() == 0
    midi('90 45 64')  # A3
    assert chromatic(0) == after, \
        'MIDI note was changed outside 16 Levels'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'Firmware process restarted'
    assert b'NRestarts=0' in ssh('systemctl show ac50 -p NRestarts')
    print('PASS: C3/C#3/G3 were transposed by 0/+1/+7 semitones; '
          'outside-mode pass-through; no firmware restart')


if __name__ == '__main__':
    main()
