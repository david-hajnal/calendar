#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
checker="$repository_root/scripts/verify-production-image.sh"

if [ ! -x "$checker" ]; then
  echo "expected production image verifier to be executable" >&2
  exit 1
fi

for required_check in \
  'docker image inspect' \
  'docker run --rm --entrypoint id' \
  'SESSION_SECRET is required in production' \
  '/health/ready' \
  'docker export'; do
  if ! grep -F "$required_check" "$checker" >/dev/null; then
    echo "expected production image verifier to check: $required_check" >&2
    exit 1
  fi
done
