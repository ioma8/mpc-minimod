# Device-side probes for the MPC Sample.  Host test on macOS, cross build for
# the device with the same zig the other MPC projects use.
#
#   make test     host build + run the recording-logic test
#   make device   build probe-tanf.so for aarch64 (glibc, needs GLIBC_2.17)
#   make clean

ZIG     ?= zig
CC      ?= cc
TARGET  ?= aarch64-linux-gnu

device: probe-tanf.so

probe-tanf.so: probe-tanf.c
	$(ZIG) cc -target $(TARGET) -shared -fPIC -O2 -Wall -Wextra \
		-o $@ $< -lm

test: probe_test
	./probe_test

probe_test: probe_test.c probe-tanf.c
	$(CC) -Wall -Wextra -o $@ probe_test.c -lm
	rm -f probe_test.dSYM

clean:
	rm -rf probe_test probe_test.dSYM probe-tanf.so

.PHONY: device test clean
