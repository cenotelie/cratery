#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

GIT_TAG=$(git tag -l --points-at HEAD)
DOCKER_TAG=latest

if [[ -n "$GIT_TAG" ]]; then
    DOCKER_TAG="$GIT_TAG"
fi

docker build --tag "cenotelie/cratery:$DOCKER_TAG" --rm "$ROOT"
