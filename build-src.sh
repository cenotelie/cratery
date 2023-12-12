#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

source "$ROOT/build-env.sh"

(cd "$ROOT"; cargo update)
if [ "$RUST_BUILD_ARG" = "--release" ]; then
    (cd "$ROOT"; cargo build "$RUST_BUILD_ARG")
else
    (cd "$ROOT"; cargo build)
fi
