#!/bin/bash
set -euo pipefail

# --- Configuration ---
REPOSITORY_NAME="cratery"
# Définition des données (Data)
REQUIRED_VARS=("INPUT_BUILD_TARGET" "GITHUB_REF_TYPE" "GITHUB_REF_NAME" "DOCKERHUB_USERNAME")

# --- Helper Functions ---
log_error() {
    echo "Error: $1" >&2
}

# Validates the presence of the necessary environment variables.
validate_env() {
    local vars=("$@")
    for var in "${vars[@]}"; do
        if [[ -z "${!var}" ]]; then
            log_error "Missing environment variable: $var"
            exit 1
        fi
    done
}


validate_env "${REQUIRED_VARS[@]}"

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

# Sanitize `ref_name` (from tag or branch name) for tag name
SAN_REF_NAME="${GITHUB_REF_NAME//\//-}"

# List base tags
if [ "$SAN_REF_NAME" = "main" ]; then
    BASE_TAGS="dev"
elif [ "$GITHUB_REF_TYPE" = "tag" ]; then
    BASE_TAGS="${SAN_REF_NAME} latest"
elif [ "$GITHUB_REF_TYPE" = "branch" ]; then
    BASE_TAGS="${SAN_REF_NAME}"
else
    log_error "Error: failed to compute docker image base tag from `ref_name` or `ref_type`"
    exit 1
fi

# Loop through each tag and construct both registry paths
# We use comma to separate tags as expected by docker/build-push-action
FINAL_TAGS=""
for tag in $BASE_TAGS; do
    # Append Docker Hub tag
    FINAL_TAGS+="${DOCKERHUB_USERNAME}/${REPOSITORY_NAME}:$tag,"
    # Append GitHub Registry tag
    FINAL_TAGS+="ghcr.io/${GITHUB_REPOSITORY_OWNER}/${REPOSITORY_NAME}:$tag,"
done
# Remove trailing comma
FINAL_TAGS="${FINAL_TAGS%,}"

# set tags_list variable for docker push
echo "tags_list=$FINAL_TAGS" >> $GITHUB_OUTPUT
