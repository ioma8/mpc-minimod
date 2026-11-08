"""Live looper acceptance: record from the selected input, play, overdub, sync.

Requires the virtual MPC lab with a build of this mod preloaded, on the Sample
page. It clears any loops left from an earlier run.

What it proves, in order:
  * SAMPLE OPTIONS has a Live Looper entry and it opens a fullscreen view
  * the record latency is the firmware's own ALSA ring geometry (one buffer
    per direction), read from the device without any probe
  * the metronome click is off by default
  * a pad records from the selected input source and closes into playback,
    with a measured latency compensation on the take
  * the recorded loop really is on the device output (440 Hz from the tone
    source, after the input is unplugged)
  * pressing the pad again stops it (output goes silent)
  * SHIFT + pad overdubs and a second press returns to playback
  * LOOP starts the sync pulse and a new loop is quantized to a whole pulse
  * STOP stops every loop, ERASE + pad clears one, SHIFT + LOOP toggles the click
  * SHIFT + DATA closes the view, keeps the loops, and the app does not restart
"""
import argparse
import json
import math
import struct
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RING_FRAMES = 44100 * 2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--panel', default='http://127.0.0.1:8765')
    args = parser.parse_args()
    sys.path.insert(0, str(args.lab))
    import lab

    def ssh(command, binary=False):
        out = subprocess.check_output(['ssh', *lab.ssh_base(), 'root@' + lab.HOST,
                                       command], timeout=40)
        return out if binary else out.decode()

    def control(name, action, value=0):
        body = json.dumps(dict(name=name, action=action, value=value)).encode()
        request = urllib.request.Request(args.panel + '/control', data=body,
                                         headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            assert json.load(response)['ok']
        time.sleep(0.08)

    def tap(name):
        control(name, 'press', 100)
        control(name, 'release')

    def route(source):
        body = json.dumps(dict(jack='AUDIO IN', target=source)).encode()
        request = urllib.request.Request(args.panel + '/route', data=body,
                                         headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            response.read()
        time.sleep(0.15)

    def symbol(name):
        for line in subprocess.check_output(['nm', '-a', str(ROOT / 'build/minimod.elf')],
                                            text=True).splitlines():
            fields = line.split()
            if fields and fields[-1] == name:
                return int(fields[0], 16)
        raise AssertionError(f'missing symbol {name}')

    pid = int(ssh('pidof "MPC Sample"'))
    maps = ssh(f'cat /proc/{pid}/maps').splitlines()
    mod_base = int(next(x for x in maps if x.endswith('/minimod.so')).split('-')[0], 16)

    def read(offset, size):
        data = ssh(f'dd if=/proc/{pid}/mem bs=1 skip={mod_base + offset} count={size} '
                   f'2>/dev/null', binary=True)
        assert len(data) == size, f'short read at {offset:#x}'
        return data

    def u32(offset):
        return struct.unpack('<I', read(offset, 4))[0]

    stride = u32(symbol('g_pad_stride'))

    def snapshot():
        raw = read(symbol('g_pads'), 16 * stride)
        pads = []
        for i in range(16):
            state, length, rec, lat = struct.unpack(
                '<IIII', raw[i * stride:i * stride + 16])
            pads.append((state, length, rec, lat))
        return pads

    def output_tail(frames=4000):
        w = u32(symbol('g_outtap_w'))
        ring = struct.unpack(f'<{RING_FRAMES}f', read(symbol('g_outtap'), RING_FRAMES * 4))
        return [ring[(w - frames + k) % RING_FRAMES] for k in range(frames)]

    def rms(values):
        return math.sqrt(sum(v * v for v in values) / len(values))

    def zero_cross_hz(values, rate=44100):
        crossings = sum(1 for a, b in zip(values, values[1:]) if (a < 0) != (b < 0))
        return crossings / 2 / (len(values) / rate)

    def wait_state(pad, want, timeout=1.5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if snapshot()[pad][0] == want:
                return
            time.sleep(0.03)
        raise AssertionError(f'pad {pad + 1} never reached state {want}: {snapshot()[pad]}')

    def open_looper():
        control('SHIFT', 'press', 127)
        control('DATA', 'press', 127)
        control('DATA', 'release')
        control('SHIFT', 'release')
        time.sleep(0.4)
        for _ in range(6):
            control('DATA', 'turn', 1)
        control('DATA', 'press', 127)
        control('DATA', 'release')
        time.sleep(0.6)

    def close_looper():
        control('SHIFT', 'press', 127)
        control('DATA', 'press', 127)
        control('DATA', 'release')
        control('SHIFT', 'release')
        time.sleep(0.6)

    def clear_all():
        control('ERASE', 'press', 127)
        tap('LOOP')
        control('ERASE', 'release')

    def hold(name, other):
        control(name, 'press', 127)
        tap(other)
        control(name, 'release')

    # --- the record latency is the firmware's own ring geometry, read from the
    #     device: one full ALSA buffer per direction, no probe involved
    route('tone')
    if u32(symbol('g_open')):
        close_looper()          # a previous run may have left the view open
    open_looper()
    assert u32(symbol('g_open')) == 1, 'the looper view did not open'
    time.sleep(0.5)
    buf_out = u32(symbol('g_buf_out'))
    buf_in = u32(symbol('g_buf_in'))
    assert buf_out > 0 and buf_in > 0, (buf_out, buf_in)
    cal_frames = buf_out + buf_in
    route('tone')
    clear_all()
    time.sleep(0.3)
    pads = snapshot()
    assert len(pads) == 16, pads
    assert all(p[0] == 0 and p[1] == 0 for p in pads), pads
    assert u32(symbol('g_click')) == 0, 'the metronome must start off'

    # --- record one second from the tone input, then close into playback
    tap('PAD 1')
    time.sleep(1.0)
    tap('PAD 1')
    wait_state(0, 2)                       # PLAY
    state, length, rec, lat = snapshot()[0]
    assert length > 40000, (state, length, rec)
    assert abs(length - rec) < 4000, (length, rec)
    assert lat == cal_frames + u32(symbol('g_lat_extra')), \
        f'take latency {lat} != ring geometry {cal_frames} + trim'

    # --- MAIN VOLUME scales the loops like the rest of the app
    control('MAIN VOLUME', 'set', 127)
    time.sleep(0.3)
    vol_full = rms(output_tail())
    control('MAIN VOLUME', 'set', 64)
    # The knob travels through the emulator panel asynchronously, so wait for
    # the change to become audible instead of assuming a fixed delay.
    deadline = time.monotonic() + 2.0
    vol_half = rms(output_tail())
    while vol_half > 0.7 * vol_full and time.monotonic() < deadline:
        time.sleep(0.05)
        vol_half = rms(output_tail())
    control('MAIN VOLUME', 'set', 127)
    time.sleep(0.2)
    assert 0.35 < vol_half / vol_full < 0.65, (vol_half, vol_full)

    # --- unplug the input: what remains can only be the loop we recorded
    route('none')
    time.sleep(0.5)
    tail = output_tail()
    loop_rms = rms(tail)
    loop_hz = zero_cross_hz(tail)
    assert 0.05 < loop_rms < 0.5, loop_rms
    assert 400 < loop_hz < 480, loop_hz
    loop_length = length

    # --- pressing the playing pad stops it
    tap('PAD 1')
    wait_state(0, 4)                       # STOP
    time.sleep(0.3)
    assert rms(output_tail()) < 0.01, 'output should be silent after stop'

    # --- SHIFT + pad overdubs, a second press returns to playback. The input is
    #     silent here so the check is about the state machine and the untouched
    #     loop, not about what a second tone would add.
    route('none')
    time.sleep(0.2)
    hold('SHIFT', 'PAD 1')
    wait_state(0, 3)                       # DUB
    time.sleep(0.5)
    tap('PAD 1')
    wait_state(0, 2)                       # PLAY
    assert snapshot()[0][1] == loop_length, snapshot()[0]
    route('none')
    time.sleep(0.4)
    assert rms(output_tail()) > 0.02, 'overdub lost the loop'

    # --- LOOP starts the sync pulse from the last recorded length
    tap('LOOP')
    time.sleep(0.3)
    assert u32(symbol('g_sync')) == 1, 'sync pulse did not start'
    pulse = u32(symbol('g_sync_frames'))
    assert abs(pulse - loop_length) < 2000, (pulse, loop_length)

    # --- a short recording is quantized to one whole pulse
    route('tone')
    time.sleep(0.2)
    tap('PAD 2')
    time.sleep(0.45)
    tap('PAD 2')
    wait_state(1, 2)                       # PLAY
    state, length, rec, lat = snapshot()[1]
    assert length == pulse, (length, pulse, rec)
    assert rec < length, (rec, length)     # padded with silence, not stretched
    quant_len, quant_rec, take_lat = length, rec, lat

    # --- STOP stops everything that is playing
    tap('STOP')
    time.sleep(0.3)
    assert all(p[0] != 2 and p[0] != 3 for p in snapshot()), snapshot()
    assert rms(output_tail()) < 0.01, 'STOP should silence every loop'

    # --- ERASE + pad clears exactly that pad
    control('ERASE', 'press', 127)
    tap('PAD 2')
    control('ERASE', 'release')
    time.sleep(0.2)
    state, length, rec, lat = snapshot()[1]
    assert state == 0 and length == 0, (state, length, rec)

    # --- SHIFT + LOOP toggles the metronome click (off by default)
    hold('SHIFT', 'LOOP')
    time.sleep(0.2)
    assert u32(symbol('g_click')) == 1, 'SHIFT + LOOP did not enable the click'
    hold('SHIFT', 'LOOP')
    time.sleep(0.2)
    assert u32(symbol('g_click')) == 0, 'SHIFT + LOOP did not disable the click'

    # --- K1 trims the record latency, 1 ms per step.  The trim is persisted, so
    #     the check is a delta, and K1 always goes back to zero even when an
    #     assertion fails: a half-turned knob would skew every later run.
    control('K1', 'set', 0)
    time.sleep(0.2)
    trim0 = u32(symbol('g_lat_extra'))
    try:
        control('K1', 'set', 10)
        time.sleep(0.2)
        assert u32(symbol('g_lat_extra')) == trim0 + 440, u32(symbol('g_lat_extra'))
    finally:
        control('K1', 'set', 0)
        time.sleep(0.2)
    assert u32(symbol('g_lat_extra')) == trim0, u32(symbol('g_lat_extra'))

    # --- SHIFT + DATA closes; loops survive and the app does not restart
    close_looper()
    time.sleep(8)                          # the reported crash comes "after a few seconds"
    after = snapshot()
    assert after[0][1] == loop_length, after[0]
    assert ssh('systemctl show ac50 -p NRestarts').strip().endswith('0'), 'app restarted'
    assert int(ssh('pidof "MPC Sample"')) == pid, 'app pid changed'
    print(f'live looper: loop {loop_length} frames ({loop_length / 44100:.2f}s), '
          f'{loop_rms:.3f} rms at {loop_hz:.0f} Hz, ring {cal_frames} frames, '
          f'take latency {take_lat} frames, sync {pulse}, '
          f'quantized {quant_rec} -> {quant_len}')


if __name__ == '__main__':
    main()
