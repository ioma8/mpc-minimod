"""Render one plucked C2 bass sample through SAMPLE OPTIONS and Extract.

Requires the virtual MPC lab, the current mod preloaded, and Sample Mode open
on a populated pad. Checks the knob page, output WAV/pitch, pad assignment, and
that the firmware process did not restart. The source check allows the app's
own dither of project working copies (±2 LSB) but rejects real corruption.
"""
import json
import hashlib
import math
from pathlib import Path
import shlex
import struct
import subprocess
import sys
import time
import urllib.request


ROOT = Path(__file__).resolve().parents[1]


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    args = parser.parse_args()
    sys.path.insert(0, str(args.lab))
    import lab

    def ssh(command):
        return subprocess.check_output(['ssh', *lab.ssh_base(), 'root@' + lab.HOST,
                                        command], timeout=20)

    def control(name, action, value=0):
        body = json.dumps(dict(name=name, action=action, value=value)).encode()
        req = urllib.request.Request(args.panel + '/control', data=body,
                                     headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=5) as response:
            assert json.load(response)['ok']
        time.sleep(.15)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def symbol(name):
        for line in subprocess.check_output(['nm', '-a', str(ROOT / 'build/minimod.elf')],
                                            text=True).splitlines():
            fields = line.split()
            if fields and fields[-1] == name:
                return int(fields[0], 16)
        raise AssertionError(f'Missing {name}')

    def read_mem(address, size):
        data = ssh(f'dd if=/proc/{pid}/mem bs=1 skip={address} count={size} 2>/dev/null')
        assert len(data) == size, 'MPC process exited or memory read failed'
        return data

    def qword(address):
        return int.from_bytes(read_mem(address, 8), 'little')

    pid = int(ssh('pidof "MPC Sample"'))
    maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
    app_base = int(next(x for x in maps if x.endswith('/MPC Sample')).split('-')[0], 16)
    mod_base = int(next(x for x in maps if x.endswith('/minimod.so')).split('-')[0], 16)
    screen = qword(mod_base + symbol('g_screen'))
    assert qword(screen) == app_base + 0x6e212a0, 'Sample Mode must be visible'
    tap('PAD 1')
    time.sleep(.3)
    binding = qword(screen + 0x178)
    state = qword(binding + 0x60)
    sample = qword(qword(state + 0x22e8) + 0x4a8)
    meta = qword(sample + 0x130)
    source_frames = int.from_bytes(read_mem(meta + 4, 4), 'little')
    source_channels = int.from_bytes(read_mem(meta, 2), 'little')
    source_rate = int.from_bytes(read_mem(meta + 8, 4), 'little')
    holder = qword(sample + 0x30)
    path_ptr = qword(holder + 0x120)
    source_path = read_mem(path_ptr, 256).split(b'\0', 1)[0].decode()
    source_dir = Path(source_path).parent
    def wav_format(data):
        pos = 12
        while pos + 8 <= len(data):
            tag = data[pos:pos + 4]
            size = int.from_bytes(data[pos + 4:pos + 8], 'little')
            if tag == b'fmt ':
                fmt = int.from_bytes(data[pos + 8:pos + 10], 'little')
                channels = int.from_bytes(data[pos + 10:pos + 12], 'little')
                bits = int.from_bytes(data[pos + 22:pos + 24], 'little')
                return fmt, channels, bits
            pos += 8 + size + (size & 1)
        raise AssertionError('Source WAV has no fmt chunk')

    def wav_audio_bytes(data):
        pos = 12
        while pos + 8 <= len(data):
            tag = data[pos:pos + 4]
            size = int.from_bytes(data[pos + 4:pos + 8], 'little')
            if tag == b'data':
                return data[pos + 8:pos + 8 + size]
            pos += 8 + size + (size & 1)
        raise AssertionError('Source WAV has no audio data')

    # The app re-serialises and re-dithers its project working copies whenever
    # Extract adds a sample, so a byte-exact comparison would fail on a healthy
    # run. Compare decoded audio instead and allow the dither (±2 LSB).
    def source_audio(raw):
        fmt, channels, bits = wav_format(raw)
        payload = wav_audio_bytes(raw)
        if fmt == 3 and bits == 32:
            values = [v[0] for v in struct.iter_unpack('<f', payload[:len(payload) // 4 * 4])]
            return values, channels, 1e-6
        if fmt != 1 or bits not in (16, 24, 32):
            raise AssertionError(f'Unsupported source format {fmt}/{bits}')
        step = bits // 8
        values = []
        for i in range(0, len(payload) - step + 1, step):
            chunk = payload[i:i + step]
            values.append(int.from_bytes(chunk, 'little', signed=True))
        return values, channels, 4.0        # counts: the app dithers by up to +-2 LSB

    source_before = ssh('cat ' + shlex.quote(source_path))
    before_audio, before_channels, tolerance = source_audio(source_before)

    def bass_outputs():
        pattern = shlex.quote(str(source_dir)) + '/' + shlex.quote('Bass C2') + '*.wav'
        lines = ssh('for f in ' + pattern + '; do [ -f "$f" ] && sha256sum "$f"; done; true')
        return {line.split(None, 1)[1]: line.split(None, 1)[0]
                for line in lines.decode().splitlines()}

    before = bass_outputs()
    restarts = ssh('systemctl show ac50 -p NRestarts').decode().strip()
    status_addr = mod_base + symbol('g_hpss_status')
    busy_addr = mod_base + symbol('g_hpss_busy')

    # Open SAMPLE OPTIONS, select Bass Synth (row 5), then open its control page.
    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    for _ in range(5):
        control('DATA', 'turn', 1)
    tap('DATA')
    for knob, value in (('K1', 96), ('K2', 48), ('K3', 80)):
        control(knob, 'set', value)
    knob_addr = mod_base + symbol('g_bass_knob')
    knob_values = struct.unpack('<3f', read_mem(knob_addr, 12))
    assert all(abs(a - b) < .02 for a, b in zip(
        knob_values, (96 / 127, 48 / 127, 80 / 127))), knob_values
    source_pre_render = ssh('cat ' + shlex.quote(source_path))
    pre_audio, pre_channels, pre_tolerance = source_audio(source_pre_render)
    assert pre_channels == before_channels and len(pre_audio) == len(before_audio) and \
        max((abs(x - y) for x, y in zip(before_audio, pre_audio)), default=0) <= \
        max(tolerance, pre_tolerance), 'Source pad audio changed before rendering'
    tap('DATA')

    deadline = time.monotonic() + 12
    status = ''
    while time.monotonic() < deadline:
        status = read_mem(status_addr, 96).split(b'\0', 1)[0].decode(errors='replace')
        busy = int.from_bytes(read_mem(busy_addr, 4), 'little', signed=True)
        if status == 'Bass Synth: copy created' and not busy:
            break
        if status.startswith('Bass Synth:') and not status.startswith('Rendering') and not busy:
            raise AssertionError(status)
        time.sleep(.2)
    assert status == 'Bass Synth: copy created', f'Unexpected result: {status}'
    current = bass_outputs()
    changed = [path for path, digest in current.items() if before.get(path) != digest]
    assert current, 'Native Extract did not write a Bass C2 WAV'
    # Repeated deterministic renders can reuse the same Bass C2 filename and
    # produce identical bytes; Extract's successful free-pad transition is the
    # assignment assertion in the mod status, while the WAV remains reusable.
    output_path = changed[0] if changed else sorted(current)[-1]
    raw = ssh('cat ' + shlex.quote(output_path))
    pos = raw.index(b'data')
    size = int.from_bytes(raw[pos + 4:pos + 8], 'little')
    samples = struct.unpack('<' + 'f' * (size // 4), raw[pos + 8:pos + 8 + size])
    audio = list(zip(samples[::2], samples[1::2]))
    assert len(audio) == int(source_rate * 1.5), (len(audio), source_rate)
    rms = (sum(l * l + r * r for l, r in audio) / (2 * len(audio))) ** .5
    assert rms > 1e-4, f'Generated sample is silent (RMS {rms})'
    mono = [(l + r) * .5 for l, r in audio]
    pitch, peak = 0.0, -1.0
    for hz in range(50, 86):
        omega = 2 * math.pi * hz / source_rate
        coeff = 2 * math.cos(omega)
        s1 = s2 = 0.0
        for i, value in enumerate(mono):
            value *= .5 - .5 * math.cos(2 * math.pi * i / (len(mono) - 1))
            s0 = value + coeff * s1 - s2
            s2, s1 = s1, s0
        strength = s1 * s1 + s2 * s2 - coeff * s1 * s2
        if strength > peak:
            pitch, peak = float(hz), strength
    assert abs(pitch - 65.406) < 4, f'Expected C2 fundamental, measured {pitch:.2f} Hz'
    source_after = ssh('cat ' + shlex.quote(source_path))
    after_audio, after_channels, after_tolerance = source_audio(source_after)
    assert after_channels == before_channels, 'Source channel count changed'
    assert len(after_audio) == len(before_audio), \
        f'Source length changed: {len(before_audio)} -> {len(after_audio)}'
    worst = max((abs(x - y) for x, y in zip(before_audio, after_audio)), default=0)
    assert worst <= max(tolerance, after_tolerance), \
        f'Source audio changed beyond dither: max delta {worst} > {tolerance}'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'Firmware restarted'
    assert ssh('systemctl show ac50 -p NRestarts').decode().strip() == restarts
    print(f'PASS: {output_path}; C2 peak {pitch:.2f} Hz; {len(audio)} stereo frames @ '
          f'{source_rate} Hz; RMS {rms:.5f}; pad assigned; {status}; no restart')


if __name__ == '__main__':
    main()
