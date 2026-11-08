#!/bin/sh
# Regenerate the golden fixtures from the original C implementation.
#
# The C sources are the reference the Rust port is checked against, so this
# script deliberately compiles them exactly as they are shipped.  Run it from
# anywhere; it writes the .bin fixtures next to itself.
set -eu
cd "$(dirname "$0")"

src=../../../mod/src
cc=${CC:-cc}
$cc -O2 -std=c11 -D_GNU_SOURCE -I"$src" \
    generate.c "$src/hpss.c" "$src/spectral.c" \
    -lm -o generate

./generate
rm -f generate
ls -l ./*.bin
