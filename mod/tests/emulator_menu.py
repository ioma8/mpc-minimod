"""Check every DATA turn through the real panel, including repeated menu opens.

Requires a running lab with the release mod and the Sample page visible.
Uses Pillow to inspect the rendered selection. Does not confirm processing.
"""
import argparse
import io
import json
import time
import urllib.request
from PIL import Image


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--panel', default='http://127.0.0.1:8765')
    args = p.parse_args()
    def control(name, action, value=0):
        req = urllib.request.Request(args.panel + '/control',
            data=json.dumps(dict(name=name, action=action, value=value)).encode(),
            headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=5) as response:
            assert json.load(response)['ok']
        time.sleep(.04)
    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')
    def selected():
        with urllib.request.urlopen(args.panel + '/screen.png', timeout=5) as response:
            im = Image.open(io.BytesIO(response.read())).convert('RGB')
        return next((i for i, y in enumerate((60, 108, 156))
                     if im.getpixel((22, y)) == (58, 42, 16)), None)
    def wait_selection(want):
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            value = selected()
            if value == want:
                return
            time.sleep(.03)
        raise AssertionError(f'Expected menu item {want}, observed {value}')
    def toggle_menu():
        control('SHIFT', 'press', 127)
        tap('DATA')
        control('SHIFT', 'release')
    if selected() is not None:
        toggle_menu()
        wait_selection(None)
    for cycle in range(3):
        toggle_menu()
        wait_selection(0)
        # Let unrelated property updates run before the first detent.
        time.sleep(.5)
        expected = 0
        for raw in [1] * 10 + [127] * 10 + [1, 127] * 5:
            control('DATA', 'turn', raw)
            expected = (expected + (1 if raw == 1 else 2)) % 3
            wait_selection(expected)
        for knob in ('K1', 'K2', 'K3', 'FADER'):
            control(knob, 'set', 64)
            time.sleep(.1)
            assert selected() == expected, f'{knob} moved the menu'
        # Close through DATA to exercise button ownership as well as rotation.
        while expected != 2:
            control('DATA', 'turn', 1)
            expected = (expected + 1) % 3
            wait_selection(expected)
        tap('DATA')
        wait_selection(None)
        print(f'PASS open {cycle+1}: 32 turns, knobs ignored, DATA closes', flush=True)


if __name__ == '__main__':
    main()
