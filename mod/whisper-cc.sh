#!/bin/bash
set -eu
args=()
skip_next=0
for arg in "$@"; do
    if [ "$skip_next" = 1 ]; then
        skip_next=0
        continue
    fi
    case "$arg" in
        -arch|-isysroot) skip_next=1 ;;
        -mmacosx-version-min=*|-Wl,-syslibroot,*) ;;
        *) args+=("$arg") ;;
    esac
done
exec zig cc -target aarch64-linux-gnu "${args[@]}"
