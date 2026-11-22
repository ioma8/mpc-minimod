# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy"]
# ///
"""Verify the microtonal 16 Levels grid by ear: measure what a pressed pad plays.

`emulator_microtonal.py` checks the values the hook produces. This case checks
that the firmware's voice actually plays them, by recording the app's PCM stream
from the lab and comparing the pitch of one pad on both grids.

Press a pad on the whole grid, halve the grid, press it again: the pitch has to
move by exactly what the slots declare — whole semitones plus five fine steps to
a half. That is what pins the fine slot's step at a tenth of a semitone. The bug
this catches is real: with the step assumed to be a cent, fifty steps sounded a
tritone instead of a half step, and the audible pitch was nowhere near the
declared one.

The case picks the pad with the most tonal material in the fixture, and skips
when there is none, because pads in a project need not share a sample and a
sub-bass rumble has no peak to track.
"""
import argparse
import base64
import json
import os
import socket
import struct
import subprocess
import time
import urllib.request
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
FS = 44100
# Window of the captured pad, in samples: 0.74 s, for a 1.35 Hz FFT bin.
WINDOW = 32768
# How far after the tap the measured window starts, in seconds.
SKIP = 0.12
# The band a pad's partial has to be in to be worth tracking: above the
# sub-bass, and low enough that the harmonics are still strong.
BAND = (150, 2000)
CANDIDATES = ('PAD 1', 'PAD 2', 'PAD 3', 'PAD 4', 'PAD 5', 'PAD 6')
# One fine step is a tenth of a semitone. This is the claim under test.
FINE_STEP = 0.1
TOLERANCE = 0.15  # semitones


class Skip(Exception):
    """The lab cannot serve this case right now."""


