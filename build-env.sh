#!/bin/bash

VERSION=$(git rev-parse --short HEAD)
HASH=$(git rev-parse HEAD)
TAG=$(git tag -l --points-at HEAD)

BUILD_TARGET=debug
DOCKER_TAG=latest
DEPLOY_CREDENTIALS=
for arg in "$@"
do
    case "$arg" in
    --target=*)
      BUILD_TARGET="${arg#*=}"
      ;;
    --deployCredentials=*)
      DEPLOY_CREDENTIALS="${arg#*=}"
      ;;
    *)
      printf "***************************\n"
      printf "* Error: Invalid argument: $arg\n"
      printf "***************************\n"
      exit 1
  esac
done

RUST_BUILD_ARG=
RUST_TARGET=debug
if [ "$BUILD_TARGET" = "production" ]; then
  RUST_BUILD_ARG="--release"
  RUST_TARGET="release"
  if [ ! -z "$TAG" -a "$TAG" != "tip" ]; then
    DOCKER_TAG="$TAG"
  fi
fi
if [ "$BUILD_TARGET" = "integration" ]; then
  RUST_BUILD_ARG="--release"
  RUST_TARGET="release"
  DOCKER_TAG="git-$VERSION"
fi
