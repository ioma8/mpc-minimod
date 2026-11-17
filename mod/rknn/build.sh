#!/bin/sh
set -eu

HERE=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
MOD=$(CDPATH= cd -- "$HERE/.." && pwd)
BUILD="$MOD/build/rknn"
MODEL_DIR="$BUILD/model"
MODEL_ZOO=${RKNN_MODEL_ZOO:-"$BUILD/model-zoo"}
SOURCE="$BUILD/src"
BUNDLE="$MOD/build/rknn-bundle/autorun/rknn"

command -v uv >/dev/null 2>&1 || { echo "uv is required" >&2; exit 1; }
command -v docker >/dev/null 2>&1 || { echo "Docker is required for RKNN conversion and ARM64 helper build" >&2; exit 1; }
mkdir -p "$MODEL_DIR"

if [ ! -s "$MODEL_DIR/whisper_encoder_tiny.en_20s.rknn" ] || [ ! -s "$MODEL_DIR/whisper_decoder_tiny.en_20s.rknn" ]; then
    if [ ! -d "$BUILD/whisper/.git" ]; then
        git clone --depth 1 --branch v20231117 https://github.com/openai/whisper.git "$BUILD/whisper"
    fi
    echo "Exporting Whisper tiny.en ONNX graphs with uv..."
    MINIMOD_WHISPER_SOURCE="$BUILD/whisper" uv run --script "$HERE/export_tiny.py" --model-dir "$MODEL_DIR"
    echo "Building pinned RKNN conversion image..."
    docker build --platform linux/amd64 -t minimod-rknn-convert:2.5.0 \
        -f "$HERE/Dockerfile.convert" "$HERE"
    echo "Converting the encoder and decoder for RK3566..."
    docker run --rm --platform linux/amd64 \
        -v "$MOD:/work" -v "$BUILD/uv-cache:/root/.cache/uv" \
        minimod-rknn-convert:2.5.0 \
        uv run --script /work/rknn/convert_tiny.py \
        /work/build/rknn/model/whisper_encoder_tiny.en_20s.onnx \
        /work/build/rknn/model/whisper_encoder_tiny.en_20s.rknn
    docker run --rm --platform linux/amd64 \
        -v "$MOD:/work" -v "$BUILD/uv-cache:/root/.cache/uv" \
        minimod-rknn-convert:2.5.0 \
        uv run --script /work/rknn/convert_tiny.py \
        /work/build/rknn/model/whisper_decoder_tiny.en_20s.onnx \
        /work/build/rknn/model/whisper_decoder_tiny.en_20s.rknn
fi

if [ ! -d "$MODEL_ZOO/.git" ]; then
    git clone --depth 1 --branch v2.5.0 \
        https://github.com/airockchip/rknn_model_zoo.git "$MODEL_ZOO"
fi

mkdir -p "$SOURCE/cpp" "$SOURCE/include" "$SOURCE/lib" \
    "$BUNDLE/lib" "$BUNDLE/model"
cp "$MODEL_ZOO/examples/whisper/cpp/process.cc" "$SOURCE/cpp/process.cc"
cp "$MODEL_ZOO/examples/whisper/cpp/process.h" "$SOURCE/cpp/process.h"
cp "$MODEL_ZOO/examples/whisper/cpp/rknpu2/whisper.cc" "$SOURCE/cpp/whisper.cc"
cp "$MODEL_ZOO/examples/whisper/cpp/whisper.h" "$SOURCE/cpp/whisper.h"
cp "$HERE/server.cc" "$SOURCE/server.cc"
sed -E 's/^#define ENCODER_OUTPUT_SIZE.*/#define ENCODER_OUTPUT_SIZE CHUNK_LENGTH * 50 * 384/' \
    "$SOURCE/cpp/process.h" > "$SOURCE/cpp/process.h.tmp"
mv "$SOURCE/cpp/process.h.tmp" "$SOURCE/cpp/process.h"
sed '/#include <opencv2\/opencv.hpp>/d' "$SOURCE/cpp/process.cc" > "$SOURCE/cpp/process.cc.tmp"
mv "$SOURCE/cpp/process.cc.tmp" "$SOURCE/cpp/process.cc"
cp "$MODEL_ZOO/utils/audio_utils.h" "$MODEL_ZOO/utils/file_utils.h" \
    "$MODEL_ZOO/3rdparty/timer/easy_timer.h" \
    "$MODEL_ZOO/3rdparty/rknpu2/include/rknn_api.h" "$SOURCE/include/"
cp -R "$MODEL_ZOO/3rdparty/fftw/include/." "$SOURCE/include/"
cp "$MODEL_ZOO/3rdparty/fftw/Linux/aarch64/libfftw3f.a" "$SOURCE/lib/"
cp "$MODEL_ZOO/3rdparty/rknpu2/Linux/aarch64/librknnrt.so" "$BUNDLE/lib/"
cp "$MODEL_DIR/whisper_encoder_tiny.en_20s.rknn" \
    "$MODEL_DIR/whisper_decoder_tiny.en_20s.rknn" \
    "$MODEL_ZOO/examples/whisper/model/vocab_en.txt" \
    "$MODEL_ZOO/examples/whisper/model/mel_80_filters.txt" "$BUNDLE/model/"

docker build --platform linux/arm64 -t minimod-rknn-builder:2.5.0 \
    -f "$HERE/Dockerfile.builder" "$HERE"
docker run --rm --platform linux/arm64 -v "$MOD/build:/build" \
    minimod-rknn-builder:2.5.0 \
    g++ -std=c++17 -O3 -mcpu=cortex-a55 -fno-exceptions -no-pie \
    -I/build/rknn/src/cpp -I/build/rknn/src/include \
    /build/rknn/src/server.cc /build/rknn/src/cpp/process.cc /build/rknn/src/cpp/whisper.cc \
    /build/rknn/src/lib/libfftw3f.a -L/build/rknn-bundle/autorun/rknn/lib \
    -Wl,-rpath,'$ORIGIN/lib' -lrknnrt -ldl -lpthread \
    -o /build/rknn-bundle/autorun/rknn/minimod-whisper-rknn

file "$BUNDLE/minimod-whisper-rknn"
ls -lh "$BUNDLE/minimod-whisper-rknn" "$BUNDLE/lib/librknnrt.so" "$BUNDLE/model"/*.rknn
