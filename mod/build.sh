#!/bin/sh
# Thin wrapper: the build itself belongs to the public mpc-sample-sdk.
set -eu
cd "$(dirname "$0")"
rust_dir=$(CDPATH= cd -- ../rust && pwd)

command -v uv >/dev/null 2>&1 || { echo "uv is required for Python build helpers" >&2; exit 1; }
sdk_dir=$(uv run --script tools/resolve_sdk.py "$rust_dir/Cargo.toml")
sdk_checkout=$(CDPATH= cd -- ../../mpc-sample-sdk 2>/dev/null && pwd || true)
if [ -n "$sdk_checkout" ] && [ -x "$sdk_checkout/build-mod.sh" ]; then
    sdk_dir=$sdk_checkout
fi
[ -x "$sdk_dir/build-mod.sh" ] || {
    echo "mpc-sample-sdk build script missing at $sdk_dir/build-mod.sh" >&2
    exit 1
}

variant=${1:-standard}
case "$variant" in standard|dictation|dictation-rknn) ;; *) echo "usage: ./build.sh [standard|dictation|dictation-rknn]" >&2; exit 2 ;; esac
mkdir -p build
case "$variant" in
    dictation|dictation-rknn)
        if [ ! -f build/minimod.zip ]; then
            "$0" standard
        fi
        if [ -f build/minimod.zip ]; then cp build/minimod.zip build/minimod-standard.zip; fi
        ;;
esac
if [ "$variant" = dictation-rknn ]; then
    ./rknn/build.sh
fi
if [ "$variant" = dictation ]; then
    ./moonshine/build.sh
fi
if [ "$variant" = dictation-rknn ]; then
set -- --features dictation-rknn
elif [ "$variant" = dictation ]; then
set -- --features dictation
else
set --
fi
"$sdk_dir/build-mod.sh" \
    --manifest-path "$rust_dir/Cargo.toml" \
    --package minimod \
    --name minimod \
    --output-dir build \
    --require-symbol g_screen \
    --require-symbol g_pads \
    --require-symbol g_pad_stride \
    --require-symbol g_open \
    --require-symbol g_click \
    --require-symbol g_buf_out \
    --pack \
    --firmware-sha256 44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f \
    --display-name minimod \
    --env MINIMOD_DEBUG=0 "$@"

if [ "$variant" = dictation ]; then
bundle="build/moonshine-bundle/autorun/moonshine"
rm -rf build/pkg/autorun/moonshine
cp -R "$bundle" build/pkg/autorun/moonshine
awk '
    !staged && /^mkdir -p \/run\/systemd\/system\/ac50.service.d/ {
        print "mkdir -p /run/minimod-moonshine/lib"
        print "if cp \"$DIR/moonshine/minimod-moonshine\" /run/minimod-moonshine/minimod-moonshine && cp \"$DIR/moonshine/lib/libmoonshine.so\" \"$DIR/moonshine/lib/libonnxruntime.so.1\" /run/minimod-moonshine/lib/; then"
        print "    chmod 755 /run/minimod-moonshine/minimod-moonshine"
        print "else"
        print "    echo MiniMod_Moonshine_worker_runtime_could_not_be_staged >&2"
        print "fi"
        staged = 1
    }
    !added && /MINIMOD_DEBUG=0/ {
        print "printf \"Environment=\\\"MINIMOD_MOONSHINE_DIR=%s/moonshine/model\\\"\\n\" \"$DIR\" >> /run/systemd/system/ac50.service.d/minimod.conf"
        print "echo Environment=MINIMOD_MOONSHINE_WORKER=/run/minimod-moonshine/minimod-moonshine >> /run/systemd/system/ac50.service.d/minimod.conf"
        print "echo Environment=MINIMOD_MOONSHINE_LIB_DIR=/run/minimod-moonshine/lib >> /run/systemd/system/ac50.service.d/minimod.conf"
        added = 1
    }
    { print }
