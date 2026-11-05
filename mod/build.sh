#!/bin/sh
# Build the preload library (pack.sh makes the SD package) and the one
# symbol the emulator test reads.
#
# zig's linker driver rejects -Map (all four spellings, verified), so the
# symbol offset comes from an unstripped link; the shipped object is the same
# link with -s, whose layout is identical.
set -eu
cd "$(dirname "$0")"
mkdir -p build

flags="-target aarch64-linux-gnu -shared -fPIC -O2 -fno-omit-frame-pointer -mbranch-protection=standard -Wall -Wextra"

zig cc $flags -o build/minimod.elf src/minimod.c src/hpss.c src/spectral.c src/reverb.c
nm build/minimod.elf | awk '$3=="g_screen"{print $1" g_screen"}' > build/minimod.map
grep -q ' g_screen$' build/minimod.map || { echo "g_screen missing from the build" >&2; exit 1; }

zig cc $flags -s -o build/minimod.so src/minimod.c src/hpss.c src/spectral.c src/reverb.c
echo "built build/minimod.so ($(wc -c < build/minimod.so) bytes), g_screen $(cut -d' ' -f1 build/minimod.map)"
