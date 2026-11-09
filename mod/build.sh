#!/bin/sh
# Build the preload library (pack.sh makes the SD package) and the one
# symbol the emulator test reads.
set -eu
cd "$(dirname "$0")"
mkdir -p build

# The mod is a Rust cdylib: no C is compiled.  The cross linker driver (which also
# silences lld's deprecated -O1 warning) lives in rust/scripts; it has to be on
# PATH or rust/check.sh fails.
if ! command -v zig-cc-aarch64-linux-gnu >/dev/null 2>&1; then
    echo "zig-cc-aarch64-linux-gnu is not on PATH; install rust/scripts/zig-cc-aarch64-linux-gnu" >&2
    exit 1
fi
(cd ../rust && cargo build --release --target aarch64-unknown-linux-gnu -p minimod --lib)
rustlib=../rust/target/aarch64-unknown-linux-gnu/release/libminimod.so
test -f "$rustlib" || { echo "Rust shared library missing" >&2; exit 1; }
cp "$rustlib" build/minimod.elf
nm build/minimod.elf | awk '$3=="g_screen"{print $1" g_screen"}' > build/minimod.map
grep -q ' g_screen$' build/minimod.map || { echo "g_screen missing from the build" >&2; exit 1; }
# The emulator test reads the looper's pad array and its stride by symbol.
for symbol in g_pads g_pad_stride g_open g_click g_buf_out; do
    nm build/minimod.elf | grep -q " $symbol\$" || {
        echo "looper symbol $symbol missing from the build" >&2; exit 1; }
done
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
# firmware's loader would fail the whole process (exit 127).  Everything it may
# legitimately import is versioned (glibc); anything else is ours.
unresolved=$(nm -D build/minimod.elf | grep ' U ' | grep -v '@' || true)
if [ -n "$unresolved" ]; then
    echo "unresolved symbols in the build:" >&2
    echo "$unresolved" >&2
    exit 1
fi
"$objcopy" --strip-all build/minimod.elf build/minimod.so

# The MPC Sample ships glibc 2.39 (see the az0x open-source dump). Refuse a
# library that would need a newer symbol version than the device has.
if [ -n "${LLVM_OBJDUMP:-}" ]; then
    objdump=$LLVM_OBJDUMP
elif command -v llvm-objdump >/dev/null 2>&1; then
    objdump=$(command -v llvm-objdump)
elif [ -x /opt/homebrew/opt/llvm/bin/llvm-objdump ]; then
    objdump=/opt/homebrew/opt/llvm/bin/llvm-objdump
else
    echo "llvm-objdump is required for the glibc symbol-version check" >&2
    exit 1
fi
version=$("$objdump" -T build/minimod.so 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -u | sort -t. -k2 -n | tail -1)
if [ -n "$version" ]; then
    v=${version#GLIBC_}
    major=${v%%.*}
    minor=${v#*.}
    if [ "$major" -gt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -gt 39 ]; }; then
        echo "built against $version, newer than the MPC Sample's glibc 2.39" >&2
        exit 1
    fi
    echo "glibc symbol versions: highest $version (device has 2.39)"
else
    echo "glibc symbol versions: none (static link?)"
fi
echo "built build/minimod.so ($(wc -c < build/minimod.so) bytes), g_screen $(cut -d' ' -f1 build/minimod.map)"
