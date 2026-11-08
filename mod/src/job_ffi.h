#ifndef MINIMOD_JOB_FFI_H
#define MINIMOD_JOB_FFI_H

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

/* One render job, defined in Rust (rust/crates/minimod/src/job.rs). */
struct minimod_job {
    uintptr_t screen;
    uintptr_t state;
    uintptr_t sample;
    uint32_t frames;
    uint32_t ch;
    uint32_t rate;
    float *input;
    float *harmonic;
    float *percussive;
    float *rendered;
    size_t rendered_frames;
    uint32_t rendered_ch;
    int32_t action;
    int32_t rc;
    char source_path[512];
    off_t source_data_offset;
    unsigned char *source_audio_backup;
    size_t source_audio_bytes;
};

int32_t minimod_job_run(struct minimod_job *job, const float *knobs,
                        uintptr_t app_base);
void minimod_job_free(struct minimod_job *job);

/* Finishing a job, from Rust (rust/crates/minimod/src/jobflow.rs). */
struct minimod_job_hooks {
    int (*extract)(uintptr_t screen, uintptr_t state, const float *buffer,
                   uint32_t start, uint32_t end, uint32_t source_frames,
                   uint32_t buffer_frames, uint32_t channels, int32_t name,
                   int32_t *pad);
    void (*restore_chop)(void);
    void (*toast)(const char *text);
};

struct minimod_start_hooks {
    void (*finished)(void);
    void (*repaint)(void);
    void (*toast)(const char *text);
};

struct minimod_job *minimod_job_start(int32_t action, uintptr_t app_base,
                                      const float *knobs,
                                      const struct minimod_start_hooks *hooks);
int32_t minimod_job_busy(void);
int32_t minimod_job_finish(const struct minimod_job_hooks *hooks,
                           uintptr_t screen, uintptr_t app_base);
void minimod_job_set_screen(uintptr_t screen);

int32_t minimod_job_finalize(struct minimod_job *job,
                             const struct minimod_job_hooks *hooks,
                             uintptr_t screen, uintptr_t app_base);

#endif
