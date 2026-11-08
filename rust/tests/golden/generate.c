/* Golden-fixture generator for the Rust port.
 *
 * It links the original C implementations of the DSP that moved to Rust and
 * writes deterministic input/output pairs.  The Rust integration test replays
 * the same inputs and compares against these outputs, so a behavioural
 * difference between the two implementations fails the build instead of the
 * device.
 *
 * Build and run with `./generate.sh`; the fixtures are committed.
 */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "hpss.h"
#include "looper.h"
#include "spectral.h"

/* looper.c is pulled in directly: the fixtures also capture the loop buffers,
 * which are file-scope statics in that translation unit. */
#include "reference/looper_reference.c"

/* Defined by looper.c and read by the emulator test through the ELF symbol. */
extern volatile unsigned g_pad_stride;

#define RATE 44100u
#define HPSS_FRAMES 8192u
#define SPECTRAL_FRAMES 96000u

static void die(const char *what)
{
    fprintf(stderr, "generate: %s\n", what);
    exit(1);
}

static void write_file(const char *name, const void *data, size_t bytes)
{
    FILE *f = fopen(name, "wb");
    if (!f)
        die(name);
    if (bytes && fwrite(data, 1, bytes, f) != bytes)
        die("short write");
    if (fclose(f) != 0)
        die("close failed");
}

/* Deterministic pseudo-noise so the percussion part is reproducible. */
static uint32_t rng_state = 0x12345678u;

static float noise(void)
{
    rng_state = rng_state * 1664525u + 1013904223u;
    return ((float)((rng_state >> 8) & 0xffff) / 32768.0f) - 1.0f;
}

/* A harmonic bed (three partials, slow tremolo) plus a noise burst every 4096
 * frames: enough structure for both the HPSS mask and the onset detector to have
 * something real to decide on. */
static float source_sample(unsigned frame, unsigned channel)
{
    float detune = channel ? 0.35f : 0.0f;
    float tremolo = 0.65f + 0.35f * sinf(2.0f * 3.14159265f * 0.7f * (float)frame / (float)RATE);
    float tone = 0.42f * sinf(6.2831853f * 110.0f * (float)frame / (float)RATE + detune)
               + 0.21f * sinf(6.2831853f * 220.0f * (float)frame / (float)RATE + detune)
               + 0.11f * sinf(6.2831853f * 331.0f * (float)frame / (float)RATE);
    float burst = 0.0f;
    unsigned phase = frame % 4096u;
    if (phase < 700u)
        burst = 0.55f * noise() * expf(-(float)phase / 120.0f);
    return tone * tremolo + burst;
}

/* HPSS: one split of the shared source. */
static void golden_hpss(void)
{
    unsigned frames = HPSS_FRAMES;
    float *stereo = calloc((size_t)frames * 2, sizeof(float));
    if (!stereo)
        die("hpss input");
    for (unsigned frame = 0; frame < frames; ++frame)
        for (unsigned channel = 0; channel < 2; ++channel)
            stereo[frame * 2 + channel] = source_sample(frame, channel);
    write_file("hpss_input.bin", stereo, (size_t)frames * 2 * sizeof(float));

    float *harmonic = 0;
    float *percussive = 0;
    if (hpss_split(stereo, frames, 2, RATE, &harmonic, &percussive) != 0)
        die("hpss_split failed");
    write_file("hpss_harmonic.bin", harmonic, (size_t)frames * 2 * sizeof(float));
    write_file("hpss_percussive.bin", percussive, (size_t)frames * 2 * sizeof(float));
    free(stereo);
    free(harmonic);
    free(percussive);
}

/* Spectral Chop: onset positions on a longer take. */

/* A take that starts with silence and then hits: this is the case the leading
 * boundary rule exists for, and the one the evenly-spaced fixture above never
 * reaches. */
