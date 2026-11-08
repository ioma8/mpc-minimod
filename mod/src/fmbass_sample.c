/* NOT SHIPPED — an investigation probe, compiled out by default.
 *
 * Builds the firmware's native FMBass instrument through its H3 graph. The host
 * lifecycle is incomplete: the graph has no FMBass render node, dispatch is
 * intermittent, and successful dispatch renders digital silence. Details and
 * the probe chronology are in docs/re/BASS-SAMPLE.md.
 *
 * The guard makes an accidental link a no-op translation unit, so a stray
 * `src/fmbass_sample.c` in an object list cannot pull an unfinished probe into
 * the release. To run the probe, build with -DMINIMOD_FMBASS_PROBE=1 and link
 * this file *instead of* bass_sample.cpp — never both.
 */
#ifndef MINIMOD_FMBASS_PROBE
#define MINIMOD_FMBASS_PROBE 0
#endif

#if MINIMOD_FMBASS_PROBE

#include "fmbass_sample.h"

#include <math.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <unistd.h>

#define H3_CONTEXT_INIT 0x3c30c00
#define H3_GRAPH_CTOR 0x3c27800
#define H3_GRAPH_BUILD 0x3c28620
#define H3_GRAPH_PREPARE 0x3c28390
#define H3_GRAPH_PROCESS 0x3c27ca0
#define H3_GRAPH_DELETE 0x3c26ba0
#define H3_EVENT_PUMP 0x3d0c1a0 /* Ghidra VA 0x3e0c1a0 minus image base 0x100000 */
#define NOTE_FRAMES 66150
#define NOTE_ON_FRAMES 17640
#define BLOCK_FRAMES 512

typedef void (*ctx_init_fn)(void *);
typedef void (*graph_ctor_fn)(void *, uint64_t, uint64_t, uint64_t,
                              uint64_t, void *);
typedef void (*graph_build_fn)(void *, const char *, int);
typedef long (*graph_prepare_fn)(void *, void *);
typedef void (*part_process_fn)(void *, const float *const *, float *const *,
                                long, uint64_t *, long);
typedef void (*graph_delete_fn)(void *);
typedef void (*part_event_fn)(void *, uint64_t *);
typedef void (*part_param_fn)(float, void *, long);
typedef void (*event_pump_fn)(void *);

struct headless_event_host { void **vtable; };

/* FMBass callbacks follow the firmware ABI: the volume argument is in s0,
 * followed by object, instrument, event, channel, and result in x1..x4. */
__attribute__((target("branch-protection=bti")))
static void accept_note(float volume, void *self, void *instrument,
                        uint32_t event, long channel, uint32_t *accepted)
{
    (void)volume;
    (void)self;
    (void)instrument;
    (void)event;
    (void)channel;
    *accepted = 1;
}

static void *event_host_vtable[] = {(void *)accept_note};
static struct headless_event_host event_host = {event_host_vtable};

static void trace(const char *stage, const void *pointer, uintptr_t value)
{
    char line[128];
    int length = snprintf(line, sizeof(line), "%s %p %#lx\n", stage, pointer,
                          (unsigned long)value);
    int fd = open("/run/fmbass-render.log", O_WRONLY | O_CREAT | O_APPEND, 0644);
    if (fd >= 0) { (void)write(fd, line, (size_t)length); (void)close(fd); }
}

static float clamp01(float value)
{
    if (!isfinite(value) || value < 0.f) return 0.f;
    if (value > 1.f) return 1.f;
    return value;
}

