#include "bass_sample.h"

#include "PhysicalModeling/stringvoice.h"
#include "Filters/svf.h"

#include <cmath>
#include <cstdlib>
#include <atomic>
#include <cstdint>
#include <cstring>
#include <limits>

namespace {
constexpr float kNoteHz = 65.4063913f; // C2, MIDI 36
constexpr float kDuration = 1.5f;
constexpr float kGate = 0.4f;
constexpr unsigned kPadCount = 16;
constexpr unsigned kVoiceCount = 4;

float clamp01(float value)
{
    return value < 0.f ? 0.f : value > 1.f ? 1.f : value;
}

struct PreviewVoice {
    daisysp::StringVoice string;
    daisysp::Svf filter;
    unsigned age = 0;
    unsigned quiet = 0;
    float level = 0.f;
    float decay = 0.6f;
    bool active = false;
};

/* The preview sounds like the rendered sample only with the same envelope:
 * full level for the key-down part, then the render's exponential tail. */
/* The preview is an audition, not the saved sample: decay from the first
 * sample so a stack of pads can never turn into a sustained drone. */
constexpr float kPreviewLength = 1.2f;

std::atomic<bool> preview_enabled{false};
std::atomic<float> preview_brightness{0.52f};
std::atomic<float> preview_excite{0.18f};
std::atomic<float> preview_cutoff{0.56f};
std::atomic<uint32_t> pending_velocity[kPadCount]{};
PreviewVoice voices[kVoiceCount];
unsigned voice_cursor;
unsigned preview_rate;
bool preview_initialized;
bool preview_was_enabled;

uint32_t float_bits(float value)
{
    uint32_t bits;
    std::memcpy(&bits, &value, sizeof bits);
    return bits;
}

float bits_float(uint32_t bits)
{
    float value;
    std::memcpy(&value, &bits, sizeof value);
    return value;
}

void init_preview(unsigned rate)
{
    preview_rate = rate;
    for (auto &voice : voices) {
        voice.string.Init(static_cast<float>(rate));
        voice.string.SetSustain(false);
        voice.filter.Init(static_cast<float>(rate));
        voice.active = false;
    }
    voice_cursor = 0;
    preview_initialized = true;
}

void stop_preview()
{
    for (auto &voice : voices)
        voice.active = false;
    for (auto &pending : pending_velocity)
        pending.store(0, std::memory_order_release);
}

void trigger_preview(unsigned pad, float velocity, float brightness,
                     float excite, float cutoff)
{
    PreviewVoice &voice = voices[voice_cursor++ % kVoiceCount];
    voice.string.Reset();
    voice.string.SetFreq(kNoteHz * std::pow(2.f, static_cast<float>(pad) / 12.f));
    voice.string.SetAccent(0.65f + excite * 0.35f);
    voice.string.SetStructure(0.25f + excite * 0.6f);
    voice.string.SetBrightness(0.05f + brightness * 0.9f);
    voice.string.SetDamping(0.35f + (1.f - brightness) * 0.55f);
    voice.string.Trig();
    voice.filter.Init(static_cast<float>(preview_rate));
    const float cutoff_hz = 700.f * std::pow(24.f, cutoff);
    voice.filter.SetFreq(cutoff_hz < preview_rate * 0.45f
                             ? cutoff_hz : preview_rate * 0.45f);
    voice.filter.SetRes(0.05f);
    voice.filter.SetDrive(excite * 0.35f);
    voice.level = 0.5f * clamp01(velocity);
    voice.decay = 0.35f + excite * 0.15f;
    voice.age = 0;
    voice.quiet = 0;
    voice.active = true;
}
}

extern "C" int bass_sample_render(unsigned rate, float harmonics, float drive,
                                  float cutoff, float **output, size_t *out_frames)
{
    if (!output || !out_frames)
        return -1;
    *output = nullptr;
    *out_frames = 0;
    if (rate < 8000 || rate > 192000)
        return -1;

    const size_t frames = static_cast<size_t>(rate * kDuration);
    if (frames > std::numeric_limits<size_t>::max() / (2 * sizeof(float)))
        return -1;
    float *audio = static_cast<float *>(std::calloc(frames * 2, sizeof(float)));
    if (!audio)
        return -1;

    harmonics = clamp01(harmonics);
    drive = clamp01(drive);
    cutoff = clamp01(cutoff);

    daisysp::StringVoice string;
    string.Init(static_cast<float>(rate));
    string.SetSustain(false);
    string.SetFreq(kNoteHz);
    string.SetAccent(0.65f + drive * 0.35f);
    string.SetStructure(0.25f + drive * 0.6f);
    string.SetBrightness(0.05f + harmonics * 0.9f);
    string.SetDamping(0.35f + (1.f - harmonics) * 0.55f);
    string.Trig();

    daisysp::Svf filter;
    filter.Init(static_cast<float>(rate));
    const float cutoff_hz = 700.f * std::pow(24.f, cutoff);
    filter.SetFreq(cutoff_hz < rate * 0.45f ? cutoff_hz : rate * 0.45f);
    filter.SetRes(0.05f);
    filter.SetDrive(drive * 0.35f);

    float peak = 0.f;
    for (size_t i = 0; i < frames; ++i) {
        const float sample = string.Process(i == 0);
        filter.Process(sample);
        float value = filter.Low();
        if (i > static_cast<size_t>(rate * kGate)) {
            const float t = static_cast<float>(i - static_cast<size_t>(rate * kGate)) / rate;
            value *= std::exp(-t / (0.48f + drive * 0.25f));
        }
        audio[2 * i] = audio[2 * i + 1] = value;
        const float magnitude = std::fabs(value);
        if (magnitude > peak)
            peak = magnitude;
    }

    if (peak < 1e-8f) {
        std::free(audio);
        return -1;
    }
    const float gain = 0.86f / peak;
    for (size_t i = 0; i < frames * 2; ++i)
        audio[i] *= gain;
    *output = audio;
    *out_frames = frames;
    return 0;
}

