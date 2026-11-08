"""Compile the production WAV helpers and check decoding plus header refusal.

The valid cases cover every format the app writes; the malformed cases are the
ones that used to hang or overrun the scanner (channel counts wider than the
16 KiB read buffer, bad bit depths, truncated frames).
"""
from pathlib import Path
import struct
import subprocess
import tempfile

source = (Path(__file__).resolve().parents[1] / 'src/minimod.c').read_text()
helpers = source[source.index('static int wav_info('):source.index('/* --------------------------------------------------------- trim parameter */')]
with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    code = '''#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
#include <math.h>
#include <signal.h>
#define mlog(...) ((void)0)
#define rd(...) 1
static void boom(int s) { (void)s; _exit(42); }
''' + helpers + '''
int main(int argc, char **argv) {
 unsigned n,r,c,o,b,f,first=0,last=0;
 signal(SIGALRM, boom);
 alarm(3);
 if (!wav_info(argv[1],&n,&r,&c,&o,&b,&f)) { puts("info=0"); return 0; }
 int rc = wav_edges(argv[1],n,o,c,b,f,&first,&last);
 printf("info=1 rc=%d %u %u\\n", rc, first, last);
 return 0;
}
'''
    (root/'check.c').write_text(code)
    subprocess.run(['cc', str(root/'check.c'), '-lm', '-o', str(root/'check')], check=True)

    def wav(ch, bits, frames, encoding=1, fmt_size=16):
        bps = bits // 8
        data = b''.join(struct.pack('<f', v) for v in [0.0] * (frames * ch)) if encoding == 3 else b'\0' * (frames * ch * bps)
        fmt = struct.pack('<HHIIHH', encoding, ch, 44100, 44100 * bps * ch, bps, bits)
        fmt = fmt.ljust(fmt_size, b'\0')
        return b'RIFF' + struct.pack('<I', 36 + len(data)) + b'WAVEfmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(data)) + data

    def run(payload):
        p = root / 'test.wav'
        p.write_bytes(payload)
        return subprocess.run([str(root/'check'), str(p)], capture_output=True, text=True, timeout=10)

    values = [0.0005] * 100 + [0.5, -1.0, 0.25] + [-0.0005] * 100
    for format, bits in [(1, 16), (1, 24), (1, 32), (3, 32)]:
        if format == 3:
            data = b''.join(struct.pack('<f', v) for v in values)
        else:
            data = b''.join(int(v * (1 << (bits - 1))).to_bytes(bits // 8, 'little', signed=True) for v in values)
        fmt = struct.pack('<HHIIHH', format, 1, 44100, 44100 * bits // 8, bits // 8, bits)
        wav_bytes = b'RIFF' + struct.pack('<I', 36 + len(data)) + b'WAVEfmt ' + struct.pack('<I', 16) + fmt + b'data' + struct.pack('<I', len(data)) + data
        r = run(wav_bytes)
        assert r.returncode == 0 and r.stdout.strip() == 'info=1 rc=1 100 102', (format, bits, r.returncode, r.stdout, r.stderr)
        print('PASS', format, bits, r.stdout.strip())

    # Header the scanner must refuse (and not hang on).
    malformed = {
        'no channels': wav(0, 16, 8),
        'three channels': wav(3, 16, 8),
        'wide frame': wav(5000, 32, 1),
        'bad bit depth': wav(1, 12, 8),
    }
    for name, payload in malformed.items():
        r = run(payload)
        assert r.returncode != 42, f'{name}: scanner hung'
        assert r.returncode == 0 and r.stdout.strip() == 'info=0', (name, r.returncode, r.stdout)
        print('PASS reject', name, r.stdout.strip())

    # Truncated fmt chunk: declared 40 bytes, file ends after 16.
    payload = wav(2, 16, 8, fmt_size=40)
    r = run(payload[:-((40 - 16) + 8 + 8)])
    assert r.returncode == 0 and r.stdout.strip() == 'info=0', (r.returncode, r.stdout)
    print('PASS reject truncated fmt')