int fmbass_sample_render(uintptr_t base, unsigned rate, float harmonics,
                         float drive, float cutoff, float **output,
                         size_t *frames)
{
    if (!output || !frames) return -1;
    *output = NULL;
    *frames = 0;
    if (!base || rate < 8000 || rate > 192000 ||
        NOTE_FRAMES > SIZE_MAX / (2 * sizeof(float))) return -1;

    float *audio = calloc(NOTE_FRAMES * 2, sizeof(*audio));
    void *context = calloc(1, 0x60);
    void *graph = calloc(1, 0x270);
    if (!audio || !context || !graph) goto fail_alloc;

    ctx_init_fn init_context = (ctx_init_fn)(base + H3_CONTEXT_INIT);
    graph_ctor_fn construct_graph = (graph_ctor_fn)(base + H3_GRAPH_CTOR);
    graph_build_fn build_graph = (graph_build_fn)(base + H3_GRAPH_BUILD);
    graph_prepare_fn prepare_graph = (graph_prepare_fn)(base + H3_GRAPH_PREPARE);
    graph_delete_fn delete_graph = (graph_delete_fn)(base + H3_GRAPH_DELETE);

    init_context(context);
    trace("context", context, rate);
    *(float *)context = (float)rate;
    *(uint32_t *)((char *)context + 8) = BLOCK_FRAMES;
    construct_graph(graph, 0, (uintptr_t)context, 0, 0, NULL);
    trace("graph-ctor", graph, 0);
    /* Mirror the firmware's embedded FMBass preset: the synth alone creates
       no render-node chain in the generic H3 wrapper. */
    static const char state[] =
        "<H3Patch><LevelTrim>0.5</LevelTrim>"
        "<H3Part Type='FMBass' Name='Synth'></H3Part>"
        "<H3Part Type='SynthMultiV2' Name='FX' Mute='true'></H3Part>"
        "</H3Patch>";
    build_graph(graph, state, (int)sizeof(state));
    trace("graph-build", graph, 0);

    void *patch = *(void **)((char *)graph + 0xb0);
    if (!patch || !prepare_graph(graph, context)) goto fail_graph;
    trace("prepared", patch, 0);
    trace("graph-render-node-head", graph,
          *(uintptr_t *)((char *)graph + 0x88));
    if (*(uintptr_t *)((char *)graph + 0x88))
        trace("graph-render-node-value", *(void **)((char *)graph + 0x88),
              *(uintptr_t *)*(void **)((char *)graph + 0x88));
    void **part_records = *(void ***)((char *)patch + 0x68);
    if (!part_records || !part_records[0]) goto fail_graph;
    void *part = *(void **)part_records[0];
    if (!part) goto fail_graph;
    trace("part", part, 0);

    void **vtable = *(void ***)part;
    part_process_fn process_part = (part_process_fn)vtable[11];
    part_param_fn set_param = (part_param_fn)vtable[21];
    part_event_fn send_event = (part_event_fn)vtable[13];
    trace("part-render-volume-after-prepare-bits", part,
          *(uint32_t *)((char *)part + 0x554));
    set_param(0.25f + 0.75f * clamp01(harmonics), part, 1);
    set_param(0.35f * clamp01(drive), part, 8);
    set_param(0.2f + 0.75f * clamp01(cutoff), part, 27);
    /* Make the standalone note's amp path explicit instead of depending on
       defaults normally delivered by a saved H3 patch. */
    set_param(0.f, part, 38);
    set_param(0.18f, part, 39);
    set_param(1.f, part, 40);
    set_param(0.18f, part, 41);
    set_param(0.75f, part, 83);
    float *parameter_values = *(float **)((char *)part + 0x140);
    trace("part-param-mode", part, *(uintptr_t *)((char *)part + 0x128));
    trace("part-param-storage", part, (uintptr_t)parameter_values);
    if (parameter_values) {
        trace("part-param-amp-attack-bits", part,
              *(uint32_t *)(parameter_values + 38));
        trace("part-param-amp-sustain-bits", part,
              *(uint32_t *)(parameter_values + 40));
        trace("part-param-master-bits", part,
              *(uint32_t *)(parameter_values + 83));
    }
    trace("part-render-gain-bits", part,
          *(uint32_t *)((char *)part + 0x5f8));
    trace("part-render-volume-bits", part,
          *(uint32_t *)((char *)part + 0x554));
    /* The normal audio host sends the channel expression update after setup.
       Setting the unity value here is a probe for the standalone host gap. */
    *(float *)((char *)part + 0x554) = 1.f;
    uint64_t note_on_event[3] = {0};
    ((uint8_t *)note_on_event)[0] = 0x10;
    ((uint8_t *)note_on_event)[1] = 36;
    ((uint8_t *)note_on_event)[2] = 0x7f;
    ((uint8_t *)note_on_event)[3] = 0x7f;
    *(void **)((char *)part + 0x38) = &event_host;
    send_event(part, note_on_event);
    void *queue = *(void **)((char *)part + 0x358);
    if (!*(uintptr_t *)((char *)queue + 0xe28) ||
        !*(uintptr_t *)((char *)queue + 0xe30))
        goto fail_graph;
    ((event_pump_fn)(base + H3_EVENT_PUMP))(queue);
    void *voice_manager = *(void **)((char *)part + 0xc0);
    trace("manager-active-after-note", voice_manager,
          *(uintptr_t *)((char *)voice_manager + 0x68));
    void *voice_group = *(void **)((char *)voice_manager + 8);
    trace("voice-group", voice_group, 0);
    trace("voice-group-vtable", voice_group,
          (uintptr_t)*(void **)voice_group);
    void **voice_begin = *(void ***)((char *)voice_group + 0x38);
    void **voice_end = *(void ***)((char *)voice_group + 0x40);
    trace("voice-count", voice_group,
          voice_begin && voice_end ? (uintptr_t)(voice_end - voice_begin) : 0);
    if (voice_begin && voice_begin != voice_end) {
        void *voice = voice_begin[0];
        trace("voice-flags-before-render", voice,
              ((uintptr_t)*((uint8_t *)voice + 8)) |
              ((uintptr_t)*((uint8_t *)voice + 9) << 8) |
              ((uintptr_t)*((uint8_t *)voice + 10) << 16) |
              ((uintptr_t)*((uint8_t *)voice + 11) << 24));
        trace("voice-pitch-before-render", voice,
              *(uintptr_t *)((char *)voice + 0x1d0));
        trace("voice-dsp-init-count-before-render", voice,
              *(uint32_t *)((char *)voice + 0x3dc));
    }

    float *input_left = calloc(BLOCK_FRAMES, sizeof(float));
    float *input_right = calloc(BLOCK_FRAMES, sizeof(float));
    float *left = calloc(BLOCK_FRAMES, sizeof(float));
    float *right = calloc(BLOCK_FRAMES, sizeof(float));
    if (!input_left || !input_right || !left || !right) {
        free(input_left); free(input_right); free(left); free(right);
        goto fail_graph;
    }
    const float *inputs[2] = {input_left, input_right};
    float *outputs[2] = {left, right};
    uint64_t flags = 0;
    for (size_t pos = 0; pos < NOTE_FRAMES;) {
        if (pos == NOTE_ON_FRAMES) {
            uint64_t note_off_event[3] = {0};
            ((uint8_t *)note_off_event)[0] = 0x20;
            ((uint8_t *)note_off_event)[1] = 36;
            send_event(part, note_off_event);
            ((event_pump_fn)(base + H3_EVENT_PUMP))(queue);
        }
        size_t count = NOTE_FRAMES - pos;
        if (count > BLOCK_FRAMES) count = BLOCK_FRAMES;
        if (pos < NOTE_ON_FRAMES && pos + count > NOTE_ON_FRAMES)
            count = NOTE_ON_FRAMES - pos;
        process_part(part, (const float *const *)outputs, outputs,
                     (long)count, &flags, 0);
        if (pos == 0) {
            float block_peak = 0.f;
            for (size_t i = 0; i < count; ++i) {
                if (fabsf(left[i]) > block_peak) block_peak = fabsf(left[i]);
                if (fabsf(right[i]) > block_peak) block_peak = fabsf(right[i]);
            }
            union { float f; uintptr_t u; } value = {0};
            value.f = block_peak;
            trace("first-block-peak-bits", part, value.u);
            trace("group-render-state-after-first-block", voice_group,
                  ((uintptr_t)*((uint8_t *)voice_group + 8)) |
                  ((uintptr_t)*((uint8_t *)voice_group + 9) << 8) |
                  ((uintptr_t)*((uint8_t *)voice_group + 10) << 16) |
                  ((uintptr_t)*((uint8_t *)voice_group + 11) << 24));
            if (voice_begin && voice_begin != voice_end) {
                void *voice = voice_begin[0];
                trace("voice-flags-after-first-block", voice,
                      ((uintptr_t)*((uint8_t *)voice + 8)) |
                      ((uintptr_t)*((uint8_t *)voice + 9) << 8) |
                      ((uintptr_t)*((uint8_t *)voice + 10) << 16) |
                      ((uintptr_t)*((uint8_t *)voice + 11) << 24));
                trace("voice-pitch-after-first-block", voice,
                      *(uintptr_t *)((char *)voice + 0x1d0));
                trace("voice-envelope-after-first-block", voice,
                      *(uintptr_t *)((char *)voice + 0x3a0));
                trace("voice-dsp-init-count-after-first-block", voice,
                      *(uint32_t *)((char *)voice + 0x3dc));
            }
        }
        for (size_t i = 0; i < count; ++i) {
            audio[(pos + i) * 2] = left[i];
            audio[(pos + i) * 2 + 1] = right[i];
            left[i] = right[i] = 0.f;
        }
        pos += count;
    }
    free(input_left); free(input_right); free(left); free(right);
    delete_graph(graph);
    free(context);

    float peak = 0.f;
    for (size_t i = 0; i < NOTE_FRAMES * 2; ++i) {
        if (!isfinite(audio[i])) { free(audio); return -1; }
        if (fabsf(audio[i]) > peak) peak = fabsf(audio[i]);
    }
    union { float f; uintptr_t u; } peak_bits = {0};
    peak_bits.f = peak;
    trace("render-peak-bits", audio, peak_bits.u);
    if (peak < 1e-7f) { free(audio); return -1; }
    if (peak > 0.98f) {
        float scale = 0.98f / peak;
        for (size_t i = 0; i < NOTE_FRAMES * 2; ++i) audio[i] *= scale;
    }
    *output = audio;
    *frames = NOTE_FRAMES;
    return 0;

fail_graph:
    delete_graph(graph);
    free(context);
    free(audio);
    return -1;
fail_alloc:
    free(audio); free(context); free(graph);
    return -1;
}

#endif /* MINIMOD_FMBASS_PROBE */
