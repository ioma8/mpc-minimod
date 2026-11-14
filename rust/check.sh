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
# The feature crate is safe except at explicit firmware boundaries. Every unsafe
# block carries a SAFETY comment (clippy's undocumented_unsafe_blocks enforces
# that); this guard also rejects a file that mentions `unsafe` without one, so a
# block cannot be added to a feature-policy module unnoticed.
for file in $(grep -rl 'unsafe' crates/minimod/src); do
    # `#[unsafe(...)]` attributes (no_mangle, export_name, link_section) are not
    # unsafe blocks; only code still mentioning `unsafe` after dropping them needs
    # the contract comment.
    if sed -E 's/#\[unsafe\(/#[attribute(/' "$file" | grep -Eq 'unsafe([[:space:]]*\{|[[:space:]]+fn)'; then
        if ! grep -q 'SAFETY' "$file"; then
            echo "unsafe without a SAFETY comment: $file" >&2
            exit 1
        fi
    fi
done

echo '== doc =='
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace

echo '== test =='
cargo test --workspace

echo '== cross build =='
cargo build --release --target aarch64-unknown-linux-gnu -p minimod --lib

echo 'all checks passed'