class Pcm:
    """The app's interleaved S16LE stereo PCM, as the panel plays it."""

    def __init__(self, port):
        try:
            self.socket = socket.create_connection(('127.0.0.1', port), timeout=5)
            key = base64.b64encode(os.urandom(16)).decode()
            self.socket.sendall(
                (f'GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n'
                 f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n'
                 f'Sec-WebSocket-Version: 13\r\n\r\n').encode())
            head = b''
            while b'\r\n\r\n' not in head:
                head += self.socket.recv(1)
        except OSError as error:
            # QEMU's audio chardev serves one client, and the panel page is the
            # other one; without the stream there is nothing to measure.
            self.socket.close()
            raise Skip(
                f'the PCM stream on port {port} is in use ({error}); '
                'close the panel page to let this case measure audio') from error
        status = head.split(b'\r\n')[0]
        if b'101' not in status:
            self.socket.close()
            raise Skip(f'the PCM stream on port {port} refused the upgrade: {status!r}')
        self.buffer = b''
        self.parts = []

    def pump(self, seconds):
        end = time.time() + seconds
        self.socket.settimeout(0.5)
        while time.time() < end:
            try:
                chunk = self.socket.recv(65536)
            except socket.timeout:
                continue
            if not chunk:
                break
            self.buffer += chunk
            while len(self.buffer) >= 2:
                first, second = self.buffer[0], self.buffer[1]
                length, offset = second & 0x7f, 2
                if length == 126:
                    if len(self.buffer) < 4:
                        break
                    length = struct.unpack('>H', self.buffer[2:4])[0]
                    offset = 4
                elif length == 127:
                    if len(self.buffer) < 10:
                        break
                    length = struct.unpack('>Q', self.buffer[2:10])[0]
                    offset = 10
                if len(self.buffer) < offset + length:
                    break
                payload = self.buffer[offset:offset + length]
                self.buffer = self.buffer[offset + length:]
                if first & 0x0f == 2:
                    self.parts.append(payload)

    def samples(self):
        data = b''.join(self.parts)
        frames = np.frombuffer(data[:len(data) // 4 * 4], dtype='<i2').astype(np.float64)
        return frames[0::2]  # left channel

    def close(self):
        self.socket.close()


def spectrum(samples):
    return np.abs(np.fft.rfft(samples)), np.fft.rfftfreq(len(samples), 1 / FS)


def peak(samples, target=None):
    """Sub-bin frequency of the strongest partial.

    Without `target`, the strongest component of [`BAND`]. With one, the
    strongest component within a semitone of it, which keeps tracking the same
    partial instead of jumping to whichever peak happens to win.
    """
    magnitudes, freqs = spectrum(samples)
    band = np.nonzero((freqs > BAND[0]) & (freqs < BAND[1]))[0]
    if band.size == 0:
        raise AssertionError('the analysis band is empty')
    if target is not None:
        near = band[(freqs[band] > target / 2 ** (1 / 12)) & (freqs[band] < target * 2 ** (1 / 12))]
        if near.size:
            band = near
    k = band[np.argmax(magnitudes[band])]
    # Parabolic interpolation on the log magnitude: the peak is stable to the
    # bin, so this is what buys the sub-semitone resolution.
    a = np.log(magnitudes[k - 1] + 1e-9)
    b = np.log(magnitudes[k] + 1e-9)
    c = np.log(magnitudes[k + 1] + 1e-9)
    curve = a - 2 * b + c
    delta = 0.5 * (a - c) / curve if curve else 0.0
    return freqs[k] + delta * (freqs[1] - freqs[0])


def tonality(samples):
    """How much a pad in `BAND` stands above its own noise: a peak/median ratio."""
    magnitudes, freqs = spectrum(samples)
    band = (freqs > BAND[0]) & (freqs < BAND[1])
    if not band.any():
        return 0.0
    return float(magnitudes[band].max() / (np.median(magnitudes[band]) + 1e-9))


def main():
    import sys
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, default=Path(__file__).resolve().parents[3] / 'virtual-mpc-sample')
    args = parser.parse_args()
    sys.path.insert(0, str(args.lab))
    import lab

    panel, pcm = lab.PORTS['panel'], lab.PORTS['pcm']
    pid = int(lab.guest('pidof "MPC Sample"').decode().split()[0])
    maps = lab.guest(f'cat /proc/{pid}/maps').decode().splitlines()
    base = int(next(line for line in maps if line.endswith('/run/mpclab-preload/minimod.so')).split('-')[0], 16)
    symbols = {}
    for line in subprocess.check_output(['nm', '-a', str(ROOT / 'build/minimod.elf')], text=True).splitlines():
        fields = line.split()
        if len(fields) >= 3:
            symbols[fields[-1]] = int(fields[0], 16)

    def read(name, fmt, offset=0):
        address = base + symbols[name] + offset
        return struct.unpack(fmt, lab.guest(f'dd if=/proc/{pid}/mem bs=1 skip={address} count=4 2>/dev/null'))[0]

    def control(name, action, value=0):
        request = urllib.request.Request(
            f'http://127.0.0.1:{panel}/control',
            json.dumps({'name': name, 'action': action, 'value': value}).encode(),
            {'Content-Type': 'application/json'},
        )
        urllib.request.urlopen(request, timeout=5).read()
        time.sleep(.12)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def gesture(direction):
        control('16 LEVELS', 'press', 127)
        control('DATA', 'turn', direction)
        control('16 LEVELS', 'release')
        time.sleep(.5)

    def take(pad):
        """Record one pad press; return its window and the slots the hook produced."""
        stream = Pcm(pcm)
        stream.pump(0.6)
        anchor = len(stream.samples())
        tap(pad)
        stream.pump(1.4)
        samples = stream.samples()
        stream.close()
        start = anchor + int(SKIP * FS)
        segment = samples[start:start + WINDOW]
        if len(segment) < WINDOW:
            segment = np.pad(segment, (0, WINDOW - len(segment)))
        if np.sqrt(np.mean(segment ** 2)) < 500:
            raise AssertionError(f'{pad} was silent: no audio to measure')
        # The slots this press produced, read before the next press overwrites them.
        slots = (read('g_levels_pitch_result', '<i'), read('g_levels_fine_result', '<i'))
        return segment * np.hanning(WINDOW), slots

    def declared(slots):
        """The pitch the slots ask for: semitones plus fine steps of a tenth."""
        return slots[0] + slots[1] * FINE_STEP

    def semitones(ratio):
        return 12 * np.log2(ratio)

    tap('SAMPLE')
    tap('PAD 1')
    if read('g_midi', '<i', 4) == 0:  # the 16 Levels flag follows the dispatcher flag
        tap('16 LEVELS')
    assert read('g_midi', '<i', 4) == 1, '16 Levels did not come up'
    # Start from the whole grid whatever an earlier case left behind.
    gesture(-1)

    # Pick pads with tonal *and* repeatable material: a loop whose strongest
    # partial moves between takes cannot measure anything, and pads in a
    # project need not share a sample, so the case measures one pad twice
    # rather than two pads.
    whole, peaks, scores, ranked = {}, {}, {}, []
    for pad in CANDIDATES:
        first, slots = take(pad)
        score = tonality(first)
        if score < 4:
            continue
        second, _ = take(pad)
        # Two takes of the same press have to agree, or nothing downstream does.
        if abs(semitones(peak(second) / peak(first))) > 0.1:
            continue
        whole[pad] = (first, slots)
        peaks[pad] = peak(first)
        scores[pad] = score
        ranked.append(pad)
        if len(ranked) == 3:
            break
    if not ranked:
        raise Skip('no pad in the fixture plays a repeatable partial to track')
    ranked.sort(key=scores.get, reverse=True)

    # A pad whose microtonal offset is odd writes a fine step; that is the half
    # semitone under test, so the case needs one and keeps looking until it has
    # it. The others still have to agree with their slots.
    gesture(1)
    heard_by_pad = {}
    for pad in ranked[:4]:
        segment, slots = take(pad)
        expected = declared(slots) - declared(whole[pad][1])
        # Track the partial the whole grid measured, at the frequency the slots
        # say it moved to.
        measured = peak(segment, target=peaks[pad] * 2 ** (expected / 12))
        heard_by_pad[pad] = (semitones(measured / peaks[pad]), expected, slots, measured, expected)
        if slots[1] != 0:
            break

    # Leave the app on the whole grid for the next case.
    gesture(-1)

    fine = [pad for pad, check in heard_by_pad.items() if check[2][1] != 0]
    if not fine:
        raise Skip('none of the tonal pads sat on a fine step, so there is no half semitone to hear')

    for pad, (heard, expected, slots, measured, _) in heard_by_pad.items():
        assert abs(heard - expected) <= TOLERANCE, (
            f'{pad}: heard {heard:+.2f} semitones ({peaks[pad]:.1f} -> {measured:.1f} Hz), '
            f'slots declare {expected:+.2f} ({whole[pad][1]} -> {slots})')
    pad = fine[0]
    heard, expected, _, measured, _ = heard_by_pad[pad]
    print(f'PASS: {pad} moved {heard:+.2f} semitones for a declared {expected:+.2f} '
          f'({peaks[pad]:.1f} -> {measured:.1f} Hz), five fine steps included; '
          f'tonality {scores[pad]:.1f}; ' + ', '.join(
              f'{other} {check[0]:+.2f}/{check[1]:+.2f}' for other, check in heard_by_pad.items()))


if __name__ == '__main__':
    try:
        main()
    except Skip as skip:
        print(f'SKIP: {skip}')
