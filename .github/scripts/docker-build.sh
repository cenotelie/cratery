#!/bin/bash
set -euo pipefail

# --- Helper Functions ---
log_error() {
    echo "Error: $1" >&2
}

# Set BUILD_FLAGS & BUILD_TARGET variables for docker image build
if [ "${INPUT_BUILD_TARGET}" = "Debug" ]; then
    BUILD_FLAGS=""
    BUILD_TARGET="debug"
elif [ "${INPUT_BUILD_TARGET}" = "Release" ]; then
    BUILD_FLAGS="--release"
    BUILD_TARGET="release"
else
    log_error "Error: invalid `BUILD_TARGET_INPUT` environment variable. Expected: 'Debug' or 'Release'"
    exit 1
fi
echo "build_flags=${BUILD_FLAGS}" >> $GITHUB_OUTPUT
echo "build_target=${BUILD_TARGET}" >> $GITHUB_OUTPUT
