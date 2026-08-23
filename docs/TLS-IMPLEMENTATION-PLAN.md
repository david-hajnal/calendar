# Cloudflare Edge TLS + Self-Signed Origin TLS Implementation Plan

## Goal

Replace the repository's Let's Encrypt/cert-manager production path with:

- Cloudflare Universal SSL for browser-to-Cloudflare TLS.
- One self-signed Kubernetes TLS Secret for Cloudflare-to-Traefik TLS.
- Cloudflare zone SSL/TLS mode `Full`, because `Full (strict)` rejects a
  self-signed origin certificate.
- The existing shared secret name, `commoncal-tls`, for both
  `cal.hajnal.space` and `mcal.hajnal.space`.

This plan does not use Cloudflare Origin CA. Origin CA would permit
`Full (strict)`, but it is not a self-signed-certificate design.

## Current State

- Cloudflare already proxies both production domains and presents the public
  wildcard edge certificate.
- `deploy/deploy-prod.sh` installs cert-manager, creates the
  `letsencrypt-prod` ClusterIssuer, and waits for a cert-manager Certificate.
- The core Flux HelmRelease has the
  `cert-manager.io/cluster-issuer: letsencrypt-prod` annotation.
- Both Ingresses read the shared `commoncal-tls` Secret.
- Direct Helm deployment refuses to create a self-signed certificate and only
  accepts a pre-provisioned certificate.
- `scripts/test-deploy-prod-stack.sh` contains most of the TLS deployment
  regression coverage and mocks.

## Fixed Design Decisions

- Generate the certificate in `deploy/deploy-prod.sh` with OpenSSL. Do not add
  cert-manager, a new controller, a Cloudflare API token, or a private key to
  Git.
- Create the Secret before either Flux reconciliation or direct Helm install.
- Generate only when the Secret is absent. Never overwrite an existing Secret
  implicitly.
- Put both production DNS names in the certificate SAN extension.
- Use an RSA 2048-bit key, SHA-256, and a 365-day validity period.
- Validate every existing Secret before reuse: type, parseable certificate,
  matching private key, both SANs, and at least 30 days remaining.
- Fail with rotation instructions when validation fails or expiry is near.
  Rotation remains an explicit operator action.
- Keep both Ingresses on Traefik's `websecure` entrypoint and keep their shared
  `ingress.tls[0].secretName` contract.
- Do not delete cert-manager from a live cluster. Stop depending on it; cluster
  cleanup is a separate operator action because other workloads may use it.

## Task Format

Each task below is intentionally small enough for a low-level LLM. Implement
one task at a time. Run its checks before starting the next task. Do not combine
unrelated cleanup with a task.

## Tasks

### TLS-01: Record the new TLS contract in the deploy-script test fixture

**Depends on:** none

**Files:**

- `scripts/test-deploy-prod-stack.sh`

**Work:**

1. Add fixture state for whether `commoncal-tls` exists.
2. Make the mocked `kubectl get secret` return absent/present deterministically.
3. Make mocked `kubectl create secret tls` record its arguments and consume
   certificate/key file paths without recording their contents.
4. Extend the OpenSSL mock to support the exact certificate generation and
   validation commands planned below.
5. Remove no existing assertions in this task.

**Acceptance:** The fixture can observe a generated TLS Secret without leaking
the private key into its logs. Existing tests still run; new behavior is not
required to pass yet.

### TLS-02: Add a failing test for first-run self-signed Secret creation

**Depends on:** TLS-01

**Files:**

- `scripts/test-deploy-prod-stack.sh`

**Work:**

1. Add a Flux-mode test where the TLS Secret is absent.
2. Assert OpenSSL is called for an RSA 2048-bit, SHA-256, 365-day certificate.
3. Assert the certificate SANs contain the configured `DOMAIN` and
   `MCP_DOMAIN`, not hard-coded defaults.
