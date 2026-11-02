/* Virtual MPC "Control Surface".
 *
 * Provides the ALSA sequencer port the app looks for, bridges it to Midi
 * Through (client 14) where the app's MIDI I/O lives, and logs every event the
 * app sends so the protocol can be observed. */
#include <stdio.h>
#include <unistd.h>
#include <string.h>

typedef struct _snd_seq snd_seq_t;

typedef struct { unsigned char client, port; } seq_addr;
typedef struct {
    unsigned char type, flags, tag, queue;
    unsigned int time[2];
    seq_addr source, dest;
    unsigned char data[12];
} seq_event;

extern int snd_seq_open(snd_seq_t **seqp, const char *name, int streams, int mode);
extern int snd_seq_set_client_name(snd_seq_t *seq, const char *name);
extern int snd_seq_create_simple_port(snd_seq_t *seq, const char *name,
                                      unsigned int caps, unsigned int type);
extern int snd_seq_connect_to(snd_seq_t *seq, int myport, int dc, int dp);
extern int snd_seq_connect_from(snd_seq_t *seq, int myport, int sc, int sp);
extern int snd_seq_event_input(snd_seq_t *seq, seq_event **ev);
extern int snd_seq_event_input_pending(snd_seq_t *seq, int fetch_sequencer);

#define CAP_READ 0x01
#define CAP_WRITE 0x02
#define CAP_SUBS_READ 0x10
#define CAP_SUBS_WRITE 0x20
#define TYPE_MIDI_GENERIC 0x02

int main(void)
{
    snd_seq_t *seq = NULL;
    if (snd_seq_open(&seq, "default", 3, 0) < 0) { printf("agent: open failed\n"); return 1; }
    snd_seq_set_client_name(seq, "Control Surface");
    int p = snd_seq_create_simple_port(seq, "Control Surface",
             CAP_READ | CAP_WRITE | CAP_SUBS_READ | CAP_SUBS_WRITE, TYPE_MIDI_GENERIC);
    if (p < 0) { printf("agent: port failed %d\n", p); return 1; }
    snd_seq_connect_to(seq, p, 14, 0);
    snd_seq_connect_from(seq, p, 14, 0);
    printf("agent: ready port=%d\n", p);
    fflush(stdout);

    seq_event *ev = NULL;
    for (;;) {
        if (snd_seq_event_input_pending(seq, 1) <= 0) { usleep(20000); continue; }
        while (snd_seq_event_input(seq, &ev) >= 0 && ev) {
            unsigned int len = 0; unsigned char *ptr = NULL;
            memcpy(&len, ev->data, 4);
            memcpy(&ptr, ev->data + 4, 8);
            printf("EV type=0x%02x src=%d:%d len=%u", ev->type,
                   ev->source.client, ev->source.port, len);
            if (ptr && len > 0 && len < 512) {
                printf(" data=");
                for (unsigned i = 0; i < len; i++) printf("%02x", ptr[i]);
            }
            printf("\n");
            fflush(stdout);
        }
    }
    return 0;
}
