#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

FILE="$ROOT/src/empty.db"

rm -f "$FILE"
touch "$FILE"

cat "$ROOT/src/schema.sql" | sqlite3 "$FILE"
