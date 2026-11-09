"""Check AutoTrim against the live model in the running MPC emulator.

AutoTrim is SHIFT+B3 on the Sample page. The mod calls the app's own control
dispatcher with frame positions; the command writes the pad's zone and the
renderer. This test reads those bounds straight out of the app's memory, so it
needs no logging build and no fixed fixture beyond a pad that has a sample.

Model path (firmware 1.3.0.12):

    screen            qword(libbase + g_screen), vtable checked
    screen + 0x178    pad controls binding
    binding + 0x60    pad state of the *selected* pad
    state + 0x22e8    vector<Layer> begin; layer 0 is the selected pad's layer
    layer + 0x4a8     the Sample; sample + 0x130 -> meta, meta + 4 = frames
    layer + 0x7b8     trim start frame      (u32)
    layer + 0x810     trim end frame        (u32)
    layer + 0x868     trim start mirror     (u32)

SHIFT+B3 cycles 3 %, 6 %, 9 % of peak and then clears the trim; each press must
tighten the bounds, and the fourth must restore the whole sample.
"""
import argparse
import importlib.util
import json
import struct
import subprocess
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--pad', type=int, default=1, help='pad number 1..16')
    parser.add_argument('--library', type=Path, default=ROOT / 'build/minimod.so')
    parser.add_argument('--mapfile', type=Path, default=ROOT / 'build/minimod.map')
    args = parser.parse_args()

    symbols = {}
    for line in args.mapfile.read_text().splitlines():
        fields = line.split()
        if fields and fields[-1] == 'g_screen':
            symbols['g_screen'] = int(fields[0], 16)
    assert 'g_screen' in symbols, 'g_screen missing from the linker map'

    spec = importlib.util.spec_from_file_location('lab', args.lab / 'lab.py')
    lab = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(lab)
    panel = f'http://{lab.HOST}:{lab.PORTS["panel"]}'

    def ssh(command):
        return subprocess.check_output(['ssh', *lab.ssh_base(), 'root@' + lab.HOST,
                                        command], timeout=20)

    def control(name, action, value=0):
        body = json.dumps(dict(name=name, action=action, value=value)).encode()
        req = urllib.request.Request(panel + '/control', data=body,
                                     headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=10) as response:
            assert json.load(response)['ok'], (name, action)
        time.sleep(.06)

    def press(name):
        control(name, 'press', 100)
        control(name, 'release')

    def leds():
        with urllib.request.urlopen(panel + '/state?since=0', timeout=5) as r:
            return json.load(r)['leds']

    pid = int(ssh('pidof "MPC Sample"').split()[0])
    maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
    mapping = next(x for x in maps if x.endswith('/' + args.library.name))
    libbase = int(mapping.split('-')[0], 16)
    stock = int(next(x.split('-')[0] for x in maps if x.endswith('/MPC Sample')), 16)

    def memory(address, size):
        data = ssh(f'dd if=/proc/{pid}/mem bs=1 skip={address} count={size} 2>/dev/null')
        assert len(data) == size, f'App died or address {address:#x} went invalid'
        return data

    def qword(address):
        return struct.unpack('<Q', memory(address, 8))[0]

    def u32(address):
        return struct.unpack('<I', memory(address, 4))[0]

    # Put the Sample page in front, in Full (not Chop) mode, on the target pad.
    press('SAMPLE')
    time.sleep(1.2)
    press('SCREEN 1')
    time.sleep(.5)
    if leds().get('75', 0) == 2:
        press('CHOP')
        time.sleep(.5)
    press(f'PAD {args.pad}')
    time.sleep(.6)

    screen = qword(libbase + symbols['g_screen'])
    assert qword(screen) == stock + 0x6e212a0, 'SampleModeScreen not found'
    state = qword(qword(screen + 0x178) + 0x60)
    layer = qword(state + 0x22e8)
    sample = qword(layer + 0x4a8)
    assert sample, f'Pad {args.pad} has no sample'
    frames = u32(qword(sample + 0x130) + 4)
    assert frames > 1, f'Pad {args.pad} sample has no audio'

    def bounds():
        return u32(layer + 0x7b8), u32(layer + 0x810), u32(layer + 0x868)

    def press_trim():
        # The cycle lives only while SHIFT stays held: releasing it resets the
        # step counter, so a whole cycle is one hold and N B3 presses.
        press('SCREEN 3')
        time.sleep(.9)

    print(f'pad {args.pad}: {frames} frames, pid {pid}')

    # Clear whatever trim the pad had: four presses in one hold.
    control('SHIFT', 'press', 127)
    for _ in range(4):
        press_trim()
    control('SHIFT', 'release')
    time.sleep(.4)
    start, end, mirror = bounds()
    assert (start, end) == (0, frames - 1), \
        f'Clearing should restore 0..{frames - 1}, got {start}..{end}'
    assert mirror == start, 'Start mirror out of sync'

    # One hold: 3 %, 6 %, 9 %, then clear again.
    control('SHIFT', 'press', 127)
    previous = None
    for step, percent in enumerate((3, 6, 9), start=1):
        press_trim()
        start, end, mirror = bounds()
        assert 0 <= start < end <= frames - 1, f'Step {step}: bad bounds {start}..{end}'
        assert mirror == start, f'Step {step}: start mirror {mirror} != {start}'
        assert end < frames - 1 or start > 0, f'Step {step}: {percent}% trimmed nothing'
        if previous is not None:
            assert end <= previous[1] and start >= previous[0], \
                f'Step {step} did not tighten: {start}..{end} vs {previous}'
        print(f'PASS step {step}: {percent}% -> {start}..{end} of {frames}')
        previous = (start, end)

    press_trim()  # fourth press of the hold clears the trim
    start, end, mirror = bounds()
    assert (start, end) == (0, frames - 1), \
        f'Fourth press should clear, got {start}..{end}'
    control('SHIFT', 'release')
    print(f'PASS clear: {start}..{end}')

    assert int(ssh('pidof "MPC Sample"').split()[0]) == pid, 'App restarted'
    print('PASS: cycle, tightening, clear, start mirror, no restart')


if __name__ == '__main__':
    main()
