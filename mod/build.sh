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
case "$variant" in standard|dictation) ;; *) echo "usage: ./build.sh [standard|dictation]" >&2; exit 2 ;; esac
mkdir -p build
if [ "$variant" = dictation ] && [ -f build/minimod.zip ]; then
    cp build/minimod.zip build/minimod-standard.zip
fi
model="build/ggml-tiny.en-q5_1.bin"
if [ "$variant" = dictation ]; then
if [ ! -s "$model" ]; then
    echo "Downloading Whisper tiny.en-q5_1 model (31 MB)..."
    curl --fail --location --silent --show-error \
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.en-q5_1.bin" \
        --output "$model"
fi
model_sha=$(shasum -a 256 "$model" | awk '{print $1}')
[ "$model_sha" = "c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b" ] || {
    echo "Whisper model checksum mismatch" >&2
    exit 1
}
llvm_ar=$(command -v llvm-ar || true)
llvm_ranlib=$(command -v llvm-ranlib || true)
[ -n "$llvm_ar" ] || llvm_ar=/opt/homebrew/opt/llvm/bin/llvm-ar
[ -n "$llvm_ranlib" ] || llvm_ranlib=/opt/homebrew/opt/llvm/bin/llvm-ranlib
[ -x "$llvm_ar" ] || { echo "llvm-ar is required to build Whisper" >&2; exit 1; }
[ -x "$llvm_ranlib" ] || { echo "llvm-ranlib is required to build Whisper" >&2; exit 1; }
fi

if [ "$variant" = dictation ]; then
CMAKE_C_COMPILER="$(pwd)/whisper-cc.sh" \
CMAKE_CXX_COMPILER="$(pwd)/whisper-cxx.sh" \
CMAKE_ASM_COMPILER="$(pwd)/whisper-cc.sh" \
CMAKE_TOOLCHAIN_FILE="$(pwd)/whisper-toolchain.cmake" \
CMAKE_C_FLAGS="-D_GNU_SOURCE" \
CMAKE_AR="$llvm_ar" \
CMAKE_RANLIB="$llvm_ranlib" \
GGML_BLAS=OFF \
GGML_ACCELERATE=OFF \
WHISPER_DONT_GENERATE_BINDINGS=1 \
export CMAKE_C_COMPILER CMAKE_CXX_COMPILER CMAKE_ASM_COMPILER CMAKE_TOOLCHAIN_FILE CMAKE_C_FLAGS CMAKE_AR CMAKE_RANLIB GGML_BLAS GGML_ACCELERATE WHISPER_DONT_GENERATE_BINDINGS
set -- --features dictation --env MINIMOD_WHISPER_MODEL=/run/minimod-whisper.bin
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
cp "$model" build/pkg/autorun/ggml-tiny.en-q5_1.bin
awk '
    !added && /^cp / {
        print "cp \"$DIR/ggml-tiny.en-q5_1.bin\" /run/minimod-whisper.bin || exit 0"
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
} | shasum -a 256 | awk '{print $1}' > build/pkg/autorun/autorun.check
rm -f build/minimod-dictation.zip
(cd build/pkg && zip -qr ../minimod-dictation.zip autorun)
if [ -f build/minimod-standard.zip ]; then
    mv build/minimod-standard.zip build/minimod.zip
else
    rm -f build/minimod.zip
fi
echo "packed Whisper model into build/minimod-dictation.zip"
fi
