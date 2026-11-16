# /// script
# requires-python = ">=3.10"
# ///
"""Check the Live Looper's per-pad lighting.

Empty pads are dim, recording is red, overdubbing amber, playing bright green
and a stopped loop dim green. Closing the looper restores the native colours.
"""
import argparse
import json
import time
import urllib.request

PADS = 16
EMPTY = [7, 7, 7]
REC = [127, 0, 0]
PLAY = [0, 127, 0]
DUB = [127, 88, 0]
STOP = [0, 28, 0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    args = parser.parse_args()

    def control(name, action, value=0):
        body = json.dumps({'name': name, 'action': action, 'value': value}).encode()
        request = urllib.request.Request(args.panel + '/control', data=body,
                                         headers={'Content-Type': 'application/json'})
        urllib.request.urlopen(request, timeout=5).read()
        time.sleep(.1)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def hold(name, other):
        control(name, 'press', 127)
        tap(other)
        control(name, 'release')

    def pads():
        with urllib.request.urlopen(args.panel + '/state', timeout=5) as reply:
            state = json.load(reply)
        return {int(key): value for key, value in state['pads'].items()
                if int(key) < PADS}

    def expect(pad, colour, label):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            got = pads()[pad]
            if got == colour:
                return
            time.sleep(.05)
        raise AssertionError(f'{label}: pad {pad + 1} is {got}, want {colour}')

    before = pads()

    # SHIFT + DATA opens the menu; row 6 is Live Looper.
    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    time.sleep(.5)
    for _ in range(6):
        control('DATA', 'turn', 1)
    tap('DATA')
    time.sleep(.8)

    # Clear anything a previous run left.
    control('ERASE', 'press', 127)
    tap('LOOP')
    control('ERASE', 'release')
    for pad in range(PADS):
        expect(pad, EMPTY, 'empty')

    tap('PAD 1')
    expect(0, REC, 'recording')
    time.sleep(.5)
    tap('PAD 1')
    time.sleep(.5)
    expect(0, PLAY, 'playing')

    hold('SHIFT', 'PAD 1')
    expect(0, DUB, 'overdubbing')
    hold('SHIFT', 'PAD 1')
    expect(0, PLAY, 'back to playing')

    tap('PAD 1')
    expect(0, STOP, 'stopped')

    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        after = pads()
        if after == before:
            break
        time.sleep(.05)
    for pad in range(PADS):
        assert after[pad] == before[pad], \
            f'pad {pad + 1}: native {before[pad]} became {after[pad]}'

    print('PASS: empty/rec/play/dub/stop colours and native restore')


if __name__ == '__main__':
    main()
