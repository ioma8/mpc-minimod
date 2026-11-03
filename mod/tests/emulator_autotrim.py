"""Run on the factory demo project after loading the release mod (port offset 100)."""
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.request

LAB = Path(os.environ.get('MPCLAB_DIR', '../virtual-mpc-sample')).resolve()
ENV = dict(os.environ, MPCLAB_PORT_OFFSET='100')

def ssh(command):
    return subprocess.check_output(['python3', str(LAB / 'lab.py'), 'ssh', command], env=ENV, text=True)

def control(name, action, **values):
    body = json.dumps(dict(name=name, action=action, **values)).encode()
    with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8865/control', data=body, headers={'Content-Type': 'application/json'}), timeout=10) as response:
        assert json.load(response)['ok']

def press(name):
    control(name, 'press', value=100)
    control(name, 'release')

def screenshot(label):
    path = Path('/tmp/autotrim-' + label + '.ppm')
    with socket.create_connection(('127.0.0.1', 5654), timeout=10) as sock:
        stream = sock.makefile('rwb', buffering=0)
        stream.readline()
        for command in [{'execute': 'qmp_capabilities'}, {'execute': 'human-monitor-command', 'arguments': {'command-line': 'screendump ' + str(path)}}]:
            stream.write((json.dumps(command) + '\n').encode())
            while True:
                result = json.loads(stream.readline())
                if 'return' in result:
                    break
                assert 'error' not in result, result
    return path.read_bytes()

pid = ssh('pidof "MPC Sample"').strip()
press('SAMPLE')
time.sleep(2)
press('SCREEN 1')
for pad, sample in [(1, 'BoomBap-Kik-BTB MD.wav'), (3, 'HipHop-Clp-Hrt OS.wav')]:
    press('PAD ' + str(pad))
    time.sleep(1)
    control('K1', 'set', value=10)
    control('K1', 'set', value=0)
    control('K2', 'set', value=80)
    control('K2', 'set', value=127)
    time.sleep(1)
    before = screenshot(str(pad) + '-before')
    control('SHIFT', 'press')
    press('SCREEN 3')
    control('SHIFT', 'release')
    time.sleep(2)
    after = screenshot(str(pad) + '-after')
    assert before != after, 'Native trim did not redraw'
    log = ssh('tail -20 /run/mpcmini.log')
    assert 'native trim pad=' + str(pad - 1) in log and sample in log, log
    assert ssh('pidof "MPC Sample"').strip() == pid, 'App restarted'
    press('SCREEN 2')
    time.sleep(1)
    assert screenshot(str(pad) + '-tune') != after, 'UI unresponsive'
    press('SCREEN 1')
    time.sleep(1)
    print('PASS pad', pad, sample, 'native redraw and live UI; pid', pid)
