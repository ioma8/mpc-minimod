#!/bin/sh
# Build minimod: the LD_PRELOAD library and the SD autorun package.
set -eu
cd "$(dirname "$0")"
mkdir -p build

zig cc -target aarch64-linux-gnu -shared -fPIC -O2 -fno-omit-frame-pointer -mbranch-protection=standard -Wall -Wextra -s \
    -o build/minimod.so src/minimod.c
echo "built build/minimod.so ($(wc -c < build/minimod.so) bytes)"
