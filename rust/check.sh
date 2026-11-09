#!/bin/sh
# Everything the Rust rewrite must pass before it ships:
#
#   1. formatting
#   2. clippy with warnings denied (only the two documented layout exceptions)
#   3. the host test suite, including the golden fixtures recorded from the
#      original C DSP
#   4. the aarch64 cross build, which is what the device loads
set -eu
cd "$(dirname "$0")"

echo '== fmt =='
cargo fmt --all -- --check

echo '== clippy =='
cargo clippy --all-targets --workspace -- -D warnings

echo '== feature safety =='
# The feature crate is safe Rust only; every firmware access is an SDK call.
if grep -RIn --include='*.rs' -E '\bunsafe\b' crates/minimod/src; then
    echo "unsafe code found in the feature crate" >&2
    exit 1
fi

echo '== doc =='
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace

echo '== test =='
cargo test --workspace

echo '== cross build =='
cargo build --release --target aarch64-unknown-linux-gnu -p minimod --lib

echo 'all checks passed'
