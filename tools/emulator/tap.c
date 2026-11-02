/* tap.c -- record what the stock app plays, inside the guest, sample-accurately.
 *
 * The lab's own tap (the mpclab.pcm host port) drops chunks when nothing reads it
 * and is therefore not sample-accurate; this reads the aloop capture side the app
 * writes into, using raw ioctls exactly as the community apps do (no libasound).
 *
 *   tap <capture> <seconds> <out.wav> [channels] [feed-playback]
 *
 * e.g. tap /dev/snd/pcmC0D1c 12 /data/tap.wav 4 /dev/snd/pcmC0D1p
 *
 * The app restarts both of its streams when its input side has no partner, so the
 * feeder end must keep running while we record; that is what the fifth argument
 * is for. Without it the app's output goes silent after a few seconds.
 */
#include <errno.h>
#include <fcntl.h>
#include <linux/types.h>
#include <sound/asound.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

static void hw_mask(struct snd_pcm_hw_params *p, int n, unsigned bit) {
    struct snd_mask *m = &p->masks[n - SNDRV_PCM_HW_PARAM_FIRST_MASK];
    memset(m->bits, 0, sizeof(m->bits));
    m->bits[bit >> 5] = 1u << (bit & 31);
}

static void hw_int(struct snd_pcm_hw_params *p, int n, unsigned v) {
    struct snd_interval *i = &p->intervals[n - SNDRV_PCM_HW_PARAM_FIRST_INTERVAL];
    i->min = i->max = v;
    i->integer = 1;
}

static int pcm_params(int fd, unsigned period, unsigned periods, int rate, unsigned ch) {
    struct snd_pcm_hw_params hw;
    memset(&hw, 0, sizeof hw);
    for (int n = SNDRV_PCM_HW_PARAM_FIRST_MASK; n <= SNDRV_PCM_HW_PARAM_LAST_MASK; n++)
        memset(hw.masks[n - SNDRV_PCM_HW_PARAM_FIRST_MASK].bits, 0xff, sizeof(hw.masks[0].bits));
    for (int n = SNDRV_PCM_HW_PARAM_FIRST_INTERVAL; n <= SNDRV_PCM_HW_PARAM_LAST_INTERVAL; n++)
        hw.intervals[n - SNDRV_PCM_HW_PARAM_FIRST_INTERVAL].max = ~0u;
    hw.rmask = ~0u;
    hw.info = ~0u;
    hw_mask(&hw, SNDRV_PCM_HW_PARAM_ACCESS, SNDRV_PCM_ACCESS_RW_INTERLEAVED);
    hw_mask(&hw, SNDRV_PCM_HW_PARAM_FORMAT, SNDRV_PCM_FORMAT_S32_LE);
    hw_mask(&hw, SNDRV_PCM_HW_PARAM_SUBFORMAT, SNDRV_PCM_SUBFORMAT_STD);
    hw_int(&hw, SNDRV_PCM_HW_PARAM_CHANNELS, ch);
    hw_int(&hw, SNDRV_PCM_HW_PARAM_RATE, (unsigned)rate);
    hw_int(&hw, SNDRV_PCM_HW_PARAM_PERIOD_SIZE, period);
    hw_int(&hw, SNDRV_PCM_HW_PARAM_BUFFER_SIZE, period * periods);
    if (ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &hw) < 0) return -1;
    if (ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) < 0) return -1;
    return 0;
}

static void put32(unsigned char *p, unsigned v) {
    p[0] = v & 0xff; p[1] = (v >> 8) & 0xff; p[2] = (v >> 16) & 0xff; p[3] = (v >> 24) & 0xff;
}

