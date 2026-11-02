#!/bin/sh
# Boot the AC50 firmware under QEMU with a display and audio.
#
#   ./run.sh                      native window (cocoa) + speakers
#   ./run.sh --headless           no window, serial console on stdin
#   ./run.sh --vnc :0             VNC on port 5900 instead of a window
#   ./run.sh --shot out.png       headless, capture the panel once, exit
#   ./run.sh --console "id" ...   run shell commands in the guest, then power off
#
# The guest keeps running until you close the window or press Ctrl-C.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
CACHE=${MPC2STEMS_CACHE:-$HOME/Library/Caches/mpc2stems/emu}
OUT=${MPC2STEMS_EMU_OUT:-$CACHE/out}
QEMU=${QEMU:-/opt/homebrew/bin/qemu-system-aarch64}

# The panel is 320x240 (2.4" MIPI-DBI display, rotated). The app always draws a
# 320x240 canvas, so match the mode to it or the image sits in one corner.
XRES=320
YRES=240
MEM=4096
SMP=4

AUDIO=coreaudio
DISPLAY=cocoa
ZOOM=0
VNCDISP=1
SERIAL="file:$CACHE/out/serial.log"
SHOT=
CONSOLE=0

while [ $# -gt 0 ]; do
    case "$1" in
        --headless) DISPLAY=none; SERIAL=mon:stdio; shift ;;
        --vnc) DISPLAY="vnc:$2"; shift 2 ;;
        --shot) SHOT="$2"; DISPLAY=none; shift 2 ;;
        --console) CONSOLE=1; shift; break ;;  # everything after is a guest command
        --no-audio) AUDIO=none; shift ;;
        --zoom) ZOOM="$2"; DISPLAY=none; shift 2 ;;
        --vnc*|--zoom*) ;;
        --audio) AUDIO="$2"; shift 2 ;;
        --) shift; break ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

[ -f "$OUT/initramfs.cpio.gz" ] || { echo "run tools/emulator/build.sh first" >&2; exit 1; }

if [ "$CONSOLE" = 1 ]; then
    exec python3 "$HERE/console.py" --dir "$OUT" --xres $XRES --yres $YRES \
        --mem $MEM --cpus $SMP --audio "$AUDIO" "$@"
fi

AUDIO_ARGS=""
[ "$AUDIO" = none ] || AUDIO_ARGS="-audiodev $AUDIO,id=aud0 -device virtio-sound-pci,audiodev=aud0,streams=1"

set -- \
    -M virt -accel hvf -cpu host -smp $SMP -m $MEM \
    -kernel "$OUT/k/vmlinuz" -initrd "$OUT/initramfs.cpio.gz" -dtb "$OUT/ac50.dtb" \
    -append "console=ttyAMA0" \
    -drive "file=$OUT/rootfs.img,format=raw,if=virtio,cache=unsafe" \
    -device "virtio-gpu-pci,xres=$XRES,yres=$YRES" \
    -device virtio-keyboard-pci -device virtio-tablet-pci \
    -netdev user,id=net0,hostfwd=tcp::8080-:80,hostfwd=tcp::2222-:22 \
    -device virtio-net-pci,netdev=net0 \
    -qmp "unix:$OUT/qmp.sock,server=on,wait=off" \
    $AUDIO_ARGS

if [ -n "$SHOT" ]; then
    # Boot headless, wait for the app to paint, grab the framebuffer, stop.
    "$QEMU" "$@" -display none -serial "file:$OUT/serial.log" &
    pid=$!
    trap 'kill $pid 2>/dev/null || true' EXIT INT TERM
    sleep 30
    python3 "$HERE/qmp.py" --dir "$OUT" --width $XRES --height $YRES shot "$SHOT"
    kill $pid 2>/dev/null || true
    wait $pid 2>/dev/null || true
    echo "captured $SHOT"
    exit 0
fi

if [ "$ZOOM" != 0 ]; then
    # QEMU's cocoa display cannot upscale (`zoom-to-fit` only shrinks) and the
    # app always paints 320x240, so magnify in the browser, which scales with
    # nearest-neighbour pixels.
    "$QEMU" "$@" -display none -serial "$SERIAL" &
    qpid=$!
    trap 'kill $qpid 2>/dev/null || true' INT TERM
    sleep 3
    uv run --project "$HERE/../.." python "$HERE/panel.py" \
        --dir "$OUT" --port 8090 --zoom "$ZOOM" --width $XRES --height $YRES &
    ppid=$!
    trap 'kill $qpid $ppid 2>/dev/null || true' INT TERM
    sleep 2
    echo "Panel:          http://127.0.0.1:8090/  (${XRES}x${YRES} at ${ZOOM}x)"
    open "http://127.0.0.1:8090/" 2>/dev/null || true
    wait $qpid
    kill $ppid 2>/dev/null || true
    exit 0
fi

echo "Panel:          the QEMU window"
echo "Web front end:  http://localhost:8080/    (after the guest takes its lease)"
echo "SSH:            ssh -p 2222 root@localhost"
echo "Kernel log:     $OUT/serial.log"
exec "$QEMU" "$@" -display "$DISPLAY" -serial "$SERIAL"
