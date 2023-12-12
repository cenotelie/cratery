#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

source "$ROOT/build-env.sh"

cp "$ROOT/target/$RUST_TARGET/cratery" "$ROOT/cratery"

# Build the new image
docker build --tag "rg.fr-par.scw.cloud/cenotelie/cratery:$DOCKER_TAG" --rm --label commit="$HASH" "$ROOT"

# Cleanup
rm "$ROOT/cratery"
