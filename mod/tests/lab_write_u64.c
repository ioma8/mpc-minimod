#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    int fd = open(argv[1], O_WRONLY);
    if (fd < 0) return 3;
    uint64_t address = strtoull(argv[2], 0, 0), value = strtoull(argv[3], 0, 0);
    return pwrite(fd, &value, sizeof(value), (off_t)address) == sizeof(value) ? 0 : 4;
}
