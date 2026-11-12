"""Check the synth page's purple chromatic pad gradient in the emulator.

Pad 1 is off, pad 16 is full brightness, and every pad shares the same purple
hue. Closing the synth page restores the app's native pad colours.
"""
import argparse
import json
import time
import urllib.request

PURPLE = 0x009B30FF
PADS = 16


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', default='/Users/jakubkolcar/projects/customs/virtual-mpc-sample')
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    args = parser.parse_args()
    del args.lab

    def control(name, action, value=0):
        body = json.dumps({'name': name, 'action': action, 'value': value}).encode()
        request = urllib.request.Request(args.panel + '/control', data=body,
                                         headers={'Content-Type': 'application/json'})
        urllib.request.urlopen(request, timeout=5).read()
        time.sleep(.12)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def pads():
        with urllib.request.urlopen(args.panel + '/state', timeout=5) as reply:
            state = json.load(reply)
        return {int(k): v for k, v in state['pads'].items()}

    tap('SAMPLE')
    time.sleep(.6)
    before = pads()

    # SHIFT + DATA opens the menu; row 5 is Synth; DATA opens its page.
    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    time.sleep(.5)
    for _ in range(5):
        control('DATA', 'turn', 1)
    tap('DATA')
    time.sleep(1.0)

    lit = pads()
    for pad in range(PADS):
        brightness = pad / (PADS - 1)
        expected = [
            int(((PURPLE >> 16) & 0xFF) * brightness) >> 1,
            int(((PURPLE >> 8) & 0xFF) * brightness) >> 1,
            int((PURPLE & 0xFF) * brightness) >> 1,
        ]
        assert lit[pad] == expected, f'pad {pad + 1}: {lit[pad]} != {expected}'
    assert lit[0] == [0, 0, 0], lit[0]
    assert lit[PADS - 1] != [0, 0, 0], lit[PADS - 1]

    # SHIFT + DATA closes the synth page.
    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    time.sleep(1.0)
    after = pads()
    for pad in range(PADS):
        assert after[pad] == before[pad], f'pad {pad + 1}: {before[pad]} -> {after[pad]}'

    print('PASS: purple gradient, pad 1 off, pad 16 full, native colours restored')


if __name__ == '__main__':
    main()
