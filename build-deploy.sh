#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

source "$ROOT/build-env.sh"

curl -X POST \
        -H "Authorization: Bearer $DEPLOY_CREDENTIALS" \
        -H 'Content-Type: application/json' \
        -d "{\"message\": \"Deploy Cratery $DOCKER_TAG\", \"args\": [\"$DOCKER_TAG\"]}" \
        "https://deploy.cenotelie.fr/actions/public/deploy-cargo.sh"

echo ""
