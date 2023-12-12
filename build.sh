#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

"$ROOT/cmd" /src/build-src.sh $@
"$ROOT/build-docker.sh" $@
