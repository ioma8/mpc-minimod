"""Run the HPSS split from SAMPLE OPTIONS and check both pads were created.

Requires the virtual MPC lab, the mod preloaded, and a sample loaded on the
selected pad.  This is the only acceptance test for the split; it exercises the
action end to end (worker, writer, extract) and the status the menu shows.
"""
import argparse
import json
import struct
import subprocess
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    parser.add_argument('--lab', default=str(ROOT.parent / 'virtual-mpc-sample'),
                        help='the virtual lab checkout (for the mod ELF symbols)')
    args = parser.parse_args()

    def ssh(command):
        return subprocess.check_output(
            ['ssh', '-p', '2022', '-o', 'StrictHostKeyChecking=no',
             '-o', 'UserKnownHostsFile=/dev/null', '-o', 'LogLevel=ERROR',
             'root@127.0.0.1', command], timeout=20)

    def control(name, action, value=0):
        body = json.dumps(dict(name=name, action=action, value=value)).encode()
        request = urllib.request.Request(args.panel + '/control', data=body,
                                         headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            assert json.load(response)['ok']
        time.sleep(.15)

    def tap(name):
        control(name, 'press', 127)
        control(name, 'release')

    def symbol(name):
        for line in subprocess.check_output(
                ['nm', '-a', str(ROOT / 'build/minimod.elf')], text=True).splitlines():
            fields = line.split()
            if fields and fields[-1] == name:
                return int(fields[0], 16)
        raise AssertionError(f'missing symbol {name}')

    time.sleep(2.0)
    pid = int(ssh('pidof "MPC Sample"'))
    maps = ssh(f'cat /proc/{pid}/maps').decode()
    base = int(next(line for line in maps.splitlines()
                    if line.endswith('/minimod.so')).split('-')[0], 16)

    def read(offset, size):
        data = ssh(f'dd if=/proc/{pid}/mem bs=1 skip={base + offset} count={size} '
                   f'2>/dev/null')
        assert len(data) == size, 'app died or the address became invalid'
        return data

    def status():
        return read(symbol('g_hpss_status'), 96).split(b'\0', 1)[0].decode(errors='replace')

    def busy():
        return struct.unpack('<i', read(symbol('g_hpss_busy'), 4))[0]

    restarts = ssh('systemctl show ac50 -p NRestarts').decode().strip()

    # SAMPLE OPTIONS opens on HPSS (row 0): DATA opens it, DATA runs it.
    control('SHIFT', 'press', 127)
    tap('DATA')
    control('SHIFT', 'release')
    time.sleep(.6)
    tap('DATA')
    time.sleep(.4)
    tap('DATA')

    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        if status() and not busy():
            break
        time.sleep(.5)
    result = status()
    # The mod sets this string only after *both* extracts reported an assigned
    # pad, so it is the action's own end-to-end assertion.
    assert result == 'HPSS: harmonic + percussive created', f'unexpected result: {result!r}'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'firmware restarted'
    assert ssh('systemctl show ac50 -p NRestarts').decode().strip() == restarts
    print(f'PASS: {result!r}; no restart; pid={pid}')


if __name__ == '__main__':
    main()
