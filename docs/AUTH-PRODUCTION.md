# SQLite authorization setup and recovery

Production auth uses SQLite for the small single-instance installation approved
on 2026-10-08. The issuer remains `https://auth.hajnal.space`. Core keeps its own
calendar database; auth has a separate file/PVC. No PostgreSQL server, database
passwords or database TLS are needed. The candidate remains dormant; no live
conversion or deployment has been performed. Existing PostgreSQL data is preserved.

## Server setup

1. Before publishing to main, pause the actual root Flux Kustomization if you
   need to prevent automatic core/MCP promotion during setup. For this repository:
   `flux suspend kustomization flux-system -n flux-system`.
2. Publish the reviewed code and wait for CI and the three auth/core/MCP images.
   Pull the repository on production. Replace each candidate image tag under
   `deploy/flux/overlays/auth-cutover/charts/` with its published immutable SHA;
   keep every release suspended. Supply core's real SMTP host/from, existing
   mail Secret and image pull credentials if GHCR packages are private.
3. Install `kubectl`, `flux`, `helm`, Python 3/PyYAML, `openssl`, and `age`
   on the operator/server host. Configure `KUBECONFIG`, namespace `commoncal`, DNS/ingress, and a default
   StorageClass for local/block storage. SQLite WAL needs reliable filesystem
   locking; do not use NFS/shared network storage. Confirm the real ingress
   controller namespace matches the NetworkPolicy. Point `auth.hajnal.space` at
   the existing proxied ingress. Browser → Cloudflare uses the publicly trusted
   Cloudflare edge certificate. Cloudflare → Traefik uses the self-signed origin
   certificate in `commoncal-auth-tls`, covering `auth.hajnal.space`. Keep the
   existing Cloudflare proxying and **Full** mode. The origin Secret is not the
   public edge certificate. No ACME or database certificates are needed.
4. Preserve your age identity outside the repository and cluster, in recoverable
   secure storage. The identity already generated for PostgreSQL backups is
   reusable. Export its public recipient as `AUTH_BACKUP_AGE_RECIPIENT`.
5. Load strong `AUTH_BRIDGE_KEY`, at least two distinct comma-separated
   32-character `AUTH_COOKIE_KEYS`, `AUTH_JWKS_FILE` and `AUTH_SIGNING_KID` from
   protected storage. Generate these once for a fresh issuer and retain them.
   The JWKS needs a private RSA RS256 key of at least 2048 bits and a unique kid.
   Provision through stdin, without putting secret values in command arguments:

   ```bash
   set +x
   export KUBECONFIG=/etc/rancher/k3s/k3s.yaml  # or your actual kubeconfig
   export AUTH_BACKUP_AGE_RECIPIENT='age1...'
   # Export AUTH_BRIDGE_KEY, AUTH_COOKIE_KEYS, AUTH_SIGNING_KID, AUTH_JWKS_FILE.
   bash deploy/bootstrap-auth-sqlite.sh
   ```

   This first provisions or validates `commoncal-auth-tls`, then creates
   `commoncal-auth-secrets` and `commoncal-auth-backup`, and refuses
   silent backup recipient changes and validates the complete recipient with age. `AUTH_DATABASE_URL`, PostgreSQL passwords,
   and `deploy/bootstrap-auth-postgres.sh` are obsolete for this setup.
6. If PostgreSQL already contains OAuth state, perform the offline import below
   before starting SQLite auth. Otherwise the auth initContainer creates a fresh
   SQLite schema on the retained PVC.
7. Validate/apply the suspended candidate while root Flux stays paused:

   ```bash
   python3 scripts/test-auth-cutover-manifests.py
   bash scripts/validate-deploy.sh
   kubectl apply -k deploy/flux/overlays/auth-cutover
   flux resume helmrelease commoncal-auth -n flux-system
   kubectl rollout status deployment/commoncal-auth -n commoncal --timeout=10m
   kubectl logs deployment/commoncal-auth -n commoncal -c migrate
   curl --fail https://auth.hajnal.space/ready
   curl --fail https://auth.hajnal.space/.well-known/openid-configuration
   ```

   `/app/data/auth.sqlite` uses a retained 1 GiB RWO PVC. Exactly one replica and
   Recreate rollouts avoid overlapping auth pods. An initContainer migrates the
   same file before auth starts; server startup never migrates. Brief downtime
   on auth upgrades is expected. Auth requests 50m CPU/64 MiB, with a 256 MiB
   memory limit. The three steady services together request 200m CPU/256 MiB
   and allow 1 GiB RAM; the daily backup job requests an additional 25m CPU/64 MiB
   and allows 192 MiB RAM. Kubernetes, Flux and ingress add overhead. These are
   configured budgets, not measured usage guarantees.
8. Resume core, check the private bridge and complete real browser OAuth while
   MCP still advertises the previous issuer. Take an encrypted backup containing
   the OAuth records and demonstrate full recovery below. Run
   `deploy/auth-prerequisites.sh` successfully before resuming MCP.
