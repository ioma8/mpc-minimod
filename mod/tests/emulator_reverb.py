"""Verify native sample effects through SAMPLE OPTIONS and native Extract.

Requires a running virtual-mpc-sample lab with the Sample page visible and the
mod preloaded. By default this verifies Reverb. Use --effect vinyl or --effect
lofi to verify the two new native effects.
"""
import argparse
import json
from pathlib import Path
import shlex
import struct
import subprocess
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    parser.add_argument('--effect', choices=('reverb', 'vinyl', 'lofi'), default='reverb')
    args = parser.parse_args()
    action = {
        'reverb': ('Reverb', 2, True, 'Reverb'),
        'vinyl': ('Vinyl', 3, False, 'Vinyl'),
        'lofi': ('Lo-Fi', 4, False, 'Lo-Fi'),
    }[args.effect]
    title, menu_index, expect_tail, filename_prefix = action
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
        # Give the firmware event loop time to consume each MIDI event. The
        # control endpoint acknowledges transport delivery, not UI handling.
        time.sleep(.2)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def symbol(name):
        for line in subprocess.check_output(['nm', '-a', str(ROOT / 'build/minimod.elf')],
                                            text=True).splitlines():
            fields = line.split()
            if fields and fields[-1] == name:
                return int(fields[0], 16)
        raise AssertionError(f'Missing {name} in build/minimod.elf')

    def wav(data):
        pos = 12
        fmt = None
        while pos + 8 <= len(data):
            tag = data[pos:pos + 4]
            size = int.from_bytes(data[pos + 4:pos + 8], 'little')
            end = pos + 8 + size
            assert end <= len(data), 'Truncated RIFF chunk'
            if tag == b'fmt ':
                chunk = data[pos + 8:end]
                fmt = (int.from_bytes(chunk[:2], 'little'),
                       int.from_bytes(chunk[2:4], 'little'),
                       int.from_bytes(chunk[4:8], 'little'),
                       int.from_bytes(chunk[14:16], 'little'))
            elif tag == b'data':
                assert fmt
                frame_bytes = fmt[1] * (fmt[3] // 8)
                return fmt, size // frame_bytes, data[pos + 8:end]
            pos = end + (size & 1)
        raise AssertionError('WAV has no data chunk')

    pid = int(ssh('pidof "MPC Sample"'))
    maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
    app_base = int(next(x for x in maps if x.endswith('/MPC Sample')).split('-')[0], 16)
    mod_base = int(next(x for x in maps if x.endswith('/minimod.so')).split('-')[0], 16)

    def memory(address, size):
        data = ssh(f'dd if=/proc/{pid}/mem bs=1 skip={address} count={size} 2>/dev/null')
        assert len(data) == size, 'App died or memory address became invalid'
        return data

    def qword(address):
        return int.from_bytes(memory(address, 8), 'little')

    screen = qword(mod_base + symbol('g_screen'))
    assert qword(screen) == app_base + 0x6e212a0, 'Sample page must be visible'
    binding = qword(screen + 0x178)
    state = qword(binding + 0x60)
    sample = qword(qword(state + 0x22e8) + 0x4a8)
    meta = qword(sample + 0x130)
    source_frames = int.from_bytes(memory(meta + 4, 4), 'little')
    source_channels = int.from_bytes(memory(meta, 2), 'little')
    source_rate = int.from_bytes(memory(meta + 8, 4), 'little')
    holder = qword(sample + 0x30)
    source_path_ptr = qword(holder + 0x120)
    source_path = memory(source_path_ptr, 256).split(b'\0', 1)[0].decode()
    source_path = Path(source_path)
    source_wav = wav(ssh('cat ' + __import__('shlex').quote(str(source_path))))
    assert source_wav[1] == source_frames
    assert source_wav[0][1:] == (source_channels, source_rate, 24) or \
           source_wav[0][1:] == (source_channels, source_rate, 32)

    # Quote the directory separately: sample project names can contain `[]`,
    # which are glob syntax if they are included in the unquoted pattern.
    output_glob = shlex.quote(str(source_path.parent)) + '/' + shlex.quote(filename_prefix) + '*.wav'
    list_outputs = 'for f in ' + output_glob + '; do [ -f "$f" ] && sha256sum "$f"; done; true'
    def output_hashes():
        lines = ssh(list_outputs).decode().splitlines()
        return {line.split(None, 1)[1]: line.split(None, 1)[0] for line in lines}
    before = output_hashes()
    baseline_restarts = ssh('systemctl show ac50 -p NRestarts').decode().strip()

    tap('PAD 1')
    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    for _ in range(menu_index):
        control('DATA', 'turn', 1)
    tap('DATA')

    status_addr = mod_base + symbol('g_hpss_status')
    busy_addr = mod_base + symbol('g_hpss_busy')
    status = ''
    for _ in range(20):
        time.sleep(.25)
        status = memory(status_addr, 96).split(b'\0', 1)[0].decode(errors='replace')
        busy = int.from_bytes(memory(busy_addr, 4), 'little', signed=True)
        if status == f'{title}: copy created' and not busy:
            break
        if status.startswith(title + ':') and not status.startswith('Processing ') and not busy:
            raise AssertionError(status)
    assert status == f'{title}: copy created', f'Unexpected result: {status}'

    current = output_hashes()
    changed = {path for path, digest in current.items() if before.get(path) != digest}
    assert len(changed) == 1, f'Expected one new or replaced Reverb WAV, found {changed}'
    output_path = next(iter(changed))
    output_fmt, output_frames, output_data = wav(ssh('cat ' + shlex.quote(output_path)))
    if expect_tail:
        assert output_frames > source_frames, (source_frames, output_frames)
    else:
        assert output_frames == source_frames, (source_frames, output_frames)
    assert output_fmt[1] == source_channels and output_fmt[2] == source_rate
    assert output_data != source_wav[2], 'Output is identical to the source'
    assert output_fmt[0] == 3 and output_fmt[3] == 32, output_fmt
    samples = struct.unpack('<' + 'f' * (len(output_data) // 4), output_data)
    output_rms = (sum(x * x for x in samples) / max(1, len(samples))) ** .5
    assert output_rms > 1e-5, f'Output is silent: RMS={output_rms}'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'Firmware restarted during Reverb'
    assert ssh('systemctl show ac50 -p NRestarts').decode().strip() == baseline_restarts
    print(f'PASS: {output_path}; {output_frames} frames (+{output_frames - source_frames} tail), '
          f'{output_fmt[1]} ch @ '
          f'{output_fmt[2]} Hz; native Extract assigned the pad; {status}; pid={pid}, '
          f'{baseline_restarts}; output RMS={output_rms:.6f}')


if __name__ == '__main__':
    main()
