"""Check every DATA turn through the real panel, including repeated menu opens.

The SAMPLE OPTIONS menu has seven rows; row geometry is measured from the
rendered screenshot rather than assumed.

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
    ROW0 = 43          # centre of the first SAMPLE OPTIONS row
    ROW_STEP = 20      # rows are 20 px apart (draw code: y + 26 + i * 20)
    ROWS = 8           # HPSS, Spectral, Reverb, Vinyl, Lo-Fi, Bass, Looper, Close
    HIGHLIGHT = (58, 42, 16)

    def selected():
        with urllib.request.urlopen(args.panel + '/screen.png', timeout=5) as response:
            im = Image.open(io.BytesIO(response.read())).convert('RGB')
        ys = [y for y in range(im.height) if im.getpixel((22, y)) == HIGHLIGHT]
        if not ys:
            return None
        # One highlighted band; its centre identifies the row.
        centre = (ys[0] + ys[-1]) / 2
        index = round((centre - ROW0) / ROW_STEP)
        return index if 0 <= index < ROWS else None
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
            # 01 turns down, 7F up; the menu wraps across all seven rows.
            expected = (expected + (1 if raw == 1 else ROWS - 1)) % ROWS
            wait_selection(expected)
        for knob in ('K1', 'K2', 'K3', 'FADER'):
            control(knob, 'set', 64)
            time.sleep(.1)
            assert selected() == expected, f'{knob} moved the menu'
        # Close through DATA to exercise button ownership as well as rotation.
        while expected != ROWS - 1:      # Close is the last row
            control('DATA', 'turn', 1)
            expected = (expected + 1) % ROWS
            wait_selection(expected)
        tap('DATA')
        wait_selection(None)
        print(f'PASS open {cycle+1}: 32 turns, knobs ignored, DATA closes', flush=True)


if __name__ == '__main__':
    main()
