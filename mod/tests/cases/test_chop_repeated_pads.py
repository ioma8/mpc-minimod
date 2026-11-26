# /// script
# requires-python = ">=3.10"
# dependencies = ["pillow", "websocket-client"]
# ///
"""Verify native region playback after repeated physical pad hits."""
from emulator import Emulator
from PIL import Image
import io
import array
import math
from pathlib import Path
import struct
import threading
import time
import uuid
import wave
import websocket

PROJECT = 'populated'


def cursor_changes(first, second, pad):
    left = 15 + (pad - 1) * 72
    return sum(
        first.getpixel((x, y)) != second.getpixel((x, y))
        for x in range(left + 3, left + 69)
        for y in range(92, 176)
    )


def main():
    mpc = Emulator()
    name = f'000-chop-regression-{uuid.uuid4().hex}.wav'
    shared = Path(mpc.lab.SHARE) / name
    remote = f'/media/MPC SAMPLE/MPC-Sample/Samples/{name}'
    socket = None
    try:
        # A self-contained 16-second sample makes each of the four regions long
        # enough to observe its moving playback cursor.
        with wave.open(str(shared), 'wb') as output:
            output.setnchannels(1)
            output.setsampwidth(2)
            output.setframerate(44100)
            frame = b''.join(struct.pack('<h', int(10000 * math.sin(2 * math.pi * 440 * i / 44100)))
                             for i in range(44100))
            output.writeframes(frame * 16)
        mpc.ssh(f'cp "/lab/{name}" "{remote}"')
        mpc.tap('PAD 1')
        mpc.tap('SAMPLE SELECT')
        mpc.tap('SCREEN 2')
        mpc.control('DATA', 'turn', 1)
        mpc.tap('DATA')
        mpc.control('DATA', 'turn', -1)  # The folder selection carries into the list.
        loaded = ''
        for attempt in range(128):
            mpc.tap('DATA')
            time.sleep(.5)
            mpc.tap('SAMPLE')
            time.sleep(.2)
            layout = mpc.firmware_layout()
            screen = mpc.u64(mpc.symbol_address('g_screen'))
            binding = mpc.u64(screen + layout.page_binding)
            state = mpc.u64(binding + 0x60)
            layer = mpc.u64(state + layout.state_layers)
            sample = mpc.u64(layer + 0x4a8)
            holder = mpc.u64(sample + 0x30)
            path = mpc.u64(holder + 0x120)
            loaded = mpc.read(path, 512).split(b'\0', 1)[0].decode()
            if name.removesuffix('.wav') in loaded:
                break
            if attempt == 127:
                raise AssertionError(f'Generated test sample was not selectable; last loaded: {loaded}')
            mpc.tap('SAMPLE SELECT')
            mpc.control('DATA', 'turn', 1)
        mpc.tap('CHOP')
        time.sleep(1)
        mpc.control('K3', 'set', 32)  # Regions 4
        time.sleep(.5)
        socket = websocket.create_connection('ws://127.0.0.1:5557', timeout=5)
        chunks = []

        def read_audio():
            while True:
                try:
                    data = socket.recv()
                except (OSError, websocket.WebSocketException):
                    return
                if isinstance(data, bytes):
                    chunks.append((time.monotonic(), data))

        threading.Thread(target=read_audio, daemon=True).start()

        for round_number in range(21):
            for pad in range(1, 5):
                if round_number in (0, 20):
                    mpc.tap('STOP')
                    time.sleep(4.5 if round_number == 20 else .25)
                start = time.monotonic()
                mpc.control(f'PAD {pad}', 'press', 100)
                if round_number in (0, 20):
                    time.sleep(.2)
                    first = Image.open(io.BytesIO(mpc.screenshot())).convert('RGB')
                    time.sleep(.35)
                    second = Image.open(io.BytesIO(mpc.screenshot())).convert('RGB')
                    changes = cursor_changes(first, second, pad)
                    assert changes > 150, f'Round {round_number + 1}, pad {pad}: cursor did not move in its region ({changes} changed pixels)'
                    if round_number == 20:
                        samples = array.array('h')
                        for timestamp, data in chunks:
                            if start + .1 < timestamp < time.monotonic() and len(data) % 2 == 0:
                                samples.frombytes(data)
                        assert samples, f'No emulator audio received for pad {pad}'
                        rms = math.sqrt(sum(value * value for value in samples) / len(samples))
                        assert rms > 500, f'Pad {pad} was silent after repeated chops (RMS {rms:.0f})'
                else:
                    time.sleep(.3)
                mpc.control(f'PAD {pad}', 'release')
            print(f'Round {round_number + 1}: played four regions', flush=True)
        print('PASS: all four regions play audio and move their cursors after 21 rounds')
    finally:
        if socket is not None:
            socket.close()
        mpc.ssh(f'rm -f "{remote}"')
        shared.unlink(missing_ok=True)


if __name__ == '__main__':
    main()
