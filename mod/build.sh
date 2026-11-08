#!/bin/sh
# Build the preload library (pack.sh makes the SD package) and the one
# symbol the emulator test reads.
set -eu
cd "$(dirname "$0")"
mkdir -p build

common="-target aarch64-linux-gnu -fPIC -O2 -fno-omit-frame-pointer -mbranch-protection=standard -Wall -Wextra -ffunction-sections -fdata-sections"
daisy="third_party/daisysp/Source"

zig cc $common -c src/minimod.c -o build/minimod.o
zig cc $common -c src/hpss.c -o build/hpss.o
zig cc $common -c src/spectral.c -o build/spectral.o
zig cc $common -c src/looper.c -o build/looper.o
zig cc $common -c src/h3m_effect.c -o build/h3m_effect.o
zig c++ $common -fno-exceptions -fno-rtti -I"$daisy" -c src/bass_sample.cpp -o build/bass_sample.o
zig c++ $common -fno-exceptions -fno-rtti -fvisibility=hidden -I"$daisy" -I"$daisy/Utility" -c "$daisy/PhysicalModeling/stringvoice.cpp" -o build/daisy_stringvoice.o
zig c++ $common -fno-exceptions -fno-rtti -fvisibility=hidden -I"$daisy" -I"$daisy/Utility" -c "$daisy/PhysicalModeling/karplusstring.cpp" -o build/daisy_karplusstring.o
zig c++ $common -fno-exceptions -fno-rtti -fvisibility=hidden -I"$daisy" -I"$daisy/Utility" -c "$daisy/Filters/svf.cpp" -o build/daisy_svf.o
zig c++ $common -fno-exceptions -fno-rtti -fvisibility=hidden -I"$daisy" -I"$daisy/Utility" -c "$daisy/Utility/dcblock.cpp" -o build/daisy_dcblock.o
zig c++ $common -fno-exceptions -fno-rtti -fvisibility=hidden -I"$daisy" -I"$daisy/Utility" -c "$daisy/Dynamics/crossfade.cpp" -o build/daisy_crossfade.o

objects="build/minimod.o build/hpss.o build/spectral.o build/looper.o build/h3m_effect.o build/bass_sample.o build/daisy_stringvoice.o build/daisy_karplusstring.o build/daisy_svf.o build/daisy_dcblock.o build/daisy_crossfade.o"
zig c++ -target aarch64-linux-gnu -shared -fPIC -Wl,--gc-sections -o build/minimod.elf $objects -lm
nm build/minimod.elf | awk '$3=="g_screen"{print $1" g_screen"}' > build/minimod.map
grep -q ' g_screen$' build/minimod.map || { echo "g_screen missing from the build" >&2; exit 1; }
if [ -n "${LLVM_OBJCOPY:-}" ]; then
    objcopy=$LLVM_OBJCOPY
elif command -v llvm-objcopy >/dev/null 2>&1; then
    objcopy=$(command -v llvm-objcopy)
elif [ -x /opt/homebrew/opt/llvm/bin/llvm-objcopy ]; then
    objcopy=/opt/homebrew/opt/llvm/bin/llvm-objcopy
else
    echo "llvm-objcopy is required to strip the shared library without shifting g_screen" >&2
    exit 1
fi
# The preload library must not leave one of its own symbols unresolved: the
# firmware's loader would fail the whole process (exit 127).
if nm -D build/minimod.elf | grep -qE '^ +U (looper_|minimod)'; then
    echo "unresolved minimod/looper symbol in the build:" >&2
    nm -D build/minimod.elf | grep -E '^ +U (looper_|minimod)' >&2
    exit 1
fi
"$objcopy" --strip-all build/minimod.elf build/minimod.so
echo "built build/minimod.so ($(wc -c < build/minimod.so) bytes), g_screen $(cut -d' ' -f1 build/minimod.map)"
