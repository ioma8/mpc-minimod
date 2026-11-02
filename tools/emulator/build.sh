#!/bin/sh
# Build everything needed to boot the AC50 firmware under QEMU on Apple Silicon.
#
# Inputs (downloaded once into $CACHE, reused afterwards):
#   * the vendor firmware image, taken from the MPC Sample updater app
#   * a Debian arm64 kernel + modules (the device kernel has no virtio at all,
#     so it cannot boot on a QEMU machine)
#   * a static arm64 busybox and alsa-utils, for the initramfs and for testing
#
# Output in $OUT: rootfs.img, initramfs.cpio.gz, ac50.dtb, k/vmlinuz*
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
CACHE=${MPC2STEMS_CACHE:-$HOME/Library/Caches/mpc2stems/emu}
OUT=${MPC2STEMS_EMU_OUT:-$CACHE/out}
FIRMWARE=${MPC2STEMS_FIRMWARE:-"/Users/jakubkolcar/Downloads/MPC Sample 1.3.0 Updater.app/Contents/Resources/Update.img"}
CODENAME=deb13

# Update.img partition offsets, from its PART table. Each is an XZ stream.
OFF_RECOVERY=1755264
OFF_BOOT=14186264
OFF_ROOTFS=26830808

KERNEL_PKG="linux-image-6.12.107+deb13-arm64-unsigned_6.12.107-1_arm64.deb"
BUSYBOX_PKG="busybox-static_1.37.0-6+b9_arm64.deb"
ALSA_PKG="alsa-utils_1.2.8-1_arm64.deb"   # bookworm build: old enough glibc

DEB=https://deb.debian.org/debian
mkdir -p "$CACHE" "$OUT"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

# --- firmware ---------------------------------------------------------------
if [ ! -f "$OUT/rootfs.img" ]; then
    [ -f "$FIRMWARE" ] || { echo "firmware image not found: $FIRMWARE" >&2; exit 1; }
    log "extracting rootfs from Update.img"
    python3 - "$FIRMWARE" "$OUT/rootfs.img" "$OFF_ROOTFS" <<'PY'
import lzma, sys
src, dst, start = sys.argv[1], sys.argv[2], int(sys.argv[3])
with open(src, 'rb') as f:
    f.seek(start)
    blob = f.read()
dec = lzma.LZMADecompressor(format=lzma.FORMAT_XZ)
with open(dst, 'wb') as out:
    out.write(dec.decompress(blob))
    while not dec.eof:
        chunk = dec.decompress(b'', max_length=1 << 22)
        if not chunk:
            break
        out.write(chunk)
print(f"  rootfs {len(open(dst,'rb').read()):,} bytes")
PY
    # The image is mounted read-write, so keep a pristine copy for rebuilds.
    if [ ! -f "$CACHE/rootfs.orig.img" ]; then
        cp "$OUT/rootfs.img" "$CACHE/rootfs.orig.img"
    fi
fi
# Always start from the pristine image: the guest writes to it (host keys, logs).
if [ -f "$CACHE/rootfs.orig.img" ]; then
    cp "$CACHE/rootfs.orig.img" "$OUT/rootfs.img"
fi

# --- downloads --------------------------------------------------------------
fetch() { # url filename
    [ -f "$CACHE/$2" ] && return 0
    log "downloading $2"
    curl -fsSL --max-time 600 -o "$CACHE/$2" "$1"
}
fetch "$DEB/pool/main/l/linux/$KERNEL_PKG" "$KERNEL_PKG"
fetch "$DEB/pool/main/b/busybox/$BUSYBOX_PKG" "$BUSYBOX_PKG"
fetch "$DEB/pool/main/a/alsa-utils/$ALSA_PKG" "$ALSA_PKG"

# --- unpack kernel, modules, busybox, tools ---------------------------------
mkdir -p "$OUT/k"
if [ ! -f "$OUT/busybox" ]; then
    log "unpacking kernel package"
    python3 "$HERE/unpack-deb.py" "$CACHE/$KERNEL_PKG" \
        --kernel-dir "$OUT/k" --modules-dir "$CACHE/modules"
    cp "$OUT/k/"vmlinuz-* "$OUT/k/vmlinuz" 2>/dev/null || true
    log "unpacking busybox"
    python3 "$HERE/unpack-deb.py" "$CACHE/$BUSYBOX_PKG" \
        --extract bin/busybox:"$OUT/busybox" --chmod 755
    log "unpacking alsa-utils"
    python3 "$HERE/unpack-deb.py" "$CACHE/$ALSA_PKG" \
        --extract "usr/bin/aplay:$OUT/tools/aplay" \
        --extract "usr/bin/amidi:$OUT/tools/amidi" \
        --extract "usr/bin/aconnect:$OUT/tools/aconnect" \
        --extract "usr/bin/speaker-test:$OUT/tools/speaker-test" --chmod 755
fi

# --- module dependency index (Debian ships no modules.dep) ------------------
if [ ! -f "$OUT/allmods.json" ]; then
    log "indexing kernel modules (slow, ~2 min, cached)"
    python3 "$HERE/modeps.py" scan "$CACHE/modules" -o "$OUT/allmods.json"
fi

# --- initramfs --------------------------------------------------------------
log "building initramfs"
python3 "$HERE/mkinitramfs.py" \
    --index "$OUT/allmods.json" \
    --deb "$CACHE/$KERNEL_PKG" \
    --busybox "$OUT/busybox" \
    --hook "$HERE/hook.sh" \
    --extra "$OUT/tools" \
    --boot-roots ext4,virtio_blk,crc32c_generic \
    --late-roots virtio-gpu,virtio_input,evdev,snd-hda-intel,snd-pcm,soundcore,drm,virtio_snd,snd-seq,snd-seq-midi,snd-seq-dummy,virtio_net \
    --out "$OUT/initramfs.cpio.gz"

# --- board identity device tree --------------------------------------------
log "building AC50 device tree"
python3 "$HERE/make-dtb.py" \
    --qemu "${QEMU:-/opt/homebrew/bin/qemu-system-aarch64}" \
    --dtc "${DTC:-/opt/homebrew/bin/dtc}" \
    --out "$OUT/ac50.dtb"

log "done -- $OUT"
ls -la "$OUT"
