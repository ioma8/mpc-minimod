/* Golden fixture for the bass synth.
 *
 * The reference draws its string excitation and dispersion noise from libc
 * `rand()`, whose sequence differs between the host and the device, so the
 * reference sources are compiled with `-Drand=golden_rand`: every `rand()` call
 * becomes a call to the deterministic generator below.  The Rust test feeds the
 * same sequence through `SequenceNoise`, which makes the whole synth — string,
 * damping filter, DC blocker, crossfade, state-variable filter — comparable
 * sample for sample.
 */
#include <cstdio>
#include <cstdint>
#include <cstdlib>

#include "bass_sample.h"

extern "C" int golden_rand(void)
{
    static uint32_t state = 1;
    state = state * 1103515245u + 12345u;
    return static_cast<int>(state & 0x7fffffffu);
}

static void write_file(const char *name, const void *data, size_t bytes)
{
    FILE *f = fopen(name, "wb");
    if (!f) {
        fprintf(stderr, "generate_bass: cannot open %s\n", name);
        exit(1);
    }
    if (bytes && fwrite(data, 1, bytes, f) != bytes) {
        fprintf(stderr, "generate_bass: short write\n");
        exit(1);
    }
    fclose(f);
}

int main(void)
{
    float *audio = nullptr;
    size_t frames = 0;
    if (bass_sample_render(44100, 0.55f, 0.35f, 0.62f, &audio, &frames) != 0) {
        fprintf(stderr, "generate_bass: render failed\n");
        return 1;
    }
    write_file("bass_audio.bin", audio, frames * 2 * sizeof(float));
    write_file("bass_frames.bin", &frames, sizeof frames);
    free(audio);
    printf("bass: %zu frames\n", frames);
    return 0;
}