static void golden_spectral_late(void)
{
    unsigned frames = SPECTRAL_FRAMES;
    float *stereo = calloc((size_t)frames * 2, sizeof(float));
    if (!stereo)
        die("spectral late input");
    unsigned lead = RATE / 4; /* 250 ms of silence */
    for (unsigned frame = lead; frame < frames; ++frame) {
        unsigned region = (frame - lead) / (RATE / 3);
        float frequency = (region % 2) ? 220.0f : 330.0f;
        unsigned phase = (frame - lead) % (RATE / 3);
        float envelope = expf(-(float)phase / (float)(RATE / 8));
        float value = 0.4f * envelope * sinf(6.2831853f * frequency * (float)frame / (float)RATE);
        if (phase < 300u)
            value += 0.35f * noise() * expf(-(float)phase / 60.0f);
        stereo[frame * 2] = value;
        stereo[frame * 2 + 1] = value * 0.9f;
    }
    write_file("spectral_late_input.bin", stereo, (size_t)frames * 2 * sizeof(float));

    unsigned positions[16] = {0};
    unsigned detected = 0;
    int count = spectral_onsets(stereo, frames, 2, RATE, positions, 16, &detected);
    if (count < 0)
        die("spectral_onsets late failed");
    unsigned header[2] = {(unsigned)count, detected};
    write_file("spectral_late_onsets.bin", header, sizeof header);
    write_file("spectral_late_positions.bin", positions, sizeof positions);
    free(stereo);
}

static void golden_spectral(void)
{
    unsigned frames = SPECTRAL_FRAMES;
    float *stereo = calloc((size_t)frames * 2, sizeof(float));
    if (!stereo)
        die("spectral input");
    /* One onset every 0.4 s, alternating pitch, so the detector has a reason to
     * cut and a reason to skip. */
    for (unsigned frame = 0; frame < frames; ++frame) {
        unsigned region = frame / (RATE * 2 / 5);
        float frequency = (region % 2) ? 196.0f : 261.63f;
        unsigned phase = frame % (RATE * 2 / 5);
        float envelope = expf(-(float)phase / (float)(RATE / 6));
        float value = 0.5f * envelope * sinf(6.2831853f * frequency * (float)frame / (float)RATE);
        if (phase < 400u)
            value += 0.3f * noise() * expf(-(float)phase / 80.0f);
        stereo[frame * 2] = value;
        stereo[frame * 2 + 1] = value * 0.9f;
    }
    write_file("spectral_input.bin", stereo, (size_t)frames * 2 * sizeof(float));

    unsigned positions[16] = {0};
    unsigned detected = 0;
    int count = spectral_onsets(stereo, frames, 2, RATE, positions, 16, &detected);
    if (count < 0)
        die("spectral_onsets failed");
    unsigned header[2] = {(unsigned)count, detected};
    write_file("spectral_onsets.bin", header, sizeof header);
    write_file("spectral_positions.bin", positions, sizeof positions);
    free(stereo);
}

/* The looper: a scripted performance, recorded into one flat trace. */
#define BLOCK 512u
#define TRACE_FRAMES 262144u

static int32_t trace_in[2 * TRACE_FRAMES];
static int32_t trace_out[2 * TRACE_FRAMES];
static unsigned trace_pos;

static float tone(float hz, unsigned frame)
{
    return 0.5f * sinf(6.2831853f * hz * (float)frame / (float)RATE);
}

static void render(unsigned frames)
{
    if (trace_pos + frames > TRACE_FRAMES)
        die("trace overflow");
    unsigned done = 0;
    while (done < frames) {
        unsigned block = frames - done;
        if (block > BLOCK)
            block = BLOCK;
        for (unsigned i = 0; i < block; ++i) {
            unsigned frame = trace_pos + i;
            trace_in[(trace_pos + i) * 2] = (int32_t)(tone(440.0f, frame) * 2147483647.0f);
            trace_in[(trace_pos + i) * 2 + 1] = (int32_t)(tone(660.0f, frame) * 2147483647.0f);
        }
        looper_input_s32_interleaved(trace_in + trace_pos * 2, 2, block);
        memset(trace_out + trace_pos * 2, 0, (size_t)block * 2 * sizeof(int32_t));
        looper_render_s32(trace_out + trace_pos * 2, 2, block);
        trace_pos += block;
        done += block;
    }
}

