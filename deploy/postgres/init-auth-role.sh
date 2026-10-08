#!/usr/bin/env bash
set -euo pipefail
: "${AUTH_DATABASE_PASSWORD:?AUTH_DATABASE_PASSWORD is required}"
# The application owns its database without PostgreSQL superuser privileges.
psql --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" --set=ON_ERROR_STOP=1 <<'SQL'
\getenv auth_password AUTH_DATABASE_PASSWORD
SELECT format('CREATE ROLE commoncal_auth LOGIN PASSWORD %L', :'auth_password') \gexec
ALTER DATABASE commoncal_auth OWNER TO commoncal_auth;
ALTER SCHEMA public OWNER TO commoncal_auth;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
SQL