4. Assert one `kubernetes.io/tls` Secret is applied in `NAMESPACE` using
   `TLS_SECRET_NAME`.
5. Assert certificate generation happens before Flux reconciliation.

**Acceptance:** The new scenario fails against the current deploy script for
the expected reason: no self-signed Secret is created.

### TLS-03: Extract reusable TLS Secret validation in the deploy script

**Depends on:** TLS-02

**Files:**

- `deploy/deploy-prod.sh`

**Work:**

1. Move the existing direct-mode certificate extraction and SAN checks into a
   shell function used by both deployment modes.
2. Check that the Secret type is `kubernetes.io/tls` and that `tls.crt` and
   `tls.key` exist.
3. Parse the certificate with OpenSSL.
4. Check both configured domain names with `openssl x509 -checkhost` rather
   than parsing human-readable certificate text.
5. Compare the public key derived from `tls.crt` with the public key derived
   from `tls.key`.
6. Use `openssl x509 -checkend 2592000` to require at least 30 days of remaining
   validity.
7. Return a nonzero status and a specific error message for each failed check.

**Acceptance:** Existing valid-secret coverage passes. Add focused assertions
for wrong Secret type, missing key, malformed certificate, SAN mismatch,
key/certificate mismatch, and expiry within 30 days.

### TLS-04: Implement idempotent self-signed Secret creation

**Depends on:** TLS-03

**Files:**

- `deploy/deploy-prod.sh`

**Work:**

1. Require `openssl` in both Flux and direct Helm modes.
2. Ensure `NAMESPACE` exists before inspecting or creating the TLS Secret.
3. Add a function that creates a temporary OpenSSL configuration containing
   SAN entries for `DOMAIN` and `MCP_DOMAIN`.
4. Generate a temporary RSA 2048-bit key and self-signed SHA-256 certificate
   valid for 365 days. Set the certificate subject CN to `DOMAIN`; SANs are the
   identity source of truth.
5. Pipe `kubectl create secret tls ... --dry-run=client -o yaml` into the
   script's existing `kubectl apply` path.
6. Delete temporary key, certificate, and configuration files through the
   existing trap pattern on success and failure.
7. If the Secret already exists, validate and reuse it. Do not regenerate it.
8. If it exists but is invalid or near expiry, abort and print exact manual
   rotation steps; do not overwrite it.
9. In `DRY_RUN=1`, generate temporary material and server-dry-run the Secret,
   but do not persist it.

**Acceptance:** TLS-02 passes. Two consecutive runs create the Secret only on
the first run. No certificate or private-key bytes appear in stdout/stderr or
fixture logs.

### TLS-05: Add certificate reuse and failure-path tests

**Depends on:** TLS-04

**Files:**

- `scripts/test-deploy-prod-stack.sh`

**Work:**

1. Add a valid-existing-Secret case and assert there is no generation or apply.
2. Add an existing Secret with the wrong SAN and assert deployment stops before
   secrets, Helm, or Flux are mutated.
3. Add an existing Secret with a mismatched private key and assert the same.
4. Add an existing certificate expiring within 30 days and assert the same.
5. Add a dry-run case and assert no TLS Secret state is persisted.
6. Assert temporary TLS files are absent after each scenario.

**Acceptance:** All new cases pass and prove fail-closed, non-overwriting
behavior.

### TLS-06: Remove cert-manager bootstrap from Flux deployment

**Depends on:** TLS-05

**Files:**

- `deploy/deploy-prod.sh`

**Work:**

1. Remove `CERT_MANAGER_ACME_EMAIL`, `CERT_MANAGER_VERSION`, and the
   cert-manager chart constants.
2. Remove Kubernetes-version compatibility checks used only by cert-manager.
3. Remove CRD and `letsencrypt-prod` ClusterIssuer discovery, installation,
   creation, and readiness logic.
4. Remove the post-reconcile waits and DNS-name inspection for the cert-manager
   Certificate resource.
