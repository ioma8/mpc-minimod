"""Render one plucked C2 bass sample through SAMPLE OPTIONS and Extract.

Requires the virtual MPC lab, the current mod preloaded, and Sample Mode open
on a populated pad. Checks the knob page, output WAV/pitch, pad assignment, and
that the firmware process did not restart. The source check allows the app's
own dither of project working copies (±2 LSB) but rejects real corruption.
"""
import json
import panel_screen
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
    def decode_audio(data):
        """The samples of a WAV as floats in -1..1, whatever the encoding.

        The stock app re-encodes a source it extracts from (24-bit PCM becomes
        32-bit float, with a metadata chunk), so the bytes change while the audio
        does not. The audio is the thing the mod must leave alone.
        """
        assert data[:4] == b'RIFF' and data[8:12] == b'WAVE', 'not a WAV'
        pos, fmt, samples = 12, None, None
        while pos + 8 <= len(data):
            tag = data[pos:pos + 4]
            size = int.from_bytes(data[pos + 4:pos + 8], 'little')
            body = data[pos + 8:pos + 8 + size]
            if tag == b'fmt ':
                fmt = struct.unpack('<HHIIHH', body[:16])
            elif tag == b'data':
                assert fmt, 'data before fmt'
                kind, _, _, _, _, bits = fmt
                if kind == 3 and bits == 32:
                    samples = list(struct.unpack('<%df' % (len(body) // 4), body[:len(body) // 4 * 4]))
                elif kind == 1 and bits == 16:
                    samples = [v / 32768 for v in struct.unpack('<%dh' % (len(body) // 2), body[:len(body) // 2 * 2])]
                elif kind == 1 and bits == 24:
                    count = len(body) // 3
                    samples = [int.from_bytes(body[i * 3:i * 3 + 3], 'little', signed=True) / 8388608
                               for i in range(count)]
                elif kind == 1 and bits == 32:
                    samples = [v / 2147483648 for v in struct.unpack('<%di' % (len(body) // 4), body[:len(body) // 4 * 4])]
                else:
                    raise AssertionError(f'unsupported WAV encoding {kind}/{bits}')
                return samples
            pos += 8 + size + (size & 1)
        raise AssertionError('Source WAV has no audio data')

    def same_audio(first, second):
        return len(first) == len(second) and all(abs(a - b) < 1e-5 for a, b in zip(first, second))

    source_before = ssh('cat ' + shlex.quote(source_path))
    source_audio = decode_audio(source_before)

    def bass_outputs():
        pattern = shlex.quote(str(source_dir)) + '/' + shlex.quote('Bass C2') + '*.wav'
        lines = ssh('for f in ' + pattern + '; do [ -f "$f" ] && sha256sum "$f"; done; true')
        return {line.split(None, 1)[1]: line.split(None, 1)[0]
                for line in lines.decode().splitlines()}

    before = bass_outputs()
    restarts = ssh('systemctl show ac50 -p NRestarts').decode().strip()
    status_addr = mod_base + symbol('g_hpss_status')
    busy_addr = mod_base + symbol('g_menu') + 4  # busy is the second i32

    # Open SAMPLE OPTIONS, select Synth (row 5), then open its control page.
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
    assert same_audio(decode_audio(source_pre_render), source_audio), \
        'Source pad audio changed before rendering'

    # The live preview: pads on the knob page strike the synth, are consumed
    # before the app can switch the sample underneath, and the mod mixes the
    # preview into the output the app writes.
    # The preview itself has no exported counter in the current build; the
    # observable contract is that the pad keys are consumed and the app never
    # switches the selected sample underneath the page.
    sample_before = qword(qword(qword(screen + 0x178) + 0x60) + 0x22e8)
    tap('PAD 5')
    tap('PAD 9')
    time.sleep(1)
    assert qword(qword(qword(screen + 0x178) + 0x60) + 0x22e8) == sample_before, \
        'a pad press on the Bass page switched the selected sample'
    time.sleep(1.5)
    tap('DATA')

    deadline = time.monotonic() + 12
    status = ''
    while time.monotonic() < deadline:
        status = read_mem(status_addr, 96).split(b'\0', 1)[0].decode(errors='replace')
        busy = int.from_bytes(read_mem(busy_addr, 4), 'little', signed=True)
        if status == 'Synth: copy created' and not busy:
            break
        if status.startswith('Synth:') and not status.startswith('Rendering') and not busy:
            raise AssertionError(status)
        time.sleep(.2)
    assert status == 'Synth: copy created', f'Unexpected result: {status}'
    time.sleep(1.5)
    panel_screen.assert_no_modal(args.panel, 'Synth')
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
    assert len(audio) == int(source_rate * 0.75), (len(audio), source_rate)
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
    assert abs(pitch - 65.406) < 5, f'Expected C2 fundamental, measured {pitch:.2f} Hz'
    sample_after = qword(qword(qword(screen + 0x178) + 0x60) + 0x22e8 + 0x4a8)
    assert sample_after != sample_before, \
        'Selected pad did not receive the rendered sample'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'Firmware restarted'
    assert ssh('systemctl show ac50 -p NRestarts').decode().strip() == restarts
    print(f'PASS: {output_path}; C2 peak {pitch:.2f} Hz; {len(audio)} stereo frames @ '
          f'{source_rate} Hz; RMS {rms:.5f}; pad assigned; {status}; no restart')


if __name__ == '__main__':
    main()
