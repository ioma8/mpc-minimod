"""Compile the hook engine's instruction classifier and branch encoder.

The prologue copy in install_hook() is only safe for instructions that do not
compute an address from their own location; a missed PC-relative class becomes a
wild branch in the trampoline.  B.cond was missing before this test existed.
"""
from pathlib import Path
import subprocess
import tempfile

source = (Path(__file__).resolve().parents[1] / 'src/minimod.c').read_text()
start = source.index('static uint32_t enc_b(')
end = source.index('static void *install_hook(')
helpers = source[start:end]

cases = [
    # instruction, expected pc_relative
    (0x14000004, 1),   # b +16
    (0x94000004, 1),   # bl +16
    (0x54000040, 1),   # b.eq +8
    (0x54FFFFE0, 1),   # b.cond -4 (bit 4 clear)
    (0x54000010, 0),   # bit 4 set: not a B.cond encoding
    (0x34000040, 1),   # cbz x0, +8
    (0x35000040, 1),   # cbnz x0, +8
    (0x36000040, 1),   # tbz x0, #0, +8
    (0x37000040, 1),   # tbnz
    (0x10000020, 1),   # adr x0, +4
    (0x90000020, 1),   # adrp x0, +0x1000
    (0x18000040, 1),   # ldr w0, literal +8
    (0x98000040, 1),   # ldrsw x0, literal +8
    (0xD8000040, 1),   # prfm literal
    (0xD61F0200, 1),   # br x16
    (0xD63F0200, 1),   # blr x16
    (0xD65F03C0, 1),   # ret
    (0xD503201F, 0),   # nop
    (0xA9BF7BFD, 0),   # stp x29, x30, [sp, #-16]!
    (0x910003FD, 0),   # mov x29, sp
    (0xB9400000, 0),   # ldr w0, [x0]
]

with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    code = '''#include <stdio.h>
#include <stdint.h>
''' + helpers + '''
int main(void) {
    static const struct { unsigned insn; int rel; } cases[] = {
''' + ''.join(f'        {{0x{c:08X}u, {r}}},\n' for c, r in cases) + '''    };
    for (unsigned i = 0; i < sizeof cases / sizeof cases[0]; ++i) {
        int got = pc_relative(cases[i].insn);
        if (got != cases[i].rel) {
            printf("pc_relative(%08x)=%d want %d\\n", cases[i].insn, got, cases[i].rel);
            return 1;
        }
    }
    if (enc_b(0x1000, 0x1000 + 4) != 0x14000001u) return 2;
    if (enc_b(0x1000, 0x1000 - 4) != 0x17FFFFFFu) return 3;
    if (enc_b(0x1000, 0x1000 + (1u << 27)) != 0) return 4;
    if (enc_b(0x1000, 0x1000 - (1u << 27)) != 0) return 5;
    if (enc_b(0x2000, 0x2000 + (1u << 27) - 4) == 0) return 6;
    puts("hook engine ok");
    return 0;
}
'''
    (root/'check.c').write_text(code)
    subprocess.run(['cc', str(root/'check.c'), '-o', str(root/'check')], check=True)
    subprocess.run([str(root/'check')], check=True)
