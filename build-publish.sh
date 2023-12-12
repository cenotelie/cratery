#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

source "$ROOT/build-env.sh"

if [ "$BUILD_TARGET" = "production" ]; then
  docker push "rg.fr-par.scw.cloud/cenotelie/cratery:$DOCKER_TAG"
fi
