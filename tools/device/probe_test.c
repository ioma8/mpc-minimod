/* Host test for probe-tanf.c: the interposer cannot be exercised off-device, but
 * the recording logic (Hz conversion, dedup, log format) can, so it is.
 *
 *   cc -DPROBE_TEST -o probe_test probe_test.c && ./probe_test
 */
#define PROBE_TEST 1
#include "probe-tanf.c"

#include <assert.h>
#include <stdio.h>
#include <string.h>

static int count_lines(const char *path, const char *needle) {
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    char line[256];
    int hits = 0;
    while (fgets(line, sizeof line, f)) {
        if (strstr(line, needle)) hits++;
    }
    fclose(f);
    return hits;
}

int main(void) {
    const char *path = "/tmp/probe_test.log";
    setenv("MPC_PROBE_LOG", path, 1);
    setenv("MPC_PROBE_FS", "44100", 1);

    /* A cutoff of 196.4 Hz is reached as tanf(pi * 196.4 / 44100). */
    float x = (float)(3.14159265358979323846 * 196.4 / 44100.0);
    probe_record_for_test(x, 0xfcc988);

    /* The same cutoff again, twice, must not add lines. */
    probe_record_for_test(x, 0xfcc988);
    probe_record_for_test(x * 1.00001f, 0xfcca4c);

    /* A clearly different cutoff must add exactly one line. */
    probe_record_for_test((float)(3.14159265358979323846 * 343.7 / 44100.0), 0xfccb0c);

    /* Values the app can legitimately pass but which are not cutoffs. */
    probe_record_for_test(0.0f, 0);
    probe_record_for_test(-1.0f, 0);
    probe_record_for_test(1e9f, 0);

    probe_close_for_test();

    assert(count_lines(path, "hz=196.4") == 1);
    assert(count_lines(path, "hz=343.7") == 1);
    assert(count_lines(path, "hz=") == 2);
    assert(count_lines(path, "lr=0xfcc988") == 1);
    assert(count_lines(path, "# mpc filter probe") == 1);

    printf("ok: 2 distinct cutoffs logged, duplicates and non-cutoffs rejected\n");
    remove(path);
    return 0;
}
