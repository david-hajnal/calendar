# Security threat model and ASVS 5.0 Level 2 release checklist

This is an implementation-evidence model, not an assertion of production
certification. “Implemented” below means the cited code and (where cited) test
exist in this repository; deployment configuration and operating procedures
remain the operator’s responsibility.

## Scope, assets, actors, and boundaries

| Item | Evidence and security relevance |
| --- | --- |
| Calendar, event, recurrence, ACL, view, and publication data | SQLite migrations in [`backend/migrations`](../backend/migrations); authenticated APIs are registered in [`backend/src/http.rs`](../backend/src/http.rs#L464). Public projections are a distinct unauthenticated boundary. |
| Identity, invitation and login-link secrets, sessions, CSRF tokens | 32-byte random opaque tokens are HMAC-hashed with a domain separator in [`backend/src/security.rs`](../backend/src/security.rs#L13); session records are read and state-checked in [`backend/src/sessions.rs`](../backend/src/sessions.rs#L85). |
| Feed URLs and imported ICS data | The original URL is encrypted at rest and the displayed URL is redacted by [`ExternalFeedService::create`](../backend/src/external_feed.rs#L181). ICS content is untrusted remote input. |
| Notification preferences, jobs, in-app records, and delivery email address | Persistence and delivery processing are in [`backend/src/notification.rs`](../backend/src/notification.rs#L121). |
| SQLite database, backup artifacts, encryption key, and backup metadata | Snapshot, hashing, compression, optional encryption, and restore are in [`backend/src/backup.rs`](../backend/src/backup.rs#L99). Backup artifacts and keys are separate high-value assets. |
| Audit records and application logs | Administrative and login actions append audit records in [`backend/src/admin.rs`](../backend/src/admin.rs#L537) and [`backend/src/login.rs`](../backend/src/login.rs#L528); request tracing redacts public-link paths in [`backend/src/http.rs`](../backend/src/http.rs#L625). |

Actors are an unauthenticated Internet client, a public-link recipient,
an active calendar user (owner/manager/editor/viewer/free-busy viewer), a
platform superadmin, a compromised browser/session, an ICS host/DNS responder,
an email recipient/provider, a Kubernetes ingress/controller, and an operator
with pod/PVC/backup/secret access. The application, SQLite/PVC, remote ICS
service, email service, ingress, and remote backup storage are separate trust
boundaries. In particular, a superadmin is not automatically trusted for a
calendar: authorization requires a calendar role ([`docs/authorization.md`](authorization.md)).

## Entry points and data flows

* The HTTP surface is assembled in [`backend/src/http.rs`](../backend/src/http.rs#L446): health endpoints; unauthenticated invitation/login-link consumption and login-link request; public view metadata/events; then session-protected admin, calendar, event, feed, view, and notification endpoints. Requests have a 1 MiB body limit ([`backend/src/http.rs`](../backend/src/http.rs#L58)).
* Invitation or passwordless login token -> token hash/state validation -> active user -> session row -> `__Host-commoncal_session` cookie and a session-bound CSRF token. Login flow writes audit records ([`backend/src/login.rs`](../backend/src/login.rs#L290)).
* Browser cookie -> session lookup/status/revocation/absolute and idle expiry checks -> CSRF/origin/fetch-metadata validation for unsafe methods -> handler ([`backend/src/sessions.rs`](../backend/src/sessions.rs#L85), [`backend/src/http.rs`](../backend/src/http.rs#L1711)).
* Authenticated actor -> central calendar decision -> SQLite reads/writes. Calendar roles and deny-by-default semantics are documented in [`docs/authorization.md`](authorization.md); authorization regression coverage is in [`backend/tests/authorization_regression.rs`](../backend/tests/authorization_regression.rs).
* Public-link token -> public-view service -> metadata/events projection. Public endpoints remove `Set-Cookie`, are marked non-indexable/no-store, and public token paths are redacted from tracing ([`backend/src/http.rs`](../backend/src/http.rs#L1191), [`backend/src/http.rs`](../backend/src/http.rs#L1233), [`backend/src/http.rs`](../backend/src/http.rs#L625)). The token remains a bearer secret in the URL and can still be exposed by a recipient or infrastructure logs not governed by this code.
* Authorized feed manager -> encrypted source URL -> safe outbound HTTPS client -> DNS validation and pinned validated addresses -> bounded/decompressed ICS body -> parser/import -> SQLite. Every redirect repeats URL/DNS validation ([`backend/src/ics_http.rs`](../backend/src/ics_http.rs#L194)); coverage includes private addresses, redirects, rebinding/pinning, limits, and log redaction ([`backend/tests/ics_http.rs`](../backend/tests/ics_http.rs#L24)).
* Event/preferences -> notification jobs -> in-app record and development email sender. The worker runs every 30 seconds in-process ([`backend/src/main.rs`](../backend/src/main.rs#L93)).
* SQLite -> `VACUUM INTO` snapshot -> integrity check -> gzip/hash/metadata -> optionally AES-256-GCM artifact and uploader -> operator-controlled restore. The shipped CLI currently invokes unencrypted `create`, not `create_encrypted_and_upload` ([`backend/src/main.rs`](../backend/src/main.rs#L175), [`backend/src/backup.rs`](../backend/src/backup.rs#L214)).

## Existing controls and misuse cases

| Misuse case | Existing control | Residual risk / status |
| --- | --- | --- |
| Replay or substitution of invitation, login, session, or public token | Random 32-byte tokens, HMAC hash-at-rest, domain separation, expiry/consumption/revocation state ([`backend/src/security.rs`](../backend/src/security.rs#L13), [`backend/tests/security.rs`](../backend/tests/security.rs#L14)). | Bearer tokens still require recipients and proxies not to disclose them. |
| CSRF or cross-site unsafe request | Protected unsafe requests require matching `Origin`, same-origin/site Fetch Metadata, and session-bound CSRF token ([`backend/src/sessions.rs`](../backend/src/sessions.rs#L142)). Secure/HttpOnly/SameSite=Lax host cookie is built in [`backend/src/security.rs`](../backend/src/security.rs#L260). | Cookies require HTTPS deployment; development may use HTTP. |
| IDOR or privilege escalation through identifiers | Authenticated middleware plus centralized deny-by-default roles; inaccessible calendar resources use a common 404 response ([`docs/authorization.md`](authorization.md)). | Keep authorization regression tests required in CI. |
| Public recipient obtains private details or a session cookie | Dedicated public handlers/projections; public responses strip cookies and set `noindex, nofollow` and `no-store` ([`backend/src/http.rs`](../backend/src/http.rs#L455), [`backend/src/http.rs`](../backend/src/http.rs#L1233)). | Publication policy/redaction correctness relies on shared-view implementation/tests; a leaked URL remains valid until revoked/rotated. |
| SSRF, DNS rebinding, redirect-to-private-network, compressed-body exhaustion via ICS | HTTPS-only feed creation; credentials rejected; destination resolution rejects non-public results; connection uses validated addresses; redirect/byte/time limits ([`backend/src/external_feed.rs`](../backend/src/external_feed.rs#L181), [`backend/src/ics_http.rs`](../backend/src/ics_http.rs#L194)). | Network policy has no egress policy, so application-layer validation is the demonstrated boundary. |
| Notification disclosure or duplicate delivery | Jobs are claimed/stateful and in-app insertion is conflict-safe ([`backend/src/notification.rs`](../backend/src/notification.rs#L131), [`backend/src/notification.rs`](../backend/src/notification.rs#L262)). | Production currently wires `DevelopmentEmailSender` ([`backend/src/main.rs`](../backend/src/main.rs#L70)); no production email-provider boundary is evidenced. |
| SQLite/PVC theft, corrupt restore, or remote-upload failure | Encrypted backup primitive uses AES-256-GCM; restore checks integrity before replacement; failed upload retains local encrypted artifact ([`backend/src/backup.rs`](../backend/src/backup.rs#L79), [`backend/tests/backup.rs`](../backend/tests/backup.rs#L114)). | Default backup CLI is plaintext gzip; PVC/database-at-rest encryption, key custody, retention, and remote uploader are not evidenced. |
| Ingress compromise or plaintext external traffic | Pod hardening, ClusterIP service, optional NetworkPolicy, configurable ingress/TLS are in [`deploy/helm/commoncal/values.yaml`](../deploy/helm/commoncal/values.yaml) and [`deploy/helm/commoncal/templates/ingress.yaml`](../deploy/helm/commoncal/templates/ingress.yaml). HSTS is enabled only when production `APP_ORIGIN` is HTTPS ([`backend/src/main.rs`](../backend/src/main.rs#L147)). | TLS is not required by chart defaults (`ingress.tls: []`); NetworkPolicy only specifies ingress ([`deploy/helm/commoncal/templates/networkpolicy.yaml`](../deploy/helm/commoncal/templates/networkpolicy.yaml)). |
| Malicious or mistaken administrator action | Superadmin-only routes, final-active-superadmin protection, session revocation, and audit logging ([`docs/authorization.md`](authorization.md), [`backend/src/admin.rs`](../backend/src/admin.rs#L378)). | No evidence of MFA, approval workflow, audit-log tamper protection/export, or operational access review. |

## OWASP ASVS 5.0.0 Level 2 evidence map

**Mapping limitation.** The repository contains the Level 2 target
([`docs/plan.md`](plan.md#L380)) and the identifiers below, but does not contain
the normative ASVS 5.0.0 requirement text or an approved crosswalk. The
descriptions are therefore control-domain labels, not independently verified
ASVS clause wording. “Verified” means the cited implementation and regression
test were inspected locally; it does **not** assert ASVS certification. Ranges
and chapter-only labels cannot be verified clause-by-clause from this
repository and are marked **partial** or **gap** accordingly.

| ASVS identifier / applicable control domain | Local evidence | Status and rationale |
| --- | --- | --- |
| `v5.0.0-V1.2.4` (database-query handling) | Bound values are used for the inspected session query in [`backend/src/sessions.rs`](../backend/src/sessions.rs#L96). | **Partial:** this spot check cannot establish that every repository query is parameterized. |
| `v5.0.0-V1.3.6` (outbound-request / SSRF controls) | Feed creation accepts HTTPS URLs in [`backend/src/external_feed.rs`](../backend/src/external_feed.rs#L195); the HTTP client validates destinations and redirects in [`backend/src/ics_http.rs`](../backend/src/ics_http.rs#L251); tests reject private destinations, redirects, and rebinding in [`backend/tests/ics_http.rs`](../backend/tests/ics_http.rs#L33). | **Partial:** application controls are verified, but no egress firewall or destination domain/port allowlist is evidenced. |
| `v5.0.0-V2.3.3` (transaction integrity) | Login-link consumption starts an immediate transaction in [`backend/src/login.rs`](../backend/src/login.rs#L294); administrative mutations do so in [`backend/src/admin.rs`](../backend/src/admin.rs#L176); feed refresh does so in [`backend/src/external_feed.rs`](../backend/src/external_feed.rs#L309). | **Partial:** representative flows were verified; no transaction inventory or per-business-flow test evidence establishes complete coverage. |
| `v5.0.0-V2.4.1` (anti-automation) | Production wiring uses `FixedWindowLoginRateLimiter` in [`backend/src/main.rs`](../backend/src/main.rs#L78); both IP and normalized-email keys are tested in [`backend/tests/passwordless_login.rs`](../backend/tests/passwordless_login.rs#L365). | **Partial:** verified only for login-link requests; no general limits for other public or costly operations are evidenced. |
| `v5.0.0-V3.3.1`–`V3.3.4` (session-cookie attributes) | The session cookie has `__Host-`, `Path=/`, `Secure`, `HttpOnly`, and `SameSite=Lax` in [`backend/src/security.rs`](../backend/src/security.rs#L265), with attribute tests in [`backend/tests/security.rs`](../backend/tests/security.rs#L81). | **Verified:** implementation and test support these attributes; HTTPS deployment is still required for the Secure attribute to operate. |
| `v5.0.0-V3.4.1`, `V3.4.3`–`V3.4.6` (browser response headers) | CSP, nosniff, referrer, permissions, cache, and conditional HSTS headers are set in [`backend/src/http.rs`](../backend/src/http.rs#L1239) and tested in [`backend/tests/application.rs`](../backend/tests/application.rs#L154). | **Partial:** HSTS is deliberately emitted only for production HTTPS configuration ([`backend/tests/application.rs`](../backend/tests/application.rs#L225)); its production deployment is not verified here. |
| `v5.0.0-V3.5.1`, `V3.5.3` (CSRF / cross-origin unsafe requests) | Unsafe requests require Origin, Fetch Metadata, and a session-bound CSRF token in [`backend/src/sessions.rs`](../backend/src/sessions.rs#L169); missing, mismatched, and cross-site cases are tested in [`backend/tests/session_middleware.rs`](../backend/tests/session_middleware.rs#L201). | **Verified:** the inspected authenticated middleware and tests enforce all three checks. |
| `v5.0.0-V6.3.1`, `V6.3.3`, `V6.5.1`–`V6.5.5`, `V6.6.2`–`V6.6.3` (passwordless authentication and abuse resistance) | Tokens are random and domain-separated in [`backend/src/security.rs`](../backend/src/security.rs#L13); login-link request/consumption state handling is in [`backend/src/login.rs`](../backend/src/login.rs#L207) and [`backend/src/login.rs`](../backend/src/login.rs#L294). | **Partial:** token and request-rate controls exist, but the repository has no normative ASVS crosswalk or evidence covering factor-strength decisions, email-channel assumptions, or an anti-automation policy beyond this endpoint. |
| `v5.0.0-V7.2.1`–`V7.2.4`, `V7.3.1`–`V7.3.2`, `V7.4.1`–`V7.4.2`, `V7.4.5` (session lifecycle) | Session lookup checks revocation, user status, absolute expiry, and idle timeout in [`backend/src/sessions.rs`](../backend/src/sessions.rs#L85); login creates a new session in [`backend/src/login.rs`](../backend/src/login.rs#L349); middleware coverage is in [`backend/tests/session_middleware.rs`](../backend/tests/session_middleware.rs#L201). | **Partial:** important controls are evidenced, but concurrent-session policy and timeout rationale are not documented and the range cannot be certified clause-by-clause. |
| `v5.0.0-V8.1.1`–`V8.1.2`, `V8.2.1`–`V8.2.3`, `V8.3.1` (authorization) | Authorization rules are documented in [`docs/authorization.md`](authorization.md); the regression matrix covers endpoint families in [`backend/tests/authorization_regression.rs`](../backend/tests/authorization_regression.rs#L194). | **Partial:** documented and tested coverage exists, but only a complete route/projection inventory linked to requirements could support a full range-level claim. |
| `v5.0.0-V11` (cryptography; chapter-only identifier) | Token hashing is implemented in [`backend/src/security.rs`](../backend/src/security.rs#L79); encrypted backup creation is implemented and tested in [`backend/src/backup.rs`](../backend/src/backup.rs#L214) and [`backend/tests/backup.rs`](../backend/tests/backup.rs#L140). | **Gap:** chapter-only mapping is not a verifiable requirement, the normal backup CLI remains plaintext ([`backend/src/main.rs`](../backend/src/main.rs#L175)), and key lifecycle/rotation evidence is absent. |
| `v5.0.0-V12` (secure communications; chapter-only identifier) | ICS fetch requires HTTPS in [`backend/src/external_feed.rs`](../backend/src/external_feed.rs#L195); Helm can configure TLS in [`deploy/helm/commoncal/templates/ingress.yaml`](../deploy/helm/commoncal/templates/ingress.yaml). | **Gap:** chart defaults leave `ingress.tls` empty ([`deploy/helm/commoncal/values.yaml`](../deploy/helm/commoncal/values.yaml#L49)); production TLS enforcement is not evidenced. |
| `v5.0.0-V13` (configuration; chapter-only identifier) | Production configuration rejects a missing session secret in [`backend/src/config.rs`](../backend/src/config.rs#L116); the chart configures pod hardening in [`deploy/helm/commoncal/values.yaml`](../deploy/helm/commoncal/values.yaml#L21). | **Gap:** no evidence covers secret rotation, outbound egress control, or production email configuration. |
| `v5.0.0-V14`; `v5.0.0-V16` (data protection; logging/error handling; chapter-only identifiers) | Feed source URLs are stored encrypted in [`backend/src/external_feed.rs`](../backend/src/external_feed.rs#L214); public response headers are handled in [`backend/src/http.rs`](../backend/src/http.rs#L1233); encrypted restore is tested in [`backend/tests/backup.rs`](../backend/tests/backup.rs#L179). | **Gap:** SQLite/PVC encryption, retention/deletion, audit-log tamper protection, and centralized log access/retention are not evidenced. |

## Release-blocking checklist

- The items marked **Block** are unresolved high-risk release gates. The final
  item is an evidence gate; remaining ASVS gaps that are not marked **Block**
  need recorded, authorized risk acceptance before release and must not be
  reported as complete.

- [ ] **Block release:** make production backups encrypted by default (or remove the plaintext backup command), define encryption-key custody/rotation and backup retention, and run a clean restore drill. The current CLI writes `.sqlite.gz` ([`backend/src/main.rs`](../backend/src/main.rs#L175)).
- [ ] **Block Internet release:** require ingress TLS for production, set HTTPS `APP_ORIGIN`, and verify HSTS; chart defaults do not require TLS ([`deploy/helm/commoncal/values.yaml`](../deploy/helm/commoncal/values.yaml)).
- [ ] **Block notification-enabled release:** replace or explicitly disable `DevelopmentEmailSender`, document provider authentication/egress and recipient-data handling ([`backend/src/main.rs`](../backend/src/main.rs#L70)).
- [ ] **Block if external feeds are enabled in a sensitive network:** establish egress NetworkPolicy/firewall/DNS controls consistent with the ICS allowlist; current chart policy is ingress-only.
- [ ] **Block release:** confirm the production session secret exists, is high entropy, is not logged, and has a tested rotation/revocation procedure; configuration only enforces presence ([`backend/src/config.rs`](../backend/src/config.rs#L72)).
- [ ] **Block release:** select and verify encryption at rest for the SQLite PVC and backup destination, including operator/PVC access controls. The repository does not evidence either control.
- [ ] **Block release:** run and retain evidence for the cited session, authorization, public-view, ICS, backup, notification, and Helm tests before release.