extern "C" void bass_preview_reset(void)
{
    stop_preview();
    for (auto &pending : pending_velocity)
        pending.store(0, std::memory_order_release);
    preview_was_enabled = false;
}

extern "C" void bass_preview_enable(int enabled)
{
    preview_enabled.store(enabled != 0, std::memory_order_release);
}

extern "C" void bass_preview_set_params(float brightness, float excite,
                                         float cutoff)
{
    preview_brightness.store(clamp01(brightness), std::memory_order_release);
    preview_excite.store(clamp01(excite), std::memory_order_release);
    preview_cutoff.store(clamp01(cutoff), std::memory_order_release);
}

extern "C" void bass_preview_note(unsigned pad, float velocity)
{
    if (pad >= kPadCount || velocity <= 0.f || !std::isfinite(velocity))
        return;
    pending_velocity[pad].store(float_bits(clamp01(velocity)),
                                std::memory_order_release);
}

extern "C" float bass_preview_mix(float **channels, size_t frames,
                                  unsigned channel_count, unsigned rate)
{
    if (!preview_enabled.load(std::memory_order_acquire) || !channels ||
        channel_count == 0 || frames == 0 || rate < 8000 || rate > 192000) {
        if (preview_was_enabled)
            stop_preview();
        preview_was_enabled = false;
        return 0.f;
    }
    if (!preview_initialized || preview_rate != rate)
        init_preview(rate);
    preview_was_enabled = true;

    const float brightness = preview_brightness.load(std::memory_order_acquire);
    const float excite = preview_excite.load(std::memory_order_acquire);
    const float cutoff = preview_cutoff.load(std::memory_order_acquire);
    const float cutoff_hz = 700.f * std::pow(24.f, cutoff);
    const float safe_cutoff = cutoff_hz < rate * 0.45f ? cutoff_hz : rate * 0.45f;

    for (unsigned pad = 0; pad < kPadCount; ++pad) {
        const uint32_t bits = pending_velocity[pad].exchange(0, std::memory_order_acq_rel);
        if (bits)
            trigger_preview(pad, bits_float(bits), brightness, excite, cutoff);
    }
    for (auto &voice : voices) {
        if (!voice.active)
            continue;
        voice.string.SetAccent(0.65f + excite * 0.35f);
        voice.string.SetStructure(0.25f + excite * 0.6f);
        voice.string.SetBrightness(0.05f + brightness * 0.9f);
        voice.string.SetDamping(0.35f + (1.f - brightness) * 0.55f);
        voice.filter.SetFreq(safe_cutoff);
        voice.filter.SetDrive(excite * 0.35f);
    }

    float peak = 0.f;
    for (size_t frame = 0; frame < frames; ++frame) {
        float sample = 0.f;
        for (auto &voice : voices) {
            if (!voice.active)
                continue;
            voice.filter.Process(voice.string.Process());
            const float t = static_cast<float>(voice.age) / rate;
            const float env = std::exp(-t / voice.decay);
            const float value = voice.filter.Low() * voice.level * env;
            sample += value;
            const float magnitude = std::fabs(value);
            if (magnitude < 1e-5f)
                ++voice.quiet;
            else
                voice.quiet = 0;
            if (++voice.age > rate * kPreviewLength || voice.quiet > rate / 3)
                voice.active = false;
        }
        if (sample > 0.85f) sample = 0.85f;
        if (sample < -0.85f) sample = -0.85f;
        if (channel_count == 1) {
            float mixed = channels[0][frame] + sample;
            if (mixed > 1.f) mixed = 1.f;
            if (mixed < -1.f) mixed = -1.f;
            channels[0][frame] = mixed;
        } else {
            float left = channels[0][frame] + sample;
            float right = channels[1][frame] + sample;
            if (left > 1.f) left = 1.f;
            if (left < -1.f) left = -1.f;
            if (right > 1.f) right = 1.f;
            if (right < -1.f) right = -1.f;
            channels[0][frame] = left;
            channels[1][frame] = right;
        }
        const float magnitude = std::fabs(sample);
        if (magnitude > peak)
            peak = magnitude;
    }
    return peak;
}
