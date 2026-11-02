/* hwwatch — report the instruction that writes one address in another process.
 *
 * Uses a perf hardware breakpoint (PERF_TYPE_BREAKPOINT, HW_BREAKPOINT_W) with
 * a sampling event; every write produces a sample whose IP is the faulting
 * instruction. Run as root in the guest:
 *
 *     hwwatch <pid> <address-hex> [seconds]
 *
 * Prints one line per write: ip=<pc> pid=<tid>
 */
#define _GNU_SOURCE
#include <errno.h>
#include <linux/hw_breakpoint.h>
#include <linux/perf_event.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static int perf_open(struct perf_event_attr *attr, pid_t pid)
{
    return (int)syscall(SYS_perf_event_open, attr, pid, -1, -1, 0);
}

int main(int argc, char **argv)
{
    if (argc < 3) {
        fprintf(stderr, "usage: %s <pid> <addr-hex> [seconds]\n", argv[0]);
        return 2;
    }
    pid_t pid = (pid_t)atoi(argv[1]);
    uint64_t addr = strtoull(argv[2], 0, 16);
    int seconds = argc > 3 ? atoi(argv[3]) : 10;

    struct perf_event_attr attr;
    memset(&attr, 0, sizeof attr);
    attr.type = PERF_TYPE_BREAKPOINT;
    attr.size = sizeof attr;
    attr.bp_type = HW_BREAKPOINT_W;
    attr.bp_addr = addr;
    attr.bp_len = HW_BREAKPOINT_LEN_4;
    attr.sample_period = 1;
    attr.sample_type = PERF_SAMPLE_IP | PERF_SAMPLE_TID;
    attr.disabled = 0;
    attr.exclude_kernel = 1;
    attr.exclude_hv = 1;
    attr.wakeup_events = 1;

    int fd = perf_open(&attr, pid);
    if (fd < 0) {
        fprintf(stderr, "perf_event_open: %s\n", strerror(errno));
        return 1;
    }
    size_t pg = (size_t)sysconf(_SC_PAGESIZE);
    size_t len = pg * 9;
    void *ring = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (ring == MAP_FAILED) {
        fprintf(stderr, "mmap: %s\n", strerror(errno));
        return 1;
    }
    struct perf_event_mmap_page *meta = ring;
    char *data = (char *)ring + pg;

    struct timespec deadline;
    clock_gettime(CLOCK_MONOTONIC, &deadline);
    deadline.tv_sec += seconds;

    int writes = 0;
    while (1) {
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec > deadline.tv_sec ||
            (now.tv_sec == deadline.tv_sec && now.tv_nsec >= deadline.tv_nsec))
            break;
        struct pollfd pfd = {.fd = fd, .events = POLLIN};
        if (poll(&pfd, 1, 200) <= 0)
            continue;
        uint64_t head = meta->data_head;
        __sync_synchronize();
        uint64_t tail = meta->data_tail;
        while (tail < head) {
            struct perf_event_header hdr;
            size_t off = (size_t)(tail % (len - pg));
            memcpy(&hdr, data + off, sizeof hdr);
            if (hdr.size == 0)
                break;
            if (hdr.type == PERF_RECORD_SAMPLE) {
                char *p = data + off + sizeof hdr;
                uint64_t ip = 0;
                uint32_t spid = 0, tid = 0;
                memcpy(&ip, p, 8);
                p += 8;
                memcpy(&spid, p, 4);
                p += 4;
                memcpy(&tid, p, 4);
                printf("ip=0x%llx pid=%u tid=%u\n", (unsigned long long)ip, spid, tid);
                fflush(stdout);
                writes++;
            }
            tail += hdr.size;
        }
        meta->data_tail = tail;
    }
    fprintf(stderr, "hwwatch: %d write(s) on 0x%llx\n", writes,
            (unsigned long long)addr);
    return 0;
}
