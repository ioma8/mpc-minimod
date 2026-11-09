#!/bin/sh
# Compatibility alias: build.sh already creates build/minimod.zip through the
# SDK's build-mod.sh.
set -eu
cd "$(dirname "$0")"
exec ./build.sh
