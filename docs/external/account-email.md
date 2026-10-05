# Account email delivery

Invitations, login links, password resets, email confirmations, old-address
notices, and calendar notifications share the application's mail transport.
The Rust CommonCal app owns this transport. The separate Node `commoncal-auth`
OIDC/bridge service does not consume these mail settings or credentials.

## Production prerequisites

Configure the following runtime variables before starting the server:

| Variable | Purpose |
| --- | --- |
| `APP_ENV` | `production` |
| `APP_ORIGIN` | Public HTTPS application origin used in links, without trailing slash |
| `CALDAV_PUBLIC_ORIGIN` | Public HTTPS CalDAV origin, normally the same origin |
| `SESSION_SECRET` | Existing application session secret |
| `MCP_INTERNAL_API_KEY` | Existing key protecting the application's internal MCP boundary |
| `PASSWORD_LOGIN_ENABLED` | `true`, required for invited users to sign in |
| `SMTP_HOST` | Provider hostname or IP, without URL scheme or port |
| `SMTP_PORT` | Provider port, normally `587` or `465`; must be 1–65535 |
| `SMTP_USERNAME` | SMTP authentication username |
| `SMTP_PASSWORD` | SMTP authentication password, supplied as a secret |
| `SMTP_FROM` | Provider-approved sender mailbox, optionally `CommonCal <address>` |

Port 465 uses implicit TLS. All other ports require STARTTLS. TLS certificates
are verified against normal trust roots; there is no plaintext, opportunistic
TLS, certificate bypass, or unauthenticated production option. Sends have a
30-second total limit and a 10-second SMTP command limit. Provider errors and
credential Debug output are redacted; message tokens/bodies are not logged.
The underlying [lettre TLS modes](https://docs.rs/lettre/0.11.19/lettre/transport/smtp/client/enum.Tls.html)
define required STARTTLS and implicit TLS behavior.

The server validates mail configuration before database migrations or serving
requests. Missing or malformed settings abort startup; production never falls
back to captured mail. This validates configuration, not provider connectivity
or inbox delivery. A provider outage can leave a ready application unable to
send mail: admin invitation failures expose retry/resend, while public recovery
requests retain their generic response to protect account privacy.

Bootstrap, backup, restore, and seed commands do not initialize SMTP. They still
require the applicable application settings, including production password
login. Bootstrap prints its invitation link for the operator; it does not send
an email by itself.

## Helm

Create a separately managed `commoncal-mail` Secret in the app namespace with
`SMTP_USERNAME` and `SMTP_PASSWORD` keys through your normal secret-management
process. Never put credential values in Helm values, ConfigMaps, or Git.
Configure non-secret values in an operator values file:

```yaml
config:
  passwordLoginEnabled: true
mail:
  host: smtp.your-provider.example
  port: 587
  from: no-reply@your-verified-domain.example
  existingSecret:
    name: commoncal-mail
    usernameKey: SMTP_USERNAME
    passwordKey: SMTP_PASSWORD
  egressCIDRs:
    - 0.0.0.0/0
```

Use your actual verified domain/provider. Chart defaults and standalone
production values deliberately leave `mail.host` and `mail.from` empty;
rendering fails until they are supplied. The chart references credentials only
in the app StatefulSet. The backup CronJob receives the ConfigMap and its own
existing backup/session secrets, without SMTP credentials. The ConfigMap
checksum triggers a rollout when non-secret settings change. Secret rotation
requires a normal application rollout/restart to load the new credentials.

The NetworkPolicy allows TCP egress on exactly `mail.port`. Narrow
`mail.egressCIDRs` to stable provider ranges where available, add IPv6 ranges
if needed, or specify the private relay's CIDR. DNS egress remains available.
No SMTP ingress is required.

For direct Helm deployment, `deploy/deploy-prod.sh` accepts `SMTP_HOST`,
`SMTP_PORT` (default 587), and `SMTP_FROM` from the operator environment or
`deploy/.env`. Missing host/sender or invalid port fails before Kubernetes
mutation. The mail Secret must already exist; the script does not create it.
For Flux, add the non-secret `mail` values to the production core HelmRelease
in Git and create the Secret separately before reconciling the new chart.
The script's SMTP environment values do not override Flux-managed values.
The existing production HelmRelease requires this operator configuration
before it can render the upgraded chart; no provider settings are invented.

## Development and verification

`APP_ENV=development` always uses the captured outbox and ignores SMTP variables,
even if present. The development Compose override sets this environment
explicitly. The base Compose file passes `.env.local` through; production
Compose requires all production variables above in that file. Use
`scripts/dev.sh` for local Docker operations.

Controlled transport tests use a loopback SMTP server with a fixture CA and
published test-only key. They verify all six message types, authenticated TLS,
sender/links, missing STARTTLS, untrusted certificates, and redacted rejection
errors. No real provider credentials or live inboxes are used.

```sh
cargo test --manifest-path backend/Cargo.toml --locked --lib smtp::tests
cargo test --manifest-path backend/Cargo.toml --locked --test mail_configuration
sh deploy/helm/commoncal/tests/mail_assertions.sh
sh deploy/helm/commoncal/tests/template_assertions.sh
```

Real inbox delivery still requires production SMTP configuration, verified
sender/domain setup, allowed relay egress, and an operator delivery check.
No live send or deployment is part of these implementation tests.
