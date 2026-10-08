#!/usr/bin/env bash
set -euo pipefail
umask 077
: "${PGHOST:?}" "${PGUSER:?}" "${PGDATABASE:?}" "${PGPASSWORD:?}"
: "${AGE_RECIPIENT:?Off-cluster age identity recipient is required}"
backup_dir=${AUTH_BACKUP_DIR:-/backup}
retention_days=${AUTH_BACKUP_RETENTION_DAYS:-14}
[[ "$retention_days" =~ ^[1-9][0-9]*$ ]] || { echo 'Invalid backup retention' >&2; exit 1; }
work_dir=$(mktemp -d)
partial_output=
trap 'rm -rf "$work_dir"; [[ -z "$partial_output" ]] || rm -f "$partial_output"' EXIT
# Fail before publishing any partial output; raw snapshots exist only in tmpfs.
pg_dump --format=custom --no-owner --no-acl --file="$work_dir/auth.dump"
pg_restore --list "$work_dir/auth.dump" >/dev/null
snapshot="auth-$(date -u +%Y%m%dT%H%M%SZ)-${RANDOM}.dump.age"
partial_output="$backup_dir/.$snapshot.tmp"
age --recipient "$AGE_RECIPIENT" --output "$partial_output" "$work_dir/auth.dump"
mv "$partial_output" "$backup_dir/$snapshot"
partial_output=
find "$backup_dir" -maxdepth 1 -type f -name 'auth-*.dump.age' -mtime "+$retention_days" -delete
printf 'Encrypted OAuth database backup completed: %s\n' "$snapshot"
