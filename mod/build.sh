#!/bin/sh
# Build mpc-mini: the LD_PRELOAD library and the SD autorun package.
set -eu
cd "$(dirname "$0")"
mkdir -p build

zig cc -target aarch64-linux-gnu -shared -fPIC -O2 -fno-omit-frame-pointer -mbranch-protection=standard -Wall -Wextra -s \
    -o build/mpcmini.so src/mpcmini.c
echo "built build/mpcmini.so ($(wc -c < build/mpcmini.so) bytes)"