static void golden_looper(void)
{
    looper_set_latency_ex(2048, 'B');
    if (!looper_activate())
        die("looper_activate failed");
    render(4410);

    /* Record one second on pad 1, close it, drain, then play. */
    looper_pad(0, 0, 0);
    render(RATE);
    looper_pad(0, 0, 0);
    render(8192);
    looper_pad(0, 0, 0);   /* stop */
    render(1024);
    looper_pad(0, 0, 0);   /* play again */
    render(4096);

    /* Overdub, then close back into playback. */
    looper_pad(0, 1, 0);
    render(4096);
    looper_pad(0, 1, 0);
    render(8192);

    /* Freeze the sync pulse on the recorded length and record a second loop,
     * which must be quantised to whole pulses. */
    looper_button(0x4a, 1, 0, 0);
    render(1024);
    looper_pad(1, 0, 0);
    render(30000);
    looper_pad(1, 0, 0);
    render(8192);

    /* Stop everything, then play both loops at half master volume. */
    looper_button(0x51, 1, 0, 0);
    render(1024);
    looper_set_master(64);
    looper_pad(0, 0, 0);
    render(4096);
    looper_pad(1, 0, 0);
    render(4096);

    /* Clear the unused pad, snapshot while the view is still open, then close. */
    looper_pad(2, 0, 1);
    struct looper_view view;
    if (!looper_snapshot(&view))
        die("looper_snapshot failed");
    write_file("looper_view.bin", &view, sizeof view);
    printf("looper trace: %u frames, pad stride %u\n", trace_pos, (unsigned)g_pad_stride);
    printf("pad 1: state %u len %u rec %u lat %u\n", view.pads[0].state,
           view.pads[0].len_frames, view.pads[0].rec_frames, view.lat_frames);
    printf("pad 2: state %u len %u; sync %d frames %u; master ok\n",
           view.pads[1].state, view.pads[1].len_frames, view.sync, view.sync_frames);
    printf("view size %zu\n", sizeof view);
    looper_close();
    render(2048);
    if (looper_snapshot(&view))
        die("snapshot must report a closed view");

    /* The recorded loops themselves, so a difference in the mix can be told
     * apart from a difference in the recording. */
    write_file("looper_pad0.bin", g_pool, 16384 * sizeof(float));
    write_file("looper_pad1.bin", g_pool + LP_MAX_FRAMES, 16384 * sizeof(float));
    write_file("looper_input.bin", trace_in, (size_t)trace_pos * 2 * sizeof(int32_t));
    write_file("looper_output.bin", trace_out, (size_t)trace_pos * 2 * sizeof(int32_t));
    write_file("looper_frames.bin", &trace_pos, sizeof trace_pos);
}

/* Sample conversion: the exact pairs the looper recorded, with what the C
 * expression produces for them.  A one-ulp difference here shows up as a tiny
 * offset in every loop that was recorded from the input. */
static void golden_convert(void)
{
    enum { PAIRS = 4096 };
    int32_t pairs[PAIRS * 2];
    float out[PAIRS];
    for (unsigned i = 0; i < PAIRS; ++i) {
        unsigned frame = 100000u + i;
        pairs[i * 2] = (int32_t)(tone(440.0f, frame) * 2147483647.0f);
        pairs[i * 2 + 1] = (int32_t)(tone(660.0f, frame) * 2147483647.0f);
        out[i] = ((float)pairs[i * 2] + (float)pairs[i * 2 + 1]) * (0.5f / 2147483648.0f);
    }
    write_file("convert_pairs.bin", pairs, sizeof pairs);
    write_file("convert_out.bin", out, sizeof out);
}

int main(void)
{
    golden_convert();
    golden_hpss();
    golden_spectral();
    golden_spectral_late();
    golden_looper();
    printf("golden fixtures written\n");
    return 0;
}