5. After Flux reconciliation, validate the shared TLS Secret with the function
   from TLS-03.
6. Preserve checks that the core and MCP HelmReleases reference the same Secret.

**Acceptance:** `rg -n -i 'letsencrypt|cert-manager|acme' deploy/deploy-prod.sh`
returns no matches. Flux and direct Helm test scenarios pass.

### TLS-07: Replace cert-manager tests with self-signed-origin tests

**Depends on:** TLS-06

**Files:**

- `scripts/test-deploy-prod-stack.sh`

**Work:**

1. Delete cert-manager CRD, ClusterIssuer, Helm-install, ACME-email, Kubernetes
   version, and Certificate readiness fixture branches.
2. Delete tests whose only purpose was cert-manager bootstrap behavior.
3. Keep and adapt deployment ordering, shared-secret, ownership, dry-run, and
   fail-closed tests.
4. Rename variables and test messages so they describe origin TLS Secrets, not
   cert-manager Certificates.

**Acceptance:** The test file contains no Let's Encrypt, ACME, ClusterIssuer,
or cert-manager expectations. `bash scripts/test-deploy-prod-stack.sh` passes.

### TLS-08: Remove the Flux ingress-shim annotation

**Depends on:** TLS-06

**Files:**

- `deploy/flux/overlays/production/charts/core-helmrelease.yaml`

**Work:**

1. Remove only `cert-manager.io/cluster-issuer: letsencrypt-prod`.
2. Keep `traefik.ingress.kubernetes.io/router.entrypoints: websecure`.
3. Keep the `commoncal-tls` entry and both DNS names under `ingress.tls`.
4. Do not add the annotation to the MCP HelmRelease.

**Acceptance:** Rendered core and MCP Ingresses both reference
`commoncal-tls`; neither rendered Ingress has a cert-manager annotation.

### TLS-09: Add static deployment validation for the new contract

**Depends on:** TLS-08

**Files:**

- `scripts/validate-deploy.sh`
- `deploy/helm/commoncal/tests/template_assertions.sh`
- `deploy/helm/commoncal-mcp/tests/template_assertions.sh`

**Work:**

1. Assert both production Ingresses render TLS with `commoncal-tls`.
2. Assert both production Ingresses use `websecure`.
3. Fail validation if production-owned files contain a cert-manager issuer
   annotation.
4. Add `scripts/test-deploy-prod-stack.sh` to `scripts/validate-deploy.sh` if it
   is not already invoked indirectly.
5. Do not scan vendored Flux CRDs for generic certificate wording; scope the
   forbidden check to this repository's app deployment files.

**Acceptance:** `bash scripts/validate-deploy.sh` passes, then fails when a
temporary cert-manager annotation is deliberately inserted into a rendered
test fixture.

### TLS-10: Update deployment documentation

**Depends on:** TLS-09

**Files:**

- `docs/DEPLOYMENT.md`

**Work:**

1. Replace cert-manager and Let's Encrypt setup text with the two-hop TLS model.
2. State that Cloudflare must proxy both DNS records and use SSL/TLS mode
   `Full`, not `Flexible` or `Full (strict)`.
3. Explain first-run generation, reuse, 30-day expiry guard, and manual
   rotation.
4. Document that `commoncal-tls` is self-signed and used only on the origin
   hop; browsers receive Cloudflare Universal SSL.
5. Remove `CERT_MANAGER_ACME_EMAIL` and `CERT_MANAGER_VERSION` configuration.
6. Add verification commands for the Cloudflare edge certificate and the
   origin certificate using SNI and the origin IP.
7. Add rollback instructions: restore the previous trusted Secret before
   switching Cloudflare back to `Full (strict)`.

**Acceptance:** A repository search finds no active deployment instruction
that asks operators to install cert-manager or obtain a Let's Encrypt cert.

### TLS-11: Rewrite the portable TLS blueprint

**Depends on:** TLS-10

**Files:**

- `docs/TLS-BLUEPRINT.md`

