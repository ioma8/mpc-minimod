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
    parser.add_argument('--lab', default='/Users/jakubkolcar/projects/customs/virtual-mpc-sample')
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    args = parser.parse_args()
    del args.lab

    def ssh(command):
        return subprocess.check_output(
            ['ssh', '-o', 'BatchMode=yes', '-p', '2022', 'root@127.0.0.1', command],
            timeout=15)

    pid = int(ssh('pidof "MPC Sample"').split()[0])
    maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
    base = int(next(line for line in maps if line.endswith('/minimod.so')).split('-')[0], 16)
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
        time.sleep(.12)

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
