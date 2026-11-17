#!/bin/sh
# Run from the SD card to capture RKNN worker launch/runtime diagnostics.
set -u

HERE=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
LOG="$HERE/rknn-probe-$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown).log"
exec >"$LOG" 2>&1

echo "MiniMod RKNN device probe"
echo "Started: $(date 2>&1)"
echo "Log: $LOG"
echo

section() { echo; echo "===== $* ====="; }
section "OS and CPU"
uname -a 2>&1
cat /etc/os-release 2>&1 || true
cat /proc/cpuinfo 2>&1
if command -v getconf >/dev/null 2>&1; then
    getconf LONG_BIT 2>&1 || true
    getconf GNU_LIBC_VERSION 2>&1 || true
fi
cat /proc/meminfo 2>&1

section "Board and kernel acceleration interfaces"
if [ -r /proc/device-tree/compatible ]; then
    tr '\000' '\n' </proc/device-tree/compatible 2>&1
fi
for path in /sys/class/devfreq/* /sys/class/misc/*rkn* /sys/bus/platform/drivers/*rkn* \
    /sys/kernel/debug/rknpu /proc/rknpu /proc/driver/rknpu; do
    [ -e "$path" ] || continue
    echo "--- $path"
    ls -la "$path" 2>&1 || true
    for attr in name governor cur_freq min_freq max_freq available_frequencies; do
        [ -r "$path/$attr" ] && { printf '%s=' "$attr"; cat "$path/$attr" 2>&1; }
    done
done
for root in /sys/firmware/devicetree/base /proc/device-tree /lib/modules/"$(uname -r)"; do
    [ -d "$root" ] || continue
    echo "--- RKNN/RKNPU entries under $root"
    find "$root" -maxdepth 8 \( -iname '*rknn*' -o -iname '*rknpu*' -o -iname '*npu*' \) -print 2>/dev/null | head -n 200
done
if [ -r /proc/config.gz ]; then
    zcat /proc/config.gz 2>/dev/null | grep -iE 'RKNPU|ROCKCHIP.*NPU' || true
fi

section "Mounts and relevant devices"
cat /proc/mounts 2>&1
for path in /dev/rknpu /dev/rknpu0 /dev/dri /sys/class/misc/rknpu /proc/rknpu; do
    ls -la "$path" 2>&1 || true
done
for path in /dev/mali0 /dev/dma_heap /dev/dma_heap/system /dev/dma_heap/linux,cma \
    /dev/ion /dev/galcore /dev/accel /dev/accel/*; do
    ls -la "$path" 2>&1 || true
done
lsmod 2>&1 | grep -iE 'rknn|rknpu|rockchip' || true

section "MiniMod service environment"
grep -R -n 'MINIMOD_RKNN_DIR\|LD_LIBRARY_PATH\|LD_PRELOAD' \
    /run/systemd/system/ac50.service.d /etc/systemd/system/ac50.service.d 2>&1 || true
for pid in $(pidof ac50 2>/dev/null || true); do
    echo "ac50 pid=$pid"
    tr '\000' '\n' <"/proc/$pid/environ" 2>/dev/null | \
        grep -E '^(MINIMOD_RKNN_DIR|LD_LIBRARY_PATH|LD_PRELOAD)=' || true
done

section "RKNN files"
echo "MINIMOD_RKNN_DIR=${MINIMOD_RKNN_DIR-<unset>}"
ls -la "$HERE" "$HERE/model" "$HERE/lib" 2>&1
for f in "$HERE/minimod-whisper-rknn" "$HERE/lib/librknnrt.so" \
    "$HERE/model/whisper_encoder_tiny.en_20s.rknn" \
    "$HERE/model/whisper_decoder_tiny.en_20s.rknn" \
    "$HERE/model/vocab_en.txt" "$HERE/model/mel_80_filters.txt"; do
    if [ -f "$f" ]; then
        ls -ln "$f"
        sha256sum "$f" 2>&1 || cksum "$f" 2>&1 || true
    else
        echo "MISSING: $f"
    fi
done

WORKER=${MINIMOD_RKNN_WORKER:-/run/minimod-rknn/minimod-whisper-rknn}
LIB=${MINIMOD_RKNN_LIB_DIR:-/run/minimod-rknn/lib}
MODEL="$HERE/model"
section "Executable and dynamic loader"
echo "Worker path: $WORKER"
echo "Runtime library path: $LIB"
command -v file >/dev/null 2>&1 && file "$WORKER" "$LIB/librknnrt.so" 2>&1 || true
command -v readelf >/dev/null 2>&1 && readelf -l "$WORKER" 2>&1 | grep -A2 'interpreter' || true
if command -v ldd >/dev/null 2>&1; then
    LD_LIBRARY_PATH="$LIB${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ldd "$WORKER" 2>&1 || true
fi

section "Other inference and graphics acceleration software"
for tool in clinfo vulkaninfo eglinfo glxinfo vainfo rknn_server strace timeout; do
    command -v "$tool" 2>&1 || true
done
for root in /lib /usr/lib /usr/local/lib /opt /vendor/lib /vendor/lib64; do
    [ -d "$root" ] || continue
    find "$root" -maxdepth 4 -type f \( \
        -iname '*opencl*' -o -iname '*vulkan*' -o -iname '*mali*' -o \
        -iname '*rknn*' -o -iname '*rknpu*' -o -iname '*armnn*' -o \
        -iname '*acl*' -o -iname '*nnapi*' \) -print 2>/dev/null
done
if command -v ldconfig >/dev/null 2>&1; then
    ldconfig -p 2>&1 | grep -iE 'opencl|vulkan|mali|rknn|rknpu|armnn|acl|nnapi' || true
fi
for tool in clinfo vulkaninfo eglinfo; do
    if command -v "$tool" >/dev/null 2>&1; then
        echo "--- $tool"
        "$tool" 2>&1 | head -n 120 || true
    fi
done

section "Worker startup test (30 second limit)"
if [ ! -x "$WORKER" ]; then
    echo "Worker is missing or not executable."
else
    LD_LIBRARY_PATH="$LIB${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
        "$WORKER" \
        "$MODEL/whisper_encoder_tiny.en_20s.rknn" \
        "$MODEL/whisper_decoder_tiny.en_20s.rknn" \
        "$MODEL/vocab_en.txt" "$MODEL/mel_80_filters.txt" \
        </dev/null >"$HERE/rknn-worker-probe.out" 2>&1 &
    worker_pid=$!
    elapsed=0
    while kill -0 "$worker_pid" 2>/dev/null && [ "$elapsed" -lt 30 ]; do
        sleep 1
        elapsed=$((elapsed + 1))
    done
    if kill -0 "$worker_pid" 2>/dev/null; then
        echo "Worker did not exit after stdin EOF within ${elapsed}s; terminating it."
        kill "$worker_pid" 2>&1 || true
        sleep 1
        kill -9 "$worker_pid" 2>/dev/null || true
    fi
    wait "$worker_pid"
    worker_status=$?
    echo "Worker exit status: $worker_status"
    echo "Worker stdout/stderr:"
    cat "$HERE/rknn-worker-probe.out" 2>&1 || true
    if command -v strace >/dev/null 2>&1; then
        echo "Repeating worker startup under strace (15 second limit)."
        if command -v timeout >/dev/null 2>&1; then
            timeout 15 strace -f -tt -s 256 -o "$HERE/rknn-worker-probe.strace" \
                env LD_LIBRARY_PATH="$LIB${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
                "$WORKER" \
                "$MODEL/whisper_encoder_tiny.en_20s.rknn" \
                "$MODEL/whisper_decoder_tiny.en_20s.rknn" \
                "$MODEL/vocab_en.txt" "$MODEL/mel_80_filters.txt" \
                </dev/null >"$HERE/rknn-worker-probe-strace.out" 2>&1
            trace_status=$?
            echo "strace worker exit status: $trace_status"
            cat "$HERE/rknn-worker-probe-strace.out" 2>&1 || true
            tail -n 300 "$HERE/rknn-worker-probe.strace" 2>&1 || true
        else
            echo "strace is installed but timeout is not; skipping trace to avoid a hanging diagnostic."
        fi
    fi
fi

section "Recent kernel diagnostics"
dmesg 2>&1 | grep -iE 'rknn|rknpu|iommu|dma|firmware|opencl|vulkan|mali|denied|segfault|error' | tail -n 300 || true

section "End"
echo "Finished: $(date 2>&1)"
echo "Attach this log and any rknn-worker-probe.* artifacts when reporting the result."
echo "Probe log: $LOG"
