#ifndef MINIMOD_SPECTRAL_H
#define MINIMOD_SPECTRAL_H
#include <stddef.h>

/* Detect up to max_positions onset positions (sample frames) in an
 * interleaved float32 buffer.  Positions are sorted and include 0 when the
 * first hit arrives later than 50 ms.  detected_out receives the number of
 * peaks found before the 16-position limit.  Returns the number written. */
int spectral_onsets(const float *input, size_t frames, unsigned channels,
                    unsigned rate, unsigned *positions, unsigned max_positions,
                    unsigned *detected_out);
#endif