9. Resume MCP, run public discovery and the clean-cache OpenCode canary in the
   candidate README. Record selected calendars, narrowing, revocation, restart
   refresh and two-account isolation.
10. Persist verified tags, configuration, suspension states and the production
    Kustomization reference in Git. Push and confirm remote state matches the
    running stack before resuming root Flux; otherwise it can overwrite staging.

Do not use the old PostgreSQL setup steps from earlier chat messages. Do not
remove existing PostgreSQL Secrets/PVCs as part of this transition.

## Auth-only origin TLS provisioning for an existing installation

On the production host, from a checkout containing this fix, run only:

```bash
set +x
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml  # use the actual kubeconfig
kubectl config current-context             # confirm the intended cluster
NAMESPACE=commoncal bash deploy/provision-auth-tls.sh
```

This command needs kubectl, Python 3 and OpenSSL with `verify -verify_hostname` support
(OpenSSL 1.1.1+ or 3.x). It requires no host Node.js, auth credentials, Helm,
Docker or Flux commands. It creates only `commoncal-auth-tls` if absent, using
RSA 2048, SHA-256, a 365-day lifetime and the `auth.hajnal.space` SAN. It reuses
an existing Secret only after checking its TLS type, decoded certificate/key,
matching public keys, hostname, current validity and at least 30 days remaining.
Invalid or expiring Secrets cause an error and are never overwritten. Cluster
read errors also abort. `DRY_RUN=1` performs server dry-run creation if missing.
Key material stays in private temporary files and is removed on exit/signals.

For the existing installation, do not rerun the full stack deployment or secret
bootstrap merely to fill this TLS gap. The auth-only command preserves signing,
cookie and bridge keys, backup credentials, `commoncal-tls`, SQLite PVCs, SMTP
settings and all Flux suspension states. Traefik watches the Secret; no workload
restart or release reconciliation is needed. If auth is already running, verify:

```bash
kubectl get secret commoncal-auth-tls -n commoncal -o jsonpath='{.type}{"\n"}'
curl --fail https://auth.hajnal.space/ready
curl --fail https://auth.hajnal.space/.well-known/openid-configuration
# Inspect the origin directly, using the real origin IP (not Cloudflare's IP):
openssl s_client -connect <ORIGIN_IP>:443 -servername auth.hajnal.space </dev/null 2>/dev/null \
  | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
```

The origin should present the self-signed auth certificate. Public requests see
Cloudflare's trusted edge certificate. If auth remains suspended, leave it in
that state and continue the staged setup gates separately when ready.

## Origin TLS rotation

Only when the helper reports an invalid/expiring existing Secret, schedule an
explicit rotation. Securely back up that Secret (the backup contains its private
key), then delete only the auth TLS Secret and rerun the auth-only helper:

```bash
set +x
umask 077
backup_dir=$(mktemp -d)
kubectl get secret commoncal-auth-tls -n commoncal -o json > "$backup_dir/auth-tls.json"
# Move this backup to protected storage before proceeding.
kubectl delete secret commoncal-auth-tls -n commoncal
NAMESPACE=commoncal bash deploy/provision-auth-tls.sh
```

Deletion/recreation can briefly interrupt origin TLS. Verify the origin SNI and
public endpoints above. If replacement fails, restore with
`kubectl apply -f <protected-backup>/auth-tls.json`. Retain the backup securely
for rollback, then remove it according to your key retention policy. Never
delete `commoncal-tls`, auth signing/cookie Secrets or PVCs for TLS rotation.

## Migration initContainer fails after switching to SQLite

A SQLite chart paired with an old PostgreSQL auth image fails in `migrate` and
can leave Helm stalled with `context deadline exceeded` and
`MissingRollbackTarget`. Check the migration output and actual image first:

```bash
kubectl logs deployment/commoncal-auth -n commoncal -c migrate
kubectl get deployment commoncal-auth -n commoncal \
  -o jsonpath='{.spec.template.spec.initContainers[?(@.name=="migrate")].image}{"\n"}'
```

The candidate's `sha-a52bbc...` placeholder predates SQLite. An error requiring
`DATABASE_URL` confirms that old migration code is running. Do not add a
PostgreSQL URL to the SQLite configuration or delete its PVC.

Wait for a successful **Promote main** workflow that publishes all three
auth/core/MCP images before selecting a replacement SHA. A commit existing on
GitHub does not establish that its images were published. The Dockerfiles now
upgrade inherited `perl-base` to at least `5.36.0-7+deb12u4`; earlier builds failed
the vulnerability scan before publication. The scan must pass for the new build.

Keep the root Flux Kustomization paused. In the server checkout, replace the
image tags in all three files under `deploy/flux/overlays/auth-cutover/charts/`
with verified published `sha-<full 40-character commit>` tags. Preserve real
SMTP values and pull credentials, and keep core/MCP suspended. Reapply the
candidate, then resume and reset the failed auth reconciliation:

```bash
kubectl apply -k deploy/flux/overlays/auth-cutover
flux resume helmrelease commoncal-auth -n flux-system
flux reconcile helmrelease commoncal-auth -n flux-system --reset --with-source
kubectl rollout status deployment/commoncal-auth -n commoncal --timeout=10m
kubectl logs deployment/commoncal-auth -n commoncal -c migrate
curl --fail https://auth.hajnal.space/ready
```

