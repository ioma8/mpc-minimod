/* snd-mcuvirt: a virtual rawmidi device standing in for the AC50 control
 * surface MCU (real hardware exposes it via the serial-midi UART driver).
 * The app only needs the character device to exist so it can open it. */
#include <linux/module.h>
#include <linux/platform_device.h>
#include <sound/core.h>
#include <sound/rawmidi.h>
#include <sound/pcm.h>
#include <sound/pcm_params.h>

static struct platform_device *pdev;
static struct snd_card *card;
static struct snd_rawmidi *rmidi;
static struct snd_pcm *pcm;

/* Minimal duplex PCM so hw:0 exists on this card, matching the real board
 * where the codec and the MCU live on the same card index. */
static const struct snd_pcm_hardware mcuhw = {
    .info = SNDRV_PCM_INFO_INTERLEAVED | SNDRV_PCM_INFO_MMAP | SNDRV_PCM_INFO_MMAP_VALID,
    .formats = SNDRV_PCM_FMTBIT_S16_LE,
    .rates = SNDRV_PCM_RATE_44100 | SNDRV_PCM_RATE_48000,
    .rate_min = 44100, .rate_max = 48000,
    .channels_min = 1, .channels_max = 8,
    .buffer_bytes_max = 1 << 20,
    .period_bytes_min = 64, .period_bytes_max = 1 << 18,
    .periods_min = 2, .periods_max = 1024,
};
static int pcm_open(struct snd_pcm_substream *s)
{ s->runtime->hw = mcuhw; return 0; }
static int pcm_close(struct snd_pcm_substream *s) { return 0; }
static int pcm_hw_params(struct snd_pcm_substream *s, struct snd_pcm_hw_params *p)
{ return snd_pcm_lib_malloc_pages(s, params_buffer_bytes(p)); }
static int pcm_hw_free(struct snd_pcm_substream *s) { return snd_pcm_lib_free_pages(s); }
static int pcm_prepare(struct snd_pcm_substream *s) { return 0; }
static int pcm_trigger(struct snd_pcm_substream *s, int cmd) { return 0; }
static snd_pcm_uframes_t pcm_pointer(struct snd_pcm_substream *s)
{
    return (snd_pcm_uframes_t)(s->runtime->control->appl_ptr % s->runtime->buffer_size);
}
static const struct snd_pcm_ops pcm_ops = {
    .open = pcm_open, .close = pcm_close,
    .ioctl = snd_pcm_lib_ioctl,
    .hw_params = pcm_hw_params, .hw_free = pcm_hw_free,
    .prepare = pcm_prepare, .trigger = pcm_trigger, .pointer = pcm_pointer,
};
static const struct snd_pcm_ops pcm_capture_ops = {
    .open = pcm_open, .close = pcm_close,
    .ioctl = snd_pcm_lib_ioctl,
    .hw_params = pcm_hw_params, .hw_free = pcm_hw_free,
    .prepare = pcm_prepare, .trigger = pcm_trigger, .pointer = pcm_pointer,
};

static int mcu_input_open(struct snd_rawmidi_substream *s) { return 0; }
static int mcu_input_close(struct snd_rawmidi_substream *s) { return 0; }
static void mcu_input_trigger(struct snd_rawmidi_substream *s, int up)
{
    if (up) snd_rawmidi_receive(s, "\0", 0);
}
static int mcu_output_open(struct snd_rawmidi_substream *s) { return 0; }
static int mcu_output_close(struct snd_rawmidi_substream *s) { return 0; }
static void mcu_output_trigger(struct snd_rawmidi_substream *s, int up)
{
    unsigned char buf[64];
    int n;
    if (!up) return;
    while ((n = snd_rawmidi_transmit_peek(s, buf, sizeof buf)) > 0)
        snd_rawmidi_transmit_ack(s, n);
}

static const struct snd_rawmidi_ops in_ops = {
    .open = mcu_input_open, .close = mcu_input_close, .trigger = mcu_input_trigger,
};
static const struct snd_rawmidi_ops out_ops = {
    .open = mcu_output_open, .close = mcu_output_close, .trigger = mcu_output_trigger,
};

static int __init mcuvirt_init(void)
{
    int err;
    pdev = platform_device_register_simple("snd-mcuvirt", -1, NULL, 0);
    if (IS_ERR(pdev)) return PTR_ERR(pdev);

    err = snd_card_new(&pdev->dev, -1, "MCU", THIS_MODULE, 0, &card);
    if (err < 0) goto err_pdev;

    strcpy(card->driver, "MCU");
    strcpy(card->shortname, "MPC Control Surface");
    strcpy(card->longname, "MPC Control Surface");

    err = snd_rawmidi_new(card, "Control Surface", 0, 1, 1, &rmidi);
    if (err < 0) goto err_card;
    strcpy(rmidi->name, "Control Surface");
    rmidi->info_flags = SNDRV_RAWMIDI_INFO_OUTPUT |
                        SNDRV_RAWMIDI_INFO_INPUT |
                        SNDRV_RAWMIDI_INFO_DUPLEX;
    snd_rawmidi_set_ops(rmidi, SNDRV_RAWMIDI_STREAM_INPUT, &in_ops);
    snd_rawmidi_set_ops(rmidi, SNDRV_RAWMIDI_STREAM_OUTPUT, &out_ops);

    err = snd_pcm_new(card, "MCU PCM", 0, 1, 1, &pcm);
    if (err < 0) goto err_card;
    pcm->private_data = card;
    strcpy(pcm->name, "MPC Sample PCM");
    snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_PLAYBACK, &pcm_ops);
    snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_CAPTURE, &pcm_capture_ops);
    snd_pcm_lib_preallocate_pages_for_all(pcm, SNDRV_DMA_TYPE_CONTINUOUS,
                                          NULL,
                                          256 * 1024, 256 * 1024);

    err = snd_card_register(card);
    if (err < 0) goto err_card;
    pr_info("snd-mcuvirt: Control Surface rawmidi registered\n");
    return 0;

err_card:
    snd_card_free(card);
err_pdev:
    platform_device_unregister(pdev);
    return err;
}

static void __exit mcuvirt_exit(void)
{
    snd_card_free(card);
    platform_device_unregister(pdev);
}

module_init(mcuvirt_init);
module_exit(mcuvirt_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Virtual AC50 control surface rawmidi");
/* arm64 module loader requires a .plt section even when the large code model
 * leaves nothing to trampoline. */
__asm__(".section .plt,\"ax\",%progbits\n"
        ".space 16\n"
        ".previous\n");
#include "versions.h"
