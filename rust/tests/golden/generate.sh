#!/bin/sh
# Regenerate the golden fixtures from the original C and C++ implementation.
#
# The reference sources are the reference the Rust port is checked against, so
# this script compiles them exactly as they are shipped.  Run it from anywhere;
# it writes the .bin fixtures next to itself.
set -eu
cd "$(dirname "$0")"

src=../../../mod/src
daisy=../../../mod/third_party/daisysp/Source
cc=${CC:-cc}
cxx=${CXX:-c++}

# The DSP behind HPSS, Spectral Chop and the live looper.
$cc -O2 -std=c11 -D_GNU_SOURCE -I"$src" \
    generate.c "$src/hpss.c" "$src/spectral.c" \
    -lm -o generate
./generate
rm -f generate

# The bass synth, with its noise source replaced by a deterministic generator.
# `-ffp-contract=off` keeps the reference from fusing multiply-adds: the Rust
# port evaluates every expression separately (Rust never contracts), so a fused
# reference would differ by one ulp on almost every sample for no musical reason.
$cxx -O2 -std=c++17 -ffp-contract=off -Drand=golden_rand -I"$src" -I"$daisy" -I"$daisy/Utility" \
    generate_bass.cpp "$src/bass_sample.cpp" \
    "$daisy/PhysicalModeling/stringvoice.cpp" \
    "$daisy/PhysicalModeling/karplusstring.cpp" \
    "$daisy/Filters/svf.cpp" \
    "$daisy/Utility/dcblock.cpp" \
    "$daisy/Dynamics/crossfade.cpp" \
    -lm -o generate_bass
./generate_bass
rm -f generate_bass

ls -l ./*.bin
