"""Helpers for emulator scenarios run by emulator_suite.py."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time
import urllib.request


class Emulator:
    """The preloaded app and panel prepared for one isolated scenario."""

    def __init__(self):
        parser = argparse.ArgumentParser(description=sys.modules['__main__'].__doc__)
        parser.add_argument('--lab', type=Path, required=True)
        parser.add_argument('--panel', required=True)
        args = parser.parse_args()
        sys.path.insert(0, str(args.lab))
        import lab
        self.lab = lab
        self.panel = args.panel.rstrip('/')

    def ssh(self, command):
        return subprocess.check_output(
            ['ssh', *self.lab.ssh_base(), 'root@' + self.lab.HOST, command], timeout=20)

    def state(self):
        with urllib.request.urlopen(self.panel + '/state?since=0', timeout=5) as response:
            return json.load(response)

    def control(self, name, action, value=0):
        data = json.dumps({'name': name, 'action': action, 'value': value}).encode()
        request = urllib.request.Request(self.panel + '/control', data,
                                         {'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            if not json.load(response).get('ok'):
                raise RuntimeError(f'Panel rejected {name} {action}')
        time.sleep(.04)

    def tap(self, name):
        self.control(name, 'press', 100)
        self.control(name, 'release')

    def midi(self, hex_bytes):
        data = json.dumps({'hex': hex_bytes}).encode()
        request = urllib.request.Request(self.panel + '/midi', data,
                                         {'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            if not json.load(response).get('ok'):
                raise RuntimeError(f'Panel rejected MIDI {hex_bytes}')

    def screenshot(self):
        with urllib.request.urlopen(self.panel + '/screen.png', timeout=5) as response:
            return response.read()

    def wait_for(self, predicate, *, timeout=5, message='condition not reached'):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(.05)
        raise AssertionError(message)

    def process(self):
        return int(self.ssh('pidof "MPC Sample"').split()[0])

    def symbol_address(self, name):
        mapfile = Path(__file__).resolve().parents[2] / 'build/minimod.map'
        for line in mapfile.read_text().splitlines():
            fields = line.split()
            if fields and fields[-1] == name:
                offset = int(fields[0], 16)
                break
        else:
            raise ValueError(f'{name} missing from {mapfile}')
        pid = self.process()
        maps = self.ssh(f'cat /proc/{pid}/maps').decode().splitlines()
        mapping = next(line for line in maps if line.endswith('/minimod.so'))
        return int(mapping.split('-')[0], 16) + offset

    def read(self, address, size):
        pid = self.process()
        data = self.ssh(f'dd if=/proc/{pid}/mem bs=1 skip={address} count={size} 2>/dev/null')
        if len(data) != size:
            raise RuntimeError(f'Could not read {size} bytes at {address:#x}')
        return data

    def u64(self, address):
        return int.from_bytes(self.read(address, 8), 'little')

    def u32(self, address):
        return int.from_bytes(self.read(address, 4), 'little')
