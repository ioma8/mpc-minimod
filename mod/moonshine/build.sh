#!/bin/sh
set -eu

HERE=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
MOD=$(CDPATH= cd -- "$HERE/.." && pwd)
BUILD="$MOD/build/moonshine"
BUNDLE="$MOD/build/moonshine-bundle/autorun/moonshine"
MODEL="$BUILD/model"

mkdir -p "$MODEL" "$BUILD/lib" "$BUILD/include" "$BUNDLE/model" "$BUNDLE/lib"

download() {
    url=$1
    path=$2
    sha=$3
    if [ ! -f "$path" ]; then
        curl --fail --location --silent --show-error "$url" --output "$path"
    fi
    actual=$(shasum -a 256 "$path" | awk '{print $1}')
    [ "$actual" = "$sha" ] || {
        echo "SHA-256 mismatch for $path" >&2
        exit 1
    }
}

BASE=https://download.moonshine.ai/model/tiny-en/quantized/tiny-en
download "$BASE/encoder_model.ort" "$MODEL/encoder_model.ort" \
    94e90a4654fc45cdfedb77c4c08e1739f48862998e58fada384b25118134f221
download "$BASE/decoder_model_merged.ort" "$MODEL/decoder_model_merged.ort" \
    cf524c4862d36e9e5ab032eddc73637efd822d70e868ac575cf1a46e1e4708a0
download "$BASE/tokenizer.bin" "$MODEL/tokenizer.bin" \
    6884b35fd6377d4c4d32336a0bc152f36b64d1e45b6503683cdc238250a8472d

LIBRARY_ARCHIVE="$BUILD/moonshine-voice-linux-arm64.tar.gz"
download \
    https://github.com/moonshine-ai/moonshine/releases/download/v0.1.6/moonshine-voice-linux-arm64.tar.gz \
    "$LIBRARY_ARCHIVE" \
    1e0a88f27c428cab43590ce1635fe122406ef9e70236b58d077daf4b95cf162c
tar -xzf "$LIBRARY_ARCHIVE" -C "$BUILD" --strip-components=1

cp "$MODEL/encoder_model.ort" "$MODEL/decoder_model_merged.ort" \
    "$MODEL/tokenizer.bin" "$BUNDLE/model/"
cp "$BUILD/lib/libmoonshine.so" "$BUILD/lib/libonnxruntime.so.1" \
    "$BUNDLE/lib/"
cp "$HERE/worker.cpp" "$BUILD/worker.cpp"

docker build --platform linux/arm64 -t minimod-moonshine-builder:0.1.6 \
    -f "$HERE/Dockerfile.builder" "$HERE"
docker run --rm --platform linux/arm64 \
    -v "$MOD/build:/build" minimod-moonshine-builder:0.1.6 \
    g++ -std=c++17 -O3 -mcpu=cortex-a55 \
    -I/build/moonshine/include \
    /build/moonshine/worker.cpp \
    -L/build/moonshine/lib -Wl,-rpath,'$ORIGIN/lib' -lmoonshine \
    -o /build/moonshine-bundle/autorun/moonshine/minimod-moonshine

file "$BUNDLE/minimod-moonshine"
ls -lh "$BUNDLE/minimod-moonshine" "$BUNDLE/lib"/* "$BUNDLE/model"/*
