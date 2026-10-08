#!/usr/bin/env bash
set -euo pipefail
umask 077
[[ "$(psql -Atc "SELECT rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication FROM pg_roles WHERE rolname=current_user")" == f ]]
[[ "$(psql -Atc 'SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()')" == t ]]
if psql -v ON_ERROR_STOP=1 -c 'CREATE ROLE forbidden_superuser SUPERUSER' >/dev/null 2>&1; then
  echo 'restricted application role unexpectedly created a superuser' >&2; exit 1
fi
if PGSSLROOTCERT=/fixture/untrusted-ca.crt psql -Atc 'SELECT 1' >/dev/null 2>&1; then
  echo 'database accepted an untrusted certificate' >&2; exit 1
fi
psql -v ON_ERROR_STOP=1 <<'SQL'
CREATE TABLE storage_proof (id INTEGER PRIMARY KEY, marker TEXT NOT NULL);
INSERT INTO storage_proof VALUES (1, 'durable-oauth-backup-proof');
SQL
export AGE_RECIPIENT
AGE_RECIPIENT=$(cat /proof/recipient)
commoncal-auth-backup
snapshot=$(find /backup -maxdepth 1 -name 'auth-*.dump.age' -type f)
[[ -n "$snapshot" && "$(find /backup -maxdepth 1 -type f | wc -l)" -eq 1 ]]
if pg_restore --list "$snapshot" >/dev/null 2>&1; then
  echo 'backup published a plaintext snapshot' >&2; exit 1
fi
if AGE_RECIPIENT=invalid-disposable-recipient commoncal-auth-backup >/dev/null 2>&1; then
  echo 'backup unexpectedly accepted an invalid encryption recipient' >&2; exit 1
fi
[[ "$(find /backup -maxdepth 1 -type f | wc -l)" -eq 1 ]]
age --decrypt -i /proof/identity.age -o /tmp/restored.dump "$snapshot"
PGHOST=restore pg_restore --single-transaction --exit-on-error --no-owner --no-acl --dbname=commoncal_auth /tmp/restored.dump
[[ "$(PGHOST=restore psql -Atc 'SELECT marker FROM storage_proof WHERE id=1')" == durable-oauth-backup-proof ]]
PGHOST=restore psql -v ON_ERROR_STOP=1 -c "UPDATE storage_proof SET marker='isolated-restore' WHERE id=1" >/dev/null
[[ "$(psql -Atc 'SELECT marker FROM storage_proof WHERE id=1')" == durable-oauth-backup-proof ]]
started=$(psql -Atc 'SELECT pg_postmaster_start_time()')
# Replace the leaf with another certificate under a separate disposable CA.
# A successful connection with only the new CA proves the postmaster reloaded
# the mounted leaf without a restart; the old CA must then stop working.
cp /proof/renewed.crt /tls/tls.crt
cp /proof/renewed.key /tls/tls.key
chown 999:999 /tls/tls.crt /tls/tls.key
chmod 600 /tls/tls.key
reloaded=0
for attempt in {1..45}; do
  if PGSSLROOTCERT=/fixture/renewed-ca.crt psql -Atc 'SELECT 1' >/dev/null 2>&1; then reloaded=1; break; fi
  sleep 1
done
[[ "$reloaded" == 1 ]] || { echo 'database leaf certificate did not reload' >&2; exit 1; }
[[ "$(PGSSLROOTCERT=/fixture/renewed-ca.crt psql -Atc 'SELECT pg_postmaster_start_time()')" == "$started" ]]
if psql -Atc 'SELECT 1' >/dev/null 2>&1; then
  echo 'database still serves its previous TLS certificate' >&2; exit 1
fi
rm -f /tmp/restored.dump
printf '%s\n' 'production PostgreSQL storage proof passed: restricted role, verified TLS, encrypted backup, isolated restore, live TLS leaf reload'
