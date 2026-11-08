#ifndef MINIMOD_LOOPER_H
#define MINIMOD_LOOPER_H

#include <stddef.h>
#include <stdint.h>

/* Live looper: 16 independent pad loops recorded from the firmware's audio
 * input stream and mixed into its audio output.
 *
 * Threads
 *   input hook   looper_input()   audio input thread; copies mono frames and
 *                                 the input level, never allocates or locks
 *   output hook  looper_render()  audio output thread; mixes loops, records,
 *                                 all buffers are preallocated on open
 *   pad/buttons  looper_pad()     control-surface thread; state changes only
 *   paint        looper_snapshot() any thread; pure reads of the volatile state
 *
 * Timing is counted in output frames (the output hook is the clock), so a loop
 * length is always an exact number of played frames.
 */

#define LOOPER_PADS 16

enum {
    LOOP_IDLE = 0,   /* no loop */
    LOOP_REC = 1,    /* recording from the input source */
    LOOP_PLAY = 2,   /* looping */
    LOOP_DUB = 3,    /* looping and adding input */
    LOOP_STOP = 4    /* loop kept, not playing */
};

struct looper_pad_view {
    unsigned char state;
    unsigned len_frames;   /* closed loop length (0 while recording) */
    unsigned rec_frames;   /* frames captured so far */
    unsigned pos;          /* playhead in frames, 0 when not playing */
};

struct looper_view {
    int open;
    int sync;                  /* sync pulse running */
    int click;                 /* metronome click enabled */
    unsigned sync_frames;      /* pulse period */
    unsigned pulse_phase;      /* 0..sync_frames, or 0xffffffff when idle */
    float input;               /* input level 0..1 */
    int recording_pad;         /* -1 when nothing is recording */
    unsigned lat_frames;       /* latency applied to the last take */
    char lat_src;              /* E estimate, C chirp, A correlation, S seeded */
    char message[40];          /* status line, empty when none */
    struct looper_pad_view pads[LOOPER_PADS];
};

/* Open/close the fullscreen view. Opening preallocates the loop pool with
 * mmap; closing stops everything but keeps the recorded loops in memory. */
int looper_activate(void);
void looper_close(void);
int looper_is_open(void);

/* Pad event. Returns 1 when the looper owns the press (always, when open).
 * `shift` selects overdub, `erase` clears the pad. */
int looper_pad(int pad, int shift, int erase);

/* Button event while open. Returns 1 when consumed. Handles LOOP (sync),
 * SHIFT+LOOP (click), ERASE+LOOP (clear all), STOP (all loops) and
 * SHIFT+DATA (close). */
int looper_button(unsigned code, int down, int shift, int erase);

/* Stop playback of every loop, keeping the material. */
void looper_stop_all(void);

/* Record latency compensation, in frames: the measured output + input delay
 * plus whatever waits in the input ring. Snapshotted when a take starts. */
void looper_set_latency(unsigned frames);
void looper_set_latency_ex(unsigned frames, char source);
unsigned looper_input_backlog(void);
int looper_click_enabled(void);
/* Show a short status line in the view. */
void looper_notice(const char *text);


/* Audio hooks. `channels` is planar float as the firmware passes it. */
void looper_input(const float *const *channels, int count, unsigned frames);
/* Same, from the firmware's S32 planar input buffers (snd_pcm_readn). */
void looper_input_s32(const int32_t *const *channels, int count, unsigned frames);
/* Same, from interleaved S32 frames (snd_pcm_readi). */
void looper_input_s32_interleaved(const int32_t *buf, unsigned channels,
                                  unsigned frames);
/* Mix the loops into the interleaved S32 block about to be written. */
void looper_render_s32(int32_t *buf, unsigned channels, unsigned frames);

/* Snapshot for the paint hook; returns 1 when something is shown. */
int looper_snapshot(struct looper_view *view);

#endif
