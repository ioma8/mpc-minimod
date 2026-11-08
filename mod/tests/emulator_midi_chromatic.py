"""Check the external MIDI chromatic hook in the running MPC emulator.

Preconditions: the mod is preloaded; MIDI Config has MIDI Port=External and
Pad MIDI In=On; a populated sample pad is selected in Sample Mode. The test
uses the panel's external-MIDI input endpoint, then reads the mod's internal
verification counters through /proc/<pid>/mem. The pitch delta is checked
against the sample's own root note (data+0x3e4), not against C3.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time
import urllib.request


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

    def integer(name, size=4, signed=False):
        return int.from_bytes(read(mod_base + symbols[name], size), 'little', signed=signed)

    screen = integer('g_screen', 8)
    assert screen and int.from_bytes(read(screen, 8), 'little') == app_base + 0x6e212a0, \
        'Open Sample Mode before running this check'

    # The MIDI config is per boot. Keep the selected pad and make 16 Levels
    # active if the user has not already done so.
    tap('SAMPLE')
    tap('PAD 1')
    if not integer('g_levels'):
        tap('16 LEVELS')
    assert integer('g_levels') == 1, '16 Levels did not become active'

    before = integer('g_midi_chromatic_count')
    for message in ('90 3c 64', '80 3c 00', '90 3d 64', '80 3d 00',
                    '90 43 64', '80 43 00'):
        midi(message)
    after = integer('g_midi_chromatic_count')
    assert after - before == 3, (
        f'Expected three transformed note-ons, got {after - before}; '
        'check that Pad MIDI In is On and MIDI Port is External')
    note = integer('g_midi_chromatic_note', signed=True)
    delta = integer('g_midi_chromatic_delta', signed=True)

    # The transform is measured from the sample's own root note. Pad 1 is A01,
    # the pad the tap above selects: surface -> instruments[pad] -> layers ->
    # sample -> data -> root.
    def qword(address):
        return int.from_bytes(read(address, 8), 'little')

    surface = integer('g_surface', 8)
    assert surface, 'The surface pointer is not set'
    array = qword(surface + 0x2808)
    state = qword(array)
    assert state, 'Pad 1 has no sample'
    layers = qword(state + 0x22e8)
    sample = qword(layers + 0x4a8)
    assert sample, 'Pad 1 has no sample'
    root = int.from_bytes(read(qword(sample + 0x130) + 0x3e4, 4), 'little', signed=True)
    assert 0 <= root <= 127, root
    assert (note, delta) == (67, 67 - root), (note, delta, root)

    # The hook should pass notes through outside 16 Levels.
    tap('16 LEVELS')
    assert integer('g_levels') == 0
    midi('90 45 64')  # A3
    assert integer('g_midi_chromatic_count') == after, \
        'MIDI note was changed outside 16 Levels'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'Firmware process restarted'
    assert b'NRestarts=0' in ssh('systemctl show ac50 -p NRestarts')
    print(f'PASS: C3/C#3/G3 were transposed by 0/+1/+7 from the sample root '
          f'(root {root}, G3 delta {67 - root}); outside-mode pass-through; '
          'no firmware restart')


if __name__ == '__main__':
    main()
