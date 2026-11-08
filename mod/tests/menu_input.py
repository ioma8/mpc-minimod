"""Exercise production native menu hooks: event ownership and repeated deltas."""
from pathlib import Path
import subprocess
import tempfile

source = (Path(__file__).resolve().parents[1] / 'src/minimod.c').read_text()
native = source[source.index('/* Native DATA input:'):source.index('static void hpss_make_names(')]
cc_start = source.index('/* A real CC event captured')
cc = source[cc_start:source.index('/* Whole-heap fingerprint:', cc_start)]
harness = r'''
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <math.h>
#define MINIMOD_DEBUG 0
#define mlog(...) ((void)0)
static int g_spectral_active, g_hpss_menu, g_hpss_busy, g_hpss_sel;
static int g_bass_edit;                 /* Bass Synth page state, stubbed off */
static float g_bass_knob[3];
static void bass_preview_set_params(float a, float b, float c) {
    (void)a; (void)b; (void)c;
}
static int paints, forwarded, cc_forwarded;
static void hpss_repaint(void) { ++paints; }
static void (*orig_cc)(void *, unsigned, float, float);
static int rd(uintptr_t addr, void *dst, size_t n) {
    memcpy(dst, (const void *)addr, n);
    return 1;
}
'''+native+cc+r'''
static void original_signal(void *self, int delta) { (void)self; (void)delta; ++forwarded; }
static void original_cc(void *ev, unsigned raw, float lo, float range) {
    (void)ev; (void)raw; (void)lo; (void)range; ++cc_forwarded;
}
int main(void) {
    g_data_signal = 0x1000;
    orig_data_signal = original_signal;
    orig_cc = original_cc;
    g_hpss_menu = 1;
    /* Same direction repeated is a sequence of events, not one changed value. */
    for (int i=1; i<=100; ++i) {
        data_signal((void*)0x1000, 1);
        assert(g_hpss_sel == i%7);   /* SAMPLE OPTIONS has 7 rows */
    }
    for (int i=1; i<=100; ++i) {
        data_signal((void*)0x1000, -1);
        assert(g_hpss_sel == (100-i)%7);
    }
    assert(paints == 200 && forwarded == 0);
    data_signal((void*)0x1000, 0);
    assert(paints == 200);
    /* Other integer events are never treated as the wheel. */
    data_signal((void*)0x2000, 1);
    assert(forwarded == 1 && g_hpss_sel == 0);
    g_hpss_busy = 1;
    data_signal((void*)0x1000, 1);
    assert(forwarded == 1 && g_hpss_sel == 0);
    g_hpss_busy = 0;
    g_hpss_menu = 0;
    g_spectral_active = 1;
    data_signal((void*)0x1000, -1);
    assert(forwarded == 1 && g_hpss_sel == 0);
    g_spectral_active = 0;
    data_signal((void*)0x1000, 1);
    assert(forwarded == 2);  /* closed menu preserves stock encoder */
    unsigned char event[32] = {0};
    g_hpss_menu = 1;
    event[8] = 0x10;
    cc_capture(event, 60, 0, 127);
    assert(cc_forwarded == 0);  /* no knob leaks behind the modal */
    event[8] = 0x64;
    cc_capture(event, 1, 0, 127);
    assert(cc_forwarded == 1 && g_hpss_sel == 0); /* decoder is not a second wheel path */
    g_hpss_menu = 0;
    event[8] = 0x12;
    cc_capture(event, 80, 0, 127);
    assert(cc_forwarded == 2);
    puts("PASS: native DATA events, ownership, busy, stock forwarding, knob suppression");
}
'''
with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    (root / 'check.c').write_text(harness)
    subprocess.run(['cc', '-std=c11', str(root / 'check.c'), '-o', str(root / 'check')], check=True)
    subprocess.run([str(root / 'check')], check=True)
