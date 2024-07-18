#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

GIT_TAG=$(git tag -l --points-at HEAD)
DOCKER_TAG=latest
BUILD_FLAGS=""
BUILD_TARGET="debug"

for arg in "$@"
do
    case "$arg" in
    --release)
        BUILD_FLAGS="--release"
        BUILD_TARGET="release"
        if [[ -n "$GIT_TAG" ]]; then
            # release mode and has a git tag => tag the image with the version
            DOCKER_TAG="$GIT_TAG"
        fi
      ;;
    *)
      printf "***************************\n"
      printf "* Error: Invalid argument: $arg\n"
      printf "***************************\n"
      exit 1
  esac
done


docker build --tag "cenotelie/cratery:$DOCKER_TAG" --rm \
    --build-arg="BUILD_FLAGS=$BUILD_FLAGS" \
    --build-arg="BUILD_TARGET=$BUILD_TARGET" \
    "$ROOT"