Retain the PVC and existing keys. Continue with the bridge, real OAuth and
encrypted recovery gates above before resuming MCP. Persist the verified
candidate values in Git before resuming root Flux.

## Encrypted backup and isolated restore

The CronJob uses the auth image, runs beside auth to share its RWO PVC, and has
no network access. Node's [SQLite online backup API](https://nodejs.org/download/release/latest-jod/docs/api/sqlite.html)
creates a consistent snapshot while writes continue. Integrity/schema are checked,
then age encrypts the snapshot before an atomic publication to
`/app/data/backups`. Plaintext stays in memory-backed temporary storage and is
removed after success/failure. Retention is 14 days; expired archives are pruned
only after successful encryption. Never copy only a live SQLite main file under WAL.

```bash
job="commoncal-auth-backup-proof-$(date +%s)"
kubectl create job --from=cronjob/commoncal-auth-backup "$job" -n commoncal
kubectl wait "job/$job" -n commoncal --for=condition=Complete --timeout=10m
kubectl logs "job/$job" -n commoncal
```

Export encrypted archives to a separate failure domain on a monitored schedule.
The in-cluster PVC alone does not protect against node/storage loss. Decrypt on
a controlled machine into an isolated empty directory:

```bash
umask 077
age --decrypt --identity /secure/auth-backup-identity.txt \
  --output /isolated/restore/auth.sqlite /exported/archive.sqlite.age
AUTH_SQLITE_PATH=/isolated/restore/auth.sqlite node --input-type=module <<'JS'
import { DatabaseSync } from 'node:sqlite';
const db = new DatabaseSync(process.env.AUTH_SQLITE_PATH, { readOnly: true });
try {
  if (db.prepare('PRAGMA integrity_check').get().integrity_check !== 'ok') {
    throw new Error('restore integrity failed');
  }
  if (db.prepare('SELECT MAX(version) AS version FROM schema_migration').get().version !== 1) {
    throw new Error('restore schema mismatch');
  }
  for (const table of ['provider_entity','interaction_handoff','authorization_audit','dcr_rate_bucket']) {
    console.log(table, db.prepare(`SELECT COUNT(*) AS count FROM ${table}`).get().count);
  }
} finally { db.close(); }
JS
```

Run an isolated auth instance on that file with the original signing/cookie keys
and issuer configuration, local binds or a controlled test ingress. Do not route
production traffic to it. Prove a pre-backup refresh token continues, and verify
representative client/grant/handoff/audit records without putting credentials in
reports. Remove temporary plaintext. Record archive, time, isolated target,
integrity/schema, record checks and OAuth result. Only after successful full
recovery create `commoncal-auth-restore-proof` with `verified=true` and these
evidence fields. The preflight checks this operator attestation and a completed
backup job; it does not replace recovery testing.

## Existing PostgreSQL issuer

Skip if PostgreSQL was never initialized. Otherwise stop the issuer and take a
recoverable source backup. Preserve issuer URL, signing/cookie keys and bridge
key. With the auth package installed, Node 22.16+ and a private destination
folder, load the existing source DSN as `DATABASE_URL` from protected storage.
Run from the repository root after installing the auth package dependencies:

```bash
export AUTH_SQLITE_PATH=/isolated/import/auth.sqlite  # must not exist
node slice1-lab/auth-server/src/import-postgres.mjs
```

The importer uses a consistent read-only source transaction and bounded batches.
It preserves provider payload/consumption, handoffs, audits, rate counters and
timestamps, refuses overwriting an existing destination, and removes a failed
new destination. Verify record counts and refresh continuation before copying
the stopped/checkpointed SQLite file to the auth PVC (UID/GID 65534, mode 0600).
Mount/copy only while auth is stopped. Retain PostgreSQL unchanged for rollback.
This operator import is separate from the production runtime. Existing PVCs and
Secrets are never deleted automatically.

## Rotation, rollback and verification

Retain previous public signing keys for the longest token lifetime when adding a
new kid. Keep previous cookie keys while sessions remain valid; coordinate
bridge-key rotation with core. Preserve the age identity for all retained
archives. Image rollback requires a compatible schema; never delete the auth PVC.

Production requires an existing migrated absolute SQLite file and has no
PostgreSQL fallback. Node 22.16+ is required for online backup; Node 22 labels
`node:sqlite` experimental. The actual production image is tested directly.
Legacy PostgreSQL code/charts remain for lab or source recovery and are excluded
from the production candidate and auth CI path.

```bash
./scripts/dev.sh auth-check          # disposable SQLite OAuth proof
./scripts/dev.sh auth-storage-check  # actual Node 22 image, age, non-root recovery
./scripts/dev.sh auth-import-check   # optional disposable legacy-source import proof
```

These local proofs do not replace live ingress, browser/OpenCode, off-cluster
export and recovery gates. This deployment supports one auth instance.
