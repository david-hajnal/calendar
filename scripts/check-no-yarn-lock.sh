#!/bin/sh
set -eu

repository_root=${1:-"$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"}
yarn_lock=$(
  find "$repository_root" \
    \( -type d -name .git -o -type d -name node_modules \) -prune -o \
    -type f -name yarn.lock -print -quit
)

if [ -n "$yarn_lock" ]; then
  echo "Yarn lockfile is not allowed: $yarn_lock" >&2
  exit 1
fi
