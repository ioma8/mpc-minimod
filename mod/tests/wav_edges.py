"""Compile the production WAV helpers and test integer/float decoding equivalence."""
from pathlib import Path
import struct
import subprocess
import tempfile

source = (Path(__file__).resolve().parents[1] / 'src/mpcmini.c').read_text()
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
#define mlog(...) ((void)0)
#define rd(...) 1
''' + helpers + '''
int main(int argc, char **argv) {
 unsigned n,r,c,o,b,f,first,last;
 if (!wav_info(argv[1],&n,&r,&c,&o,&b,&f)) return 1;
 if (!wav_edges(argv[1],n,o,c,b,f,&first,&last)) return 2;
 printf("%u %u\\n",first,last);
 return 0;
}
'''
    (root/'check.c').write_text(code)
    subprocess.run(['cc', str(root/'check.c'), '-lm', '-o', str(root/'check')], check=True)
    values = [0.0005]*100 + [0.5, -1.0, 0.25] + [-0.0005]*100
    for format, bits in [(1,16),(1,24),(1,32),(3,32)]:
        if format == 3:
            data = b''.join(struct.pack('<f',v) for v in values)
        else:
            data = b''.join(int(v*(1 << (bits-1))).to_bytes(bits//8,'little',signed=True) for v in values)
        fmt = struct.pack('<HHIIHH',format,1,44100,44100*bits//8,bits//8,bits)
        wav = root/'test.wav'
        wav.write_bytes(b'RIFF'+struct.pack('<I',36+len(data))+b'WAVEfmt '+struct.pack('<I',16)+fmt+b'data'+struct.pack('<I',len(data))+data)
        result = subprocess.check_output([str(root/'check'),str(wav)],text=True).strip()
        assert result == '100 102', (format,bits,result)
        print('PASS',format,bits,result)
