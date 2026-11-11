#!/bin/sh
# Thin wrapper: the build itself belongs to the public mpc-sample-sdk.
set -eu
cd "$(dirname "$0")"
rust_dir=$(CDPATH= cd -- ../rust && pwd)

# Find the SDK checkout cargo resolved, so this wrapper always uses the same
# build script and revision as the crate.  The dependency may be a git checkout
# or a local path (a temporary [patch] while the SDK contracts are validated).
sdk_dir=$(
    cargo metadata --format-version 1 --manifest-path "$rust_dir/Cargo.toml" |
    python3 -c '
import json
import pathlib
import sys

metadata = json.load(sys.stdin)
package = next(
    (item for item in metadata["packages"]
     if item["name"] == "mpc-sample-sdk"),
    None,
)
if package is None:
    sys.exit("mpc-sample-sdk is not a dependency")
print(pathlib.Path(package["manifest_path"]).parent)
'
)
[ -x "$sdk_dir/build-mod.sh" ] || {
    echo "mpc-sample-sdk build script missing at $sdk_dir/build-mod.sh" >&2
    exit 1
}

exec "$sdk_dir/build-mod.sh" \
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
    --env MINIMOD_DEBUG=0