**Work:**

1. Change the recommended origin path from Let's Encrypt to a self-signed
   certificate generated by the service's deployment script.
2. Preserve the explanation that Cloudflare Universal SSL provides public TLS.
3. Clearly state the required `Full` mode and why `Full (strict)` is invalid for
   this design.
4. Replace Certificate/ClusterIssuer examples with an OpenSSL SAN example and
   a `kubectl create secret tls --dry-run=client | kubectl apply` example.
5. Replace cert-manager verification with Secret inspection and direct-origin
   OpenSSL checks.
6. Retain the warning that the edge wildcard does not cover multi-level labels.

**Acceptance:** The blueprint has no operational dependency on Let's Encrypt,
ACME, or cert-manager and does not claim that Cloudflare validates the
self-signed origin.

### TLS-12: Add an operator cutover checklist

**Depends on:** TLS-11

**Files:**

- `docs/DEPLOYMENT.md`

**Work:**

1. Add this exact order: confirm proxied DNS, set Cloudflare mode to `Full`,
   deploy, verify both edge endpoints, verify direct-origin SNI, inspect logs.
2. State that changing to `Full (strict)` before replacing the self-signed
   certificate causes Cloudflare error 526.
3. State that `Flexible` must not be used because the applications and OAuth
   URLs require HTTPS end to end.
4. Include a rollback sequence that restores the old TLS Secret and previous
   Cloudflare mode.
5. Mark deletion of cluster cert-manager resources as optional and require an
   inventory of other Certificate resources first.

**Acceptance:** An operator can execute or roll back the cutover without
reading the implementation diff.

### TLS-13: Run the complete verification gate

**Depends on:** TLS-12

**Files:** none unless a test exposes a defect

**Work:**

1. Run `bash scripts/test-deploy-prod-stack.sh`.
2. Run `bash scripts/test-deploy-prod.sh`.
3. Run both chart `tests/template_assertions.sh` scripts.
4. Run `bash scripts/validate-deploy.sh`.
5. Run `rg -n -i 'letsencrypt|cert-manager|acme' deploy scripts docs`.
6. Classify remaining search hits as historical explanation, vendored Flux
   schema text, or a defect. Fix defects only.
7. Inspect `git diff --check` and `git status --short`.

**Acceptance:** All tests pass; production app manifests and deploy scripts
have no Let's Encrypt/cert-manager dependency; only intended files changed.

## Manual Production Acceptance

These checks require Cloudflare and cluster access and must not be simulated by
unit tests:

1. Both DNS records are proxied in Cloudflare.
2. Zone encryption mode is `Full`.
3. `kubectl get secret commoncal-tls -n commoncal` reports type
   `kubernetes.io/tls`.
4. Direct-origin TLS with SNI succeeds and presents a self-signed certificate
   containing both production SANs.
5. `https://cal.hajnal.space` and `https://mcal.hajnal.space/mcp` present a
   publicly trusted Cloudflare certificate.
6. Core login/session behavior and MCP OAuth discovery still work through the
   Cloudflare endpoints.

## Security Follow-Ups (Separate Scope)

- Restrict public origin ingress to Cloudflare's published IP ranges so clients
  cannot bypass Cloudflare and accept the self-signed endpoint directly.
- Automate certificate rotation before the 30-day guard. The initial change is
  deliberately fail-closed and operator-driven.
- If validated origin identity becomes a requirement, migrate from self-signed
  certificates to Cloudflare Origin CA and change the zone to `Full (strict)`.

## Out of Scope

- Installing or configuring a Cloudflare plugin or API client.
- Storing Cloudflare credentials in Kubernetes or Git.
- Creating Cloudflare DNS records automatically.
- Deleting cert-manager or its CRDs from the cluster.
- Changing application-level HTTPS URLs, OAuth issuer URLs, or HSTS behavior.
- Supporting direct public access to the origin as a trusted browser endpoint.
