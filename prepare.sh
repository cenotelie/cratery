#!/bin/bash

SCRIPT="$(readlink -f "$0")"
ROOT="$(dirname "$SCRIPT")"

DATABASE_URL=sqlite://data/registry.db cargo sqlx prepare
