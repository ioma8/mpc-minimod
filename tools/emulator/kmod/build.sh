#!/bin/sh
# Build the virtual AC50 control-surface card (rawmidi + duplex PCM).
# Needs the two Debian header packages unpacked; set KH to their root.
set -e
KH=${KH:?set KH to the directory containing usr/src/linux-headers-*}
A=$KH/usr/src/linux-headers-6.12.107+deb13-arm64
C=$KH/usr/src/linux-headers-6.12.107+deb13-common
INC="-I$C/arch/arm64/include -I$A/arch/arm64/include/generated -I$C/include \
     -I$C/arch/arm64/include/uapi -I$A/arch/arm64/include/generated/uapi \
     -I$C/include/uapi -I$A/include/generated/uapi -I$A/include"
CFLAGS="-D__KERNEL__ -DMODULE -DKBUILD_MODNAME=\"snd_mcuvirt\" $INC \
  -include $C/include/linux/compiler_types.h -include $C/include/linux/kconfig.h \
  -std=gnu11 -O2 -fno-pic -fno-stack-protector -fno-common -fshort-wchar \
  -fno-asynchronous-unwind-tables -fno-unwind-tables -mno-outline-atomics \
  -Wno-unused-parameter -Wno-pointer-sign"
# 1st pass resolves imports, genver.py turns them into __versions entries
zig cc -target aarch64-freestanding -c -o snd-mcuvirt.o snd-mcuvirt.c $CFLAGS
python3 genver.py snd-mcuvirt.o
zig cc -target aarch64-freestanding -c -o snd-mcuvirt.o snd-mcuvirt.c $CFLAGS
python3 genver.py snd-mcuvirt.o
# the module.lds link is what produces the .plt arm64 requires
zig cc -target aarch64-freestanding -nostdlib -r -T "$A/arch/arm64/module.lds" \
  -o snd-mcuvirt.ko snd-mcuvirt.o
echo "built snd-mcuvirt.ko"
