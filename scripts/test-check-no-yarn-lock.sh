#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
checker="$repository_root/scripts/check-no-yarn-lock.sh"
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT HUP INT TERM

sh "$checker" "$fixture"
touch "$fixture/yarn.lock"

if sh "$checker" "$fixture" >/dev/null 2>&1; then
  echo "expected yarn.lock check to fail" >&2
  exit 1
fi
