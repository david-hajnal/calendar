# Account invitations and management

## Admin Settings → Users

Enter an email and choose **Send invitation**. The recipient opens the email,
creates and confirms a password, then signs in. Opening the invitation alone
does not register the account or create a session. Passwords require at least
12 characters and at most 72 UTF-8 bytes.

| Status | Meaning and available actions |
| --- | --- |
| invited | Password setup is outstanding. Resend invitation, change email, or disable. |
| registered | Account can sign in. Request a confirmed email change or disable. |
| pending | Awaiting approval. Email changes/resends are unavailable; admins can disable. |
| inactive | Access is disabled. Account data is retained; resend/email-change actions are unavailable. |

**Resend invitation** replaces the previous link, including expired or failed
invitations, and preserves the same account. Invitations expire after 24 hours.
Delivery failure is shown as a failure; the invited row remains available for
retry. A duplicate or reserved email cannot create another account.

**Disable user** requires confirmation naming the address. It ends browser
sessions and revokes outstanding account links, MCP grants, and calendar
connection passwords. Admins cannot disable themselves or remove the final
registered admin. This screen does not provide reactivation or pending approval.

**Change email** on an invited user sends a replacement invitation to the new
address and invalidates old links. For registered users it sends confirmation
to the new address; their existing email remains active until confirmation.
Admin changes preserve the account ID and calendars. Pending confirmation is
shown in Users. If replacement delivery fails after an invited address changes,
use Resend on its new address.

## Password recovery

Choose **Password → Forgot password?** on the sign-in page. The response is the
same for known, unknown, and ineligible accounts. Eligible registered accounts
receive a single-use link expiring after 15 minutes. A new request replaces the
previous link. Set and confirm the new password, then sign in explicitly.
Resetting ends older browser sessions and login links while preserving separate
MCP and calendar connection credentials.

Existing registered users can still choose **Email link** to sign in. Invitations
require password setup even though email-link login remains available afterward.

## Personal Settings → Account

Enter a new email and the current password, then choose **Send confirmation**.
Passwordless accounts can establish a password through recovery first. The old
address stays active until the new address is explicitly confirmed. Confirmation
links expire after 24 hours; sending another replaces the previous request.

Confirmation retains the same account and calendars, ends browser sessions and
older account links, and sends a notice to the old address. Sign in using the
new email afterward. Calendar clients using email as their username need the
new address; their connection password remains valid. MCP grants remain valid.

## Deployment and verification

Production requires authenticated TLS SMTP and enabled password login. Follow
[account email setup](external/account-email.md) before rollout. Development
captures mail locally; implementation verification never sends live email.

The account browser suite starts a separate temporary backend/database/outbox
for each journey. Rate limits remain enabled; scenarios do not share buckets.
Bootstrap verification prepares an empty-user fixture, matching the backend
tests by removing migration 0019's seeded `admin@localhost` account only from
that disposable database. The real bootstrap command still requires an empty
user table. Both invitation URL forms (`/invitations/accept` and the legacy
`/invitations/consume`) require explicit password creation.

```sh
cargo build --manifest-path backend/Cargo.toml --locked
pnpm --dir frontend build
E2E_ISOLATED_BACKEND=1 pnpm --dir e2e exec playwright test account-management.spec.ts --workers=1
E2E_ISOLATED_BACKEND=1 pnpm --dir e2e exec playwright test desktop-mvp.spec.ts --project=desktop --workers=1
```

The account suite covers desktop Firefox and mobile WebKit, statuses,
loading/retry, admin authorization/CSRF, invite/resend/disable, recovery,
personal/admin email confirmation, bootstrap/legacy links, URL-token removal,
sensitive response headers, retained calendars/CalDAV credentials, and email-link
login. The calendar journey additionally checks collaboration, public views,
ICS import/external feeds, notifications, and mobile primary views. Backend
tests cover migration preservation, token expiry/reuse/domain isolation,
concurrent transactions/rollback, rate limits, MCP revocation/compatibility, and
controlled TLS SMTP delivery.
