# Dormant Phase 6 cutover candidate

Prepared and locally rendered on 2026-10-08. The active production overlay does
not reference this directory. Every candidate HelmRelease is suspended. The image
tags copied from the existing stack are placeholders for configuration review;
they do not prove that the Phase 5/6 images exist or contain these changes.

The dependency order is auth → core → MCP. Auth stores state in SQLite on its retained PVC. Auth does not depend on
core: its readiness checks the migrated SQLite file, while browser interaction
redirects to core only after the stack is available. Core reads
`AUTH_BRIDGE_SECRET` from Secret `commoncal-auth-secrets`, data key
`AUTH_BRIDGE_KEY`; auth reads the same data key as `AUTH_BRIDGE_KEY`. Core's
bridge timeout uses `AUTH_BRIDGE_TIMEOUT_SECS`. MCP's primary issuer is the
single literal `MCP_OAUTH_ISSUER=https://auth.hajnal.space`; its primary and hold
issuer Secret references are disabled to prevent duplicate environment entries.

Render and verify locally without deploying:

```sh
kubectl kustomize deploy/flux/overlays/auth-cutover
python3 scripts/test-auth-cutover-manifests.py
bash scripts/validate-deploy.sh
```

The cross-chart test renders operator SMTP host/from fixtures because those
production values are not present in the existing Flux core HelmRelease. Supply
real `mail.host`, `mail.port` and `mail.from` in the candidate core values before
activation. Do not deploy the rendering fixtures.

## Operator activation gates

1. Publish immutable build images for auth, core and MCP, and replace
   every candidate image tag with the corresponding verified published SHA tag.
   Confirm image pull credentials for all three repositories.
2. Follow `docs/AUTH-PRODUCTION.md` to provision auth and age recipient Secrets,
   the retained SQLite PVC and public TLS. No database server, passwords or
   database certificates are needed. Preserve existing signing/cookie keys and
   import stopped PostgreSQL state first if an issuer was previously initialized.
3. Review the Git change connecting the candidate to production. Keep all
   releases suspended initially. Root Flux must remain paused during manual
   staging; persist final configuration before resuming root reconciliation.
4. Unsuspend auth first, prove initContainer migration and `/ready`, then core
   and its private bridge. While the issuer remains unadvertised, prove the real
   OAuth flow, take an encrypted online SQLite backup and complete the full OAuth
   recovery drill in `docs/AUTH-PRODUCTION.md`. Run `deploy/auth-prerequisites.sh`
   successfully before unsuspending MCP. `dependsOn` enforces readiness order;
   it does not enforce these operator proofs. Exactly one auth replica and
   Recreate upgrades are required. Backup jobs share its RWO volume on the same node.
5. Run public-ingress acceptance below and the browser/OpenCode canary. Record
   results before broad client cutover. Keep the previous image/configuration
   revisions and database backups available for operator rollback.

No step above has been executed against production by this change.

## Public-ingress acceptance

The existing discovery harness supports public ingress and writes a record:

```sh
mkdir -p /tmp/commoncal-ingress-acceptance
MCP_URL=https://mcal.hajnal.space/mcp \
MCP_OAUTH_ISSUER=https://auth.hajnal.space \
MCP_RECORD_DIR=/tmp/commoncal-ingress-acceptance \
  node mcp-acceptance/discovery.js
```

It must prove the unauthenticated Bearer challenge, public resource metadata and
issuer metadata through TLS ingress. It validates the advertised `jwks_uri` URL
but does not fetch its contents. Separately fetch that URL with verified TLS and
check the public signing-key set, check issuer readiness, and verify that public ingress
cannot reach `/internal/interactions/...`, `/internal/audit` or lab test hooks.
Public-host reachability alone does not verify the private bridge NetworkPolicy;
prove an allowed core-to-auth call and denial from an unrelated workload in the
cluster.

The current `mcp-acceptance/oauth-lifecycle.js` is a mock-oriented regression
harness, not a production browser acceptance driver. Its shared OAuth helper
requests `/authorize`, expects an immediate code redirect, omits the resource
parameter and has no browser login, consent or cookie handling. Production
`oidc-provider` advertises `/auth` and requires the real browser interaction.
Its default `/callback` redirect also differs from the admitted OpenCode shape
`http://127.0.0.1:<unprivileged-port>/mcp/oauth/callback`.
`security-isolation.js` depends on mock `/_test/token/mint` and `/_test/grant/*`
hooks, which production intentionally does not expose. Do not enable those hooks
or claim these suites prove production OAuth or grant isolation.

Use a clean OpenCode OAuth cache and a dedicated disposable CommonCal test
account to complete browser login and calendar consent using the released client.
Record nine-tool discovery, successful `calendar_list` for the approved account,
calendar-scope narrowing, immediate rejection after CommonCal grant revocation,
and refresh after an authorization-server restart. Denial, wrong-account and
cross-client isolation also need real-account proofs. Keep token/refresh-secret
values out of acceptance reports. This browser/client canary and the cluster
NetworkPolicy, backup and ingress proofs remain live release gates.