int main(int argc, char **argv) {
    const char *dev = argc > 1 ? argv[1] : "/dev/snd/pcmC0D1c";
    double seconds = argc > 2 ? atof(argv[2]) : 10.0;
    const char *out = argc > 3 ? argv[3] : "/data/tap.wav";
    unsigned ch = argc > 4 ? (unsigned)atoi(argv[4]) : 4;
    const char *feed_dev = argc > 5 ? argv[5] : NULL;

    int fd = open(dev, O_RDONLY);
    if (fd < 0) { fprintf(stderr, "open %s: %s\n", dev, strerror(errno)); return 1; }
    int ok = -1;
    for (unsigned try = ch; try >= 2 && ok < 0; try -= 2) {
        if (pcm_params(fd, 128, 8, 44100, try) == 0) { ch = try; ok = 0; }
    }
    if (ok < 0) { fprintf(stderr, "hw_params failed on %s\n", dev); return 1; }
    fprintf(stderr, "tap: %s %uch S32_LE 44100\n", dev, ch);

    int feed = -1;
    if (feed_dev) {
        feed = open(feed_dev, O_WRONLY);
        if (feed < 0 || pcm_params(feed, 128, 8, 44100, ch) != 0) {
            fprintf(stderr, "tap: feeder %s unavailable (%s); the app may stall\n",
                    feed_dev, strerror(errno));
            if (feed >= 0) { close(feed); feed = -1; }
        } else {
            static char silence[128 * 8 * 4];
            for (int i = 0; i < 4; i++) {           /* prefill so a late read never starves it */
                struct snd_xferi w = { .buf = silence, .frames = 128 };
                ioctl(feed, SNDRV_PCM_IOCTL_WRITEI_FRAMES, &w);
            }
            fprintf(stderr, "tap: feeding silence to %s\n", feed_dev);
        }
    }

    FILE *f = fopen(out, "wb");
    if (!f) { fprintf(stderr, "open %s: %s\n", out, strerror(errno)); return 1; }
    unsigned char hdr[44] = {0};
    memcpy(hdr, "RIFF", 4); memcpy(hdr + 8, "WAVEfmt ", 8);
    hdr[16] = 16; hdr[20] = 1; hdr[22] = 2;
    put32(hdr + 24, 44100); put32(hdr + 28, 44100u * 4u);
    hdr[32] = 4; hdr[34] = 16; memcpy(hdr + 36, "data", 4);
    fwrite(hdr, 1, sizeof hdr, f);

    static int32_t raw[128 * 8];
    static int16_t s16[128 * 2];
    long frames = 0;
    long target = (long)(seconds * 44100.0);
    while (frames < target) {
        struct snd_xferi x = { .buf = raw, .frames = 128 };
        if (ioctl(fd, SNDRV_PCM_IOCTL_READI_FRAMES, &x) < 0) {
            if (errno == EPIPE) { ioctl(fd, SNDRV_PCM_IOCTL_PREPARE); continue; }
            fprintf(stderr, "read: %s\n", strerror(errno));
            break;
        }
        for (unsigned i = 0; i < x.result; i++) {
            s16[i * 2] = (int16_t)(raw[i * ch] >> 16);
            s16[i * 2 + 1] = (int16_t)(raw[i * ch + 1] >> 16);
        }
        fwrite(s16, sizeof(int16_t), (size_t)x.result * 2, f);
        frames += (long)x.result;
        if (feed >= 0) {
            static char silence[128 * 8 * 4];
            struct snd_xferi w = { .buf = silence, .frames = x.result };
            if (ioctl(feed, SNDRV_PCM_IOCTL_WRITEI_FRAMES, &w) < 0 && errno == EPIPE)
                ioctl(feed, SNDRV_PCM_IOCTL_PREPARE);
        }
    }
    long size = ftell(f);
    unsigned char b[4];
    put32(b, (unsigned)(size - 8)); fseek(f, 4, SEEK_SET); fwrite(b, 1, 4, f);
    put32(b, (unsigned)(size - 44)); fseek(f, 40, SEEK_SET); fwrite(b, 1, 4, f);
    fclose(f);
    close(fd);
    fprintf(stderr, "tap: wrote %ld frames to %s\n", frames, out);
    return 0;
}