' build/pkg/autorun/autorun.sh > build/pkg/autorun/autorun.sh.tmp
mv build/pkg/autorun/autorun.sh.tmp build/pkg/autorun/autorun.sh
chmod 755 build/pkg/autorun/autorun.sh
{
    cat build/pkg/autorun/autorun.json
    cat build/pkg/autorun/autorun.sh
    printf '%s' 'f7bed1c9-7e90-4731-9194-c996666b4331'
} | shasum -a 256 | awk '{printf "%s", $1}' > build/pkg/autorun/autorun.check
rm -f build/minimod-dictation.zip
(cd build/pkg && zip -qr ../minimod-dictation.zip autorun)
if [ -f build/minimod-standard.zip ]; then
    mv build/minimod-standard.zip build/minimod.zip
else
    rm -f build/minimod.zip
fi
echo "packed Moonshine Tiny CPU dictation into build/minimod-dictation.zip"
fi

if [ "$variant" = dictation-rknn ]; then
bundle="build/rknn-bundle/autorun/rknn"
[ -x "$bundle/minimod-whisper-rknn" ] || { echo "RKNN helper missing at $bundle" >&2; exit 1; }
cp rknn/probe.sh "$bundle/probe.sh"
rm -f build/pkg/autorun/ggml-tiny.en-q5_1.bin
rm -rf build/pkg/autorun/rknn
cp -R "$bundle" build/pkg/autorun/rknn
awk '
    !added && /^systemctl daemon-reload/ {
        print "printf \"Environment=\\\"MINIMOD_RKNN_DIR=%s/rknn\\\"\\n\" \"$DIR\" >> /run/systemd/system/ac50.service.d/minimod.conf"
        print "echo Environment=MINIMOD_RKNN_WORKER=/run/minimod-rknn/minimod-whisper-rknn >> /run/systemd/system/ac50.service.d/minimod.conf"
        print "echo Environment=MINIMOD_RKNN_LIB_DIR=/run/minimod-rknn/lib >> /run/systemd/system/ac50.service.d/minimod.conf"
        added = 1
    }
    !rknn_staged && /^mkdir -p \/run\/systemd\/system\/ac50.service.d/ {
        print "mkdir -p /run/minimod-rknn/lib"
        print "if cp \"$DIR/rknn/minimod-whisper-rknn\" /run/minimod-rknn/minimod-whisper-rknn && cp \"$DIR/rknn/lib/librknnrt.so\" /run/minimod-rknn/lib/librknnrt.so; then"
        print "    chmod 755 /run/minimod-rknn/minimod-whisper-rknn"
        print "else"
        print "    echo MiniMod_RKNN_helper_runtime_could_not_be_staged_in_run >&2"
        print "fi"
        rknn_staged = 1
    }
    !probe_added && /^systemctl restart ac50$/ {
        print "if [ ! -e \"$DIR/rknn/probe-once.done\" ]; then"
        print "    ("
        print "        sleep 30"
        print "        echo \"Starting RKNN probe: $(date)\""
        print "        /bin/sh \"$DIR/rknn/probe.sh\""
        print "        echo \"RKNN probe exit status: $?\""
        print "        : > \"$DIR/rknn/probe-once.done\""
        print "    ) </dev/null >>\"$DIR/rknn/probe-launch.log\" 2>&1 &"
        print "fi"
        probe_added = 1
    }
    { print }
' build/pkg/autorun/autorun.sh > build/pkg/autorun/autorun.sh.tmp
mv build/pkg/autorun/autorun.sh.tmp build/pkg/autorun/autorun.sh
chmod 755 build/pkg/autorun/autorun.sh
{
    cat build/pkg/autorun/autorun.json
    cat build/pkg/autorun/autorun.sh
    printf '%s' 'f7bed1c9-7e90-4731-9194-c996666b4331'
} | shasum -a 256 | awk '{printf "%s", $1}' > build/pkg/autorun/autorun.check
rm -f build/minimod-dictation-rknn.zip
(cd build/pkg && zip -qr ../minimod-dictation-rknn.zip autorun)
if [ -f build/minimod-standard.zip ]; then
    mv build/minimod-standard.zip build/minimod.zip
else
    rm -f build/minimod.zip
fi
echo "packed RK3566 NPU dictation into build/minimod-dictation-rknn.zip"
fi
