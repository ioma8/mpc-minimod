# /// script
# requires-python = ">=3.10"
# ///
"""Check the synth page's PLUS/MINUS eight-semitone shift.

The page starts at 0, PLUS adds 8 semitones, MINUS subtracts 8, the shift
clamps at four steps and is reset when the page is opened again.
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
    parser.add_argument('--lab', required=True)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    args = parser.parse_args()
    import sys
    sys.path.insert(0, args.lab)
    import lab

    def ssh(command):
        return subprocess.check_output(
            ['ssh', *lab.ssh_base(), 'root@' + lab.HOST, command],
            timeout=15)

    pid = int(ssh('pidof "MPC Sample"').split()[0])
    maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
    # The first VMA is not necessarily the ELF load bias (the linker can leave
    # a gap before the first PT_LOAD). Match a mapped file offset to its segment.
    elf = (ROOT / 'build' / 'minimod.so').read_bytes()
    phoff = struct.unpack_from('<Q', elf, 32)[0]
    phentsize, phnum = struct.unpack_from('<HH', elf, 54)
    load_segments = []
    for index in range(phnum):
        offset = phoff + index * phentsize
        kind, file_offset, virtual_address = struct.unpack_from('<I4xQQ', elf, offset)
        if kind == 1:  # PT_LOAD
            load_segments.append((file_offset & ~0xfff, virtual_address & ~0xfff))
    base = None
    for line in maps:
        if not line.endswith('/minimod.so'):
            continue
        address, _, offset, *_ = line.split(maxsplit=5)
        start = int(address.split('-')[0], 16)
        file_offset = int(offset, 16)
        for segment_offset, segment_address in load_segments:
            if file_offset == segment_offset:
                base = start - segment_address
                break
        if base is not None:
            break
    assert base is not None, 'could not find minimod ELF load bias'
    symbols = {}
    for line in subprocess.check_output(
            ['nm', '-a', str(ROOT / 'build' / 'minimod.elf')], text=True).splitlines():
        fields = line.split()
        if len(fields) >= 3:
            symbols[fields[-1]] = int(fields[0], 16)

    def read_i32(name):
        address = base + symbols[name]
        return struct.unpack(
            '<i', ssh(f'dd if=/proc/{pid}/mem bs=1 skip={address} count=4 2>/dev/null'))[0]

    def control(name, action, value=0):
        body = json.dumps({'name': name, 'action': action, 'value': value}).encode()
        request = urllib.request.Request(args.panel + '/control', data=body,
                                         headers={'Content-Type': 'application/json'})
        urllib.request.urlopen(request, timeout=5).read()
        time.sleep(.04 if action in ('press', 'release') else .02)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def open_synth():
        tap('SAMPLE')
        time.sleep(.5)
        control('SHIFT', 'press', 127)
        tap('DATA')
        control('SHIFT', 'release')
        time.sleep(.5)
        for _ in range(5):
            control('DATA', 'turn', 1)
        tap('DATA')
        time.sleep(.8)

    def close_synth():
        control('SHIFT', 'press', 127)
        tap('DATA')
        control('SHIFT', 'release')
        time.sleep(.7)

    open_synth()
    assert read_i32('g_bass_transpose') == 0, 'synth page did not start at 0'
    tap('PLUS')
    assert read_i32('g_bass_transpose') == 8, 'PLUS did not add 8 semitones'
    tap('PLUS')
    assert read_i32('g_bass_transpose') == 16, 'second PLUS did not add 8'
    tap('MINUS')
    assert read_i32('g_bass_transpose') == 8, 'MINUS did not subtract 8'
    tap('MINUS')
    assert read_i32('g_bass_transpose') == 0, 'MINUS did not return to 0'
    tap('MINUS')
    assert read_i32('g_bass_transpose') == -8, 'MINUS did not go below 0'

    close_synth()
    open_synth()
    assert read_i32('g_bass_transpose') == 0, 'reopening the page did not reset the shift'
    close_synth()

    print('PASS: synth PLUS/MINUS shifts by 8 semitones and resets on open')


if __name__ == '__main__':
    main()
