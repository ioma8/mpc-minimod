#include "bass_sample.h"

#include <math.h>
#include <stdint.h>
#include <stdlib.h>

#define BASS_NOTE_HZ 65.4063913f /* C2, MIDI note 36 */
#define BASS_SECONDS 1.5f
#define BASS_NOTE_OFF_SECONDS 0.4f
#define BASS_PARTIALS 16

int bass_sample_render(unsigned rate, float harmonics, float drive,
                       float cutoff, float **output, size_t *out_frames)
{
    if (!output || !out_frames)
        return -1;
    *output = NULL;
    *out_frames = 0;
    if (rate < 8000 || rate > 192000)
        return -1;

    size_t frames = (size_t)((double)rate * BASS_SECONDS);
    if (frames > SIZE_MAX / (2 * sizeof(float)))
        return -1;
    float *audio = calloc(frames * 2, sizeof *audio);
    if (!audio)
        return -1;

    if (harmonics < 0.f) harmonics = 0.f;
    if (harmonics > 1.f) harmonics = 1.f;
    if (drive < 0.f) drive = 0.f;
    if (drive > 1.f) drive = 1.f;
    if (cutoff < 0.f) cutoff = 0.f;
    if (cutoff > 1.f) cutoff = 1.f;

    float partial_amp[BASS_PARTIALS];
    float partial_sum = 0.f;
    float slope = 1.8f - harmonics * 1.25f;
    for (unsigned i = 0; i < BASS_PARTIALS; ++i) {
        partial_amp[i] = 1.f / powf((float)(i + 1), slope);
        partial_sum += partial_amp[i];
    }
    float cutoff_hz = 400.f * powf(30.f, cutoff);
    if (cutoff_hz > rate * 0.45f)
        cutoff_hz = rate * 0.45f;
    float filter = 1.f - expf(-6.28318530718f * cutoff_hz / rate);
    float drive_gain = 1.f + drive * 11.f;
    float drive_norm = tanhf(drive_gain);
    float phase = 0.f, low = 0.f, peak = 0.f;
    const float step = 6.28318530718f * BASS_NOTE_HZ / rate;
    const size_t note_off = (size_t)(rate * BASS_NOTE_OFF_SECONDS);
    const float release_tau = 0.37f;

    for (size_t n = 0; n < frames; ++n) {
        float t = (float)n / rate;
        float envelope = expf(-t / release_tau);
        /* The note-off starts a gentle extra fade, like a plucked bass string
           damping after the finger leaves the string. */
        if (n > note_off)
            envelope *= expf(-((float)(n - note_off) / rate) / 0.72f);
        float sample = 0.f;
        float angle = phase;
        for (unsigned i = 0; i < BASS_PARTIALS; ++i) {
            sample += partial_amp[i] * sinf(angle);
            angle += phase;
        }
        phase += step;
        if (phase >= 6.28318530718f)
            phase -= 6.28318530718f;
        sample = sample / partial_sum * 1.8f * envelope;
        sample = tanhf(sample * drive_gain) / drive_norm;
        low += filter * (sample - low);
        audio[n * 2] = low;
        audio[n * 2 + 1] = low;
        if (fabsf(low) > peak)
            peak = fabsf(low);
    }

    if (peak < 1e-6f) {
        free(audio);
        return -1;
    }
    float gain = 0.86f / peak;
    for (size_t i = 0; i < frames * 2; ++i)
        audio[i] *= gain;
    *output = audio;
    *out_frames = frames;
    return 0;
}
