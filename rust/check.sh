#!/bin/sh
# Everything the Rust rewrite must pass before it ships:
#
#   1. formatting
#   2. clippy with warnings denied (no inline lint annotations anywhere)
#   3. the host test suite, including the golden fixtures recorded from the
#      original C DSP
#   4. the aarch64 cross build, which is what the device loads
set -eu
cd "$(dirname "$0")"

echo '== fmt =='
cargo fmt --all -- --check

echo '== clippy =='
cargo clippy --all-targets --workspace -- -D warnings

echo '== test =='
cargo test --workspace

echo '== cross build =='
cargo build --release --target aarch64-unknown-linux-gnu -p minimod --lib

echo 'all checks passed'
