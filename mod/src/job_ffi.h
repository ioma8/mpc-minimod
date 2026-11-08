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

#endif
