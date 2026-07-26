# Calendar Platform Planning Pack

## Chained output

1. Product lead brief
2. Staff tech lead proposal
3. Senior architect review
4. Ordered implementation prompts for low-reasoning coding agents

---

# 1. Product Lead Brief

## 1.1 Product name

Working name: **CommonCal**

## 1.2 Product vision

Create a centralized, multi-user calendar platform that combines personal calendars, team calendars, externally sourced public calendars, and curated public calendar views in one responsive web application.

The product should provide Google Calendar-like usability without requiring a native mobile application.

## 1.3 Problem statement

Small organizations, families, communities, and internal teams frequently need to:

* Maintain individual and shared calendars.
* Control who may view, edit, or administer each calendar.
* Combine several calendars into a reusable view.
* Publish selected calendar information without exposing the underlying private calendars.
* subscribe to public Google Calendar or iCalendar feeds.
* Deliver reminders according to each user’s preferences.
* Operate the platform from desktop and mobile browsers.

Existing solutions often combine calendar ownership, calendar sharing, and public publishing in ways that are difficult to understand or administer. CommonCal should treat these as separate concepts with explicit permissions.

## 1.4 Product assumptions

The initial version assumes:

* One hosted installation rather than multiple isolated business tenants.
* Email addresses are the primary user identities.
* Authentication is passwordless.
* Google Calendar integration means importing publicly accessible iCalendar/ICS feeds.
* Google OAuth and two-way Google Calendar synchronization are not part of the MVP.
* A public calendar view is read-only.
* Native iOS and Android applications are not required.
* Email and in-application notifications are included in the MVP.
* Browser push notifications may be added later.

## 1.5 User types

### Superadmin

A platform-level administrator who can:

* Invite users.
* Suspend or reactivate accounts.
* Assign or remove other superadmins.
* Inspect platform health, audit events, feed failures, and notification failures.
* Revoke sessions and invitations.

A superadmin does **not** automatically receive permission to read private calendar events. Exceptional access, should it ever be added, must use a separate audited break-glass process.

### Registered user

A normal authenticated user who can:

* Own calendars.
* Participate in shared calendars.
* Create events where permitted.
* Share calendars with other users.
* Create composite calendar views.
* Publish composite views.
* Configure personal notification preferences.
* Subscribe to permitted external ICS feeds.

### Public visitor

An unauthenticated visitor who can:

* Open a published calendar view through its public URL.
* View only the fields and calendars explicitly included by the publisher.
* Navigate permitted date ranges.
* Download an ICS representation only if the publisher enables it.

## 1.6 Calendar permission model

Each calendar has an owner and zero or more ACL entries.

| Role             | Read events | Create events | Edit all events | Manage calendar settings | Manage sharing | Delete calendar |
| ---------------- | ----------: | ------------: | --------------: | -----------------------: | -------------: | --------------: |
| Owner            |         Yes |           Yes |             Yes |                      Yes |            Yes |             Yes |
| Manager          |         Yes |           Yes |             Yes |                      Yes |            Yes |              No |
| Editor           |         Yes |           Yes |             Yes |                       No |             No |              No |
| Viewer           |         Yes |            No |              No |                       No |             No |              No |
| Free/busy viewer |  Times only |            No |              No |                       No |             No |              No |

Rules:

* A calendar must always have exactly one owner.
* Ownership transfer must be explicit.
* An owner cannot remove their own access without first transferring ownership.
* Authorization is evaluated for every API request.
* Unmatched authorization rules deny access.
* Public access is never represented as a normal calendar ACL entry.
* Composite views do not expand a user’s permissions to their source calendars.

## 1.7 Core capabilities

### A. Passwordless user invitation

A superadmin can enter an email address and optionally a display name.

The system:

1. Creates a pending invitation.
2. Sends a single-use, expiring acceptance link.
3. Allows the invited user to activate their account without creating a password.
4. Creates a secure authenticated session.
5. Marks the invitation as consumed.

Invitations can be:

* Resent.
* Revoked.
* Expired.
* Audited.

Existing users must not accidentally receive duplicate accounts.

### B. Passwordless login

Registered users request a login link by entering their email address.

The system sends a short-lived, single-use magic link. Responses must not reveal whether an email address is registered.

Future authentication options may include WebAuthn or external OpenID Connect.

### C. Personal calendars

A user can create one or more calendars.

Calendar fields include:

* Name.
* Description.
* Display color.
* Default timezone.
* Default event visibility.
* Default notification rules.
* Archive status.

The first calendar created during user activation may be designated as the user’s default personal calendar.

### D. Shared calendars

A calendar owner or manager can share a calendar with another registered user.

The sharing interface must:

* Search or select users.
* Assign a calendar role.
* Change an existing role.
* Revoke access.
* Clearly show inherited capabilities.
* Prevent the current owner from removing the last owner.

Sharing changes must be auditable.

### E. Events

The system supports:

* Timed events.
* All-day events.
* Multi-day events.
* Event title.
* Description.
* Location.
* Start and end.
* Event timezone.
* Recurring events.
* Exceptions to recurring events.
* Event reminders.
* Event creator and last editor.
* Optimistic concurrency protection.

The UI must support:

* Quick event creation.
* Full event editing.
* Dragging events.
* Resizing timed events.
* Moving events between writable calendars.
* Recurring-event edit choices such as one occurrence or the series.

### F. Composite calendar views

A registered user can create a saved view containing multiple calendars they are authorized to see.

A view can define:

* Name.
* Description.
* Included calendars.
* Per-calendar display color override.
* Calendar visibility toggles.
* Default display mode.
* Default timezone.
* Initial date range.
* Whether cancelled events are shown.

A view is not itself a calendar and does not contain copied events.

If access to a source calendar is revoked, that calendar automatically disappears from the composite view for that user.

### G. Public calendar views

A composite view owner can publish the view using an unguessable URL.

Public publishing configuration includes:

* Published title.
* Optional public description.
* Included source calendars.
* Detail level:

  * Full details.
  * Title and time only.
  * Free/busy only.
* Default timezone.
* Maximum past and future navigation range.
* Optional ICS download.
* Optional expiration date.
* Link revocation and regeneration.

Public pages must not expose:

* Internal calendar identifiers.
* Internal user identifiers.
* Source calendar names unless explicitly enabled.
* Private descriptions or locations when the selected detail level excludes them.
* Editing operations.
* Authentication cookies.

Public pages should use `noindex` by default.

### H. Public iCalendar/Google Calendar feeds

A user with sufficient calendar permissions can add a public ICS URL as a read-only external calendar source.

The system must:

* Fetch the source safely.
* Parse standard iCalendar events.
* Preserve external event UIDs.
* Support recurring events and recurrence exceptions.
* Use ETag and Last-Modified when available.
* Refresh on a configurable schedule.
* Display last successful synchronization time.
* Display non-sensitive synchronization errors.
* Allow manual refresh subject to rate limits.
* Disable or delete a feed.

Imported events are read-only.

This feature does not include Google OAuth, private Google calendars, or two-way synchronization.

### I. Notifications

Users can configure notification rules at three levels:

1. Account defaults.
2. Calendar defaults.
3. Per-event overrides.

An event-level rule overrides the calendar default, which overrides the account default.

MVP channels:

* In-application notification.
* Email.

Potential future channels:

* Browser push.
* Webhooks.
* SMS through an external provider.

Notification options include:

* At event start.
* A number of minutes, hours, or days before an event.
* Daily agenda.
* Changes to events on selected shared calendars.
* Invitations or calendar access changes.
* External feed synchronization failures for calendar managers.

Each notification must be delivered independently for each user. One user dismissing or changing a reminder must not affect another user.

### J. Responsive calendar UI

Desktop views:

* Month.
* Week.
* Day.
* Agenda/list.

Mobile behavior:

* Responsive navigation.
* Touch-friendly controls.
* Day and agenda views prioritized on narrow screens.
* Month view available with simplified event rendering.
* No horizontal page overflow.
* Touch-based event creation and editing.
* Accessible dialogs and form fields.

The application should remain usable as an installed browser PWA later, but offline operation is not required for the MVP.

### K. Administration

Superadmins can:

* List active, invited, suspended, and deleted users.
* Invite or reinvite users.
* Revoke pending invitations.
* Suspend accounts.
* Revoke active sessions.
* Promote and demote superadmins, subject to retaining at least one.
* View audit records.
* Inspect failed ICS feeds.
* Inspect failed notifications.
* View application version and health information.

## 1.8 Main user stories

### Invitation

As a superadmin, I can invite a user by email so that they can activate an account without creating a password.

### Calendar sharing

As a calendar owner, I can give another user viewer or editor access so that we can collaborate without sharing credentials.

### Composite view

As a user, I can combine my work, family, and external event calendars into one saved view.

### Public publishing

As a view owner, I can publish selected calendars with title-and-time-only visibility without exposing private descriptions.

### ICS subscription

As a calendar manager, I can subscribe to a public Google Calendar ICS address and see updates automatically.

### User-specific reminder

As a user, I can receive an email 30 minutes before an event even when other calendar members use different reminder settings.

## 1.9 Non-functional requirements

### Security

* All production traffic uses HTTPS.
* Authorization is enforced server-side on every protected operation.
* The system follows deny-by-default and least-privilege principles.
* Private identifiers are opaque.
* Sessions can be revoked.
* Sensitive tokens are single-use and stored as hashes.
* State-changing browser requests receive CSRF protection.
* Public URL fetching includes SSRF protection.
* User-controlled content is safely rendered.
* Security-sensitive activity is audited.
* No secrets are stored in source control.
* The application targets OWASP ASVS 5.0 Level 2 verification.

OWASP explicitly recommends deny-by-default authorization and permission validation on every request.

### Performance targets

Initial design targets:

* Authenticated API p95 below 500 ms under expected normal load.
* Initial calendar UI usable within three seconds on a typical mobile connection.
* A calendar range query must be bounded.
* Recurrence expansion must use explicit date windows and occurrence limits.
* Public views must tolerate traffic spikes without exposing private data.

### Reliability

* Database backups run automatically.
* Restore procedures are tested.
* Notification delivery is idempotent.
* External feed failures do not make the main application unavailable.
* Failed background jobs can be retried.
* Deployments do not silently delete persistent data.

### Accessibility

* Keyboard navigation for primary workflows.
* Visible focus state.
* Accessible labels and dialogs.
* Sufficient contrast.
* Calendar information also available through a list view.

### Observability

* Structured application logs.
* Request correlation IDs.
* Metrics for API latency, errors, jobs, feed synchronization, and notification delivery.
* Health and readiness endpoints.
* Audit records separate from operational logs.

## 1.10 MVP scope

Included:

* Superadmin bootstrap.
* Passwordless invitations and login.
* User administration.
* Personal and shared calendars.
* Calendar ACLs.
* Timed and all-day events.
* Recurrence and recurrence exceptions.
* Month, week, day, and agenda views.
* Composite views.
* Public composite views.
* Public ICS feed import.
* In-app and email notifications.
* Responsive mobile web UI.
* Docker image.
* k3s deployment through Helm.
* SQLite backup and restore tooling.
* Security and authorization test suite.

## 1.11 Explicitly out of scope for MVP

* Native mobile applications.
* Google OAuth synchronization.
* Two-way external calendar synchronization.
* CalDAV server support.
* Exchange synchronization.
* Meeting-room and equipment booking.
* Video-conference provider integration.
* SMS delivery.
* Offline calendar editing.
* Multi-organization tenant isolation.
* Full-text search across all event history.
* Superadmin access to private event contents.

## 1.12 Success measures

* At least 90% of invited users can activate without administrator assistance.
* A calendar can be created and shared in under two minutes.
* A public view can be created in under three minutes.
* At least 99% of queued notifications reach a terminal delivered or explicitly failed state.
* No unauthorized object access is found in the authorization regression suite.
* Backups meet the agreed recovery-point objective.
* A documented restore test succeeds before production launch.
* Mobile browser testing passes on current major iOS Safari and Android Chrome versions.

---

# 2. Handoff to Staff Tech Lead

## Agent instruction

You are a staff technical lead. Design a concrete technical solution for the supplied product brief.

Mandatory constraints:

* Rust backend.
* SQLite data store.
* React frontend.
* pnpm only; never use Yarn.
* Docker containers.
* k3s hosting.
* Helm-managed deployment.
* Publicly accessible server.
* Strong authentication and authorization.
* Responsive website; no native mobile application.
* Security must be treated as a release requirement.
* Use TDD and automated authorization regression tests.
* Do not claim that security can be absolutely guaranteed.

---

# 3. Staff Tech Lead Proposal

## 3.1 Proposed system

Build a modular monolith consisting of:

* A Rust HTTP API.
* A background job runtime inside the same Rust process.
* A React single-page application.
* A single SQLite database on persistent storage.
* A single application replica.
* An ingress controller providing TLS termination.
* An external transactional email provider.
* Optional S3-compatible object storage for encrypted backups.

A modular monolith is preferred over microservices because:

* The domain is cohesive.
* SQLite strongly favors a single application writer.
* Transactions across calendars, ACLs, invitations, and notifications remain simple.
* Deployment and operational complexity remain low.
* Internal module boundaries can support later extraction if scale requires it.

## 3.2 Deployment topology

```text
Public browser
    |
    | HTTPS
    v
k3s ingress
    |
    v
CommonCal StatefulSet, replicas: 1
    |
    +-- Rust API and background scheduler
    |
    +-- React static assets
    |
    +-- SQLite database on a ReadWriteOnce PVC
    |
    +-- backup snapshot staging directory
    |
    +--> transactional email provider
    |
    +--> validated public ICS destinations
    |
    +--> encrypted backup object storage
```

The deployment must use one application replica while SQLite is the primary database.

SQLite WAL mode improves concurrency because readers do not block a writer and a writer does not block readers, but it does not remove the need to design around serialized writes.

A Kubernetes StatefulSet is appropriate for stable persistent-volume attachment. Persistent volumes survive ordinary Pod replacement, but they are not a substitute for backups.

## 3.3 Repository layout

```text
/
├── backend/
│   ├── Cargo.toml
│   ├── migrations/
│   ├── src/
│   │   ├── main.rs
│   │   ├── app.rs
│   │   ├── config.rs
│   │   ├── error.rs
│   │   ├── db/
│   │   ├── auth/
│   │   ├── users/
│   │   ├── calendars/
│   │   ├── events/
│   │   ├── views/
│   │   ├── feeds/
│   │   ├── notifications/
│   │   ├── audit/
│   │   └── jobs/
│   └── tests/
├── frontend/
│   ├── package.json
│   ├── pnpm-lock.yaml
│   ├── vite.config.ts
│   └── src/
├── deploy/
│   └── helm/
│       └── commoncal/
├── docs/
│   ├── product-brief.md
│   ├── architecture.md
│   ├── threat-model.md
│   ├── authorization-matrix.md
│   ├── backup-restore.md
│   └── operations.md
├── pnpm-workspace.yaml
├── Makefile
└── README.md
```

Repository rules:

* No `yarn.lock`.
* No Yarn commands in documentation or CI.
* Frontend installation uses `pnpm install --frozen-lockfile`.
* Rust dependencies are locked through `Cargo.lock`.
* Production images use reproducible locked dependency installation.

## 3.4 Backend technology

Recommended components:

* Rust.
* Axum for HTTP routing and middleware.
* Tokio runtime.
* SQLx with SQLite.
* Serde for serialization.
* Tower middleware.
* Tracing for structured telemetry.
* Reqwest for external HTTP retrieval.
* A standards-compatible iCalendar parser selected through conformance tests.
* OpenAPI generation or a checked-in API contract.
* UUID-based opaque identifiers.

The application should expose:

* `/api/v1/...` for authenticated APIs.
* `/public/v1/...` for published views.
* `/health/live`.
* `/health/ready`.
* `/metrics`, restricted to internal access.

## 3.5 Frontend technology

* React.
* TypeScript with strict mode.
* Vite.
* pnpm.
* A query/cache library for API state.
* A calendar rendering component supporting month, week, day, list, drag, resize, and touch interaction.
* A schema-driven API client generated from or validated against the backend contract.
* Component-level tests.
* Browser tests using Playwright.
* Responsive CSS based on content breakpoints rather than device names.

No secrets or authorization decisions may exist only in frontend code.

## 3.6 SQLite configuration

Production database configuration:

* WAL journal mode.
* Foreign keys enabled.
* Busy timeout configured.
* Synchronous mode selected deliberately and documented.
* Bounded connection pool.
* Short write transactions.
* Database migrations executed before the server becomes ready.
* Integrity checks included in operational procedures.
* Database and WAL files kept on the same mounted filesystem.

Application replica count remains one.

Do not place the live SQLite database on an eventually consistent object-storage filesystem or a network filesystem without verified SQLite locking semantics.

## 3.7 Data model

### users

* `id`
* `email_normalized`
* `display_name`
* `status`: invited, active, suspended, deleted
* `is_superadmin`
* `preferred_timezone`
* `created_at`
* `updated_at`
* `last_login_at`

Unique index on normalized email.

### invitations

* `id`
* `email_normalized`
* `display_name`
* `token_hash`
* `expires_at`
* `consumed_at`
* `revoked_at`
* `invited_by_user_id`
* `created_at`

### login_tokens

* `id`
* `user_id`
* `token_hash`
* `expires_at`
* `consumed_at`
* `requested_ip_hash`
* `created_at`

### sessions

* `id`
* `user_id`
* `session_token_hash`
* `csrf_secret_hash`
* `created_at`
* `last_seen_at`
* `idle_expires_at`
* `absolute_expires_at`
* `revoked_at`
* `user_agent_summary`
* `ip_prefix_hash`

### calendars

* `id`
* `owner_user_id`
* `name`
* `description`
* `default_timezone`
* `display_color`
* `archived_at`
* `version`
* `created_at`
* `updated_at`

### calendar_acl

* `calendar_id`
* `user_id`
* `role`
* `granted_by_user_id`
* `created_at`
* `updated_at`

Unique key on calendar and user.

The owner is represented both by `owner_user_id` and an invariant checked in the service layer. Ownership changes occur in one transaction.

### events

* `id`
* `calendar_id`
* `created_by_user_id`
* `last_modified_by_user_id`
* `title`
* `description`
* `location`
* `start_instant`
* `end_instant`
* `start_date`
* `end_date_exclusive`
* `timezone`
* `is_all_day`
* `status`
* `visibility`
* `recurrence_rule`
* `recurrence_parent_id`
* `recurrence_id`
* `version`
* `created_at`
* `updated_at`

Timed events use UTC instants plus an IANA timezone.

All-day events use dates and an exclusive end date rather than artificial midnight timestamps.

### event_reminder_overrides

* `event_id`
* `user_id`
* `channel`
* `offset_seconds`
* `enabled`

### notification_preferences

* `user_id`
* `calendar_id`, nullable for account default
* `notification_type`
* `channel`
* `offset_seconds`, nullable
* `enabled`

### shared_views

* `id`
* `owner_user_id`
* `name`
* `description`
* `default_timezone`
* `default_view_mode`
* `version`
* `created_at`
* `updated_at`

### shared_view_calendars

* `view_id`
* `calendar_id`
* `display_order`
* `color_override`
* `enabled`

The service filters inaccessible calendars at read time.

### public_view_links

* `id`
* `view_id`
* `public_token_hash`
* `public_token_lookup_prefix`
* `detail_level`
* `public_title`
* `public_description`
* `expose_source_names`
* `allow_ics_download`
* `not_before`
* `expires_at`
* `revoked_at`
* `created_at`

The raw public token is never stored.

### external_feeds

* `id`
* `calendar_id`
* `source_url_encrypted`
* `source_url_display`
* `refresh_interval_seconds`
* `etag`
* `last_modified`
* `last_attempt_at`
* `last_success_at`
* `next_refresh_at`
* `last_error_code`
* `disabled_at`
* `created_by_user_id`

### external_event_mapping

* `feed_id`
* `external_uid`
* `recurrence_id`
* `event_id`
* `external_sequence`
* `external_modified_at`
* `content_hash`
* `last_seen_sync_id`

### notification_jobs

* `id`
* `deduplication_key`
* `user_id`
* `event_id`
* `channel`
* `scheduled_at`
* `state`
* `attempt_count`
* `next_attempt_at`
* `locked_until`
* `last_error_code`
* `delivered_at`

Unique constraint on `deduplication_key`.

### in_app_notifications

* `id`
* `user_id`
* `type`
* `payload_json`
* `created_at`
* `read_at`

### audit_log

* `id`
* `occurred_at`
* `actor_user_id`
* `actor_session_id`
* `action`
* `target_type`
* `target_id`
* `result`
* `request_id`
* `metadata_json`

Audit metadata must not contain magic-link tokens, session tokens, full ICS URLs containing secrets, or private event descriptions.

## 3.8 Authentication design

### Initial superadmin bootstrap

On an empty database:

1. The operator runs a one-time administrative bootstrap command.
2. The command creates an invitation for the initial superadmin.
3. The invitation link is printed once or delivered through configured email.
4. Bootstrap mode permanently disables after the first superadmin activates.
5. There are no default credentials.

### Invitation tokens

* Generated from at least 256 bits of cryptographically secure randomness.
* Base64url encoded.
* Stored only as a keyed hash or cryptographic hash.
* Email-bound.
* Single-use.
* Short-lived.
* Invalidated after successful acceptance.
* Invalidated when replaced by a new invitation.

### Login tokens

Use equivalent token protections.

The login-request endpoint always returns a generic success response to prevent user enumeration.

### Sessions

Use opaque server-side sessions rather than browser-readable JWT access tokens.

Session cookie properties:

* `Secure`.
* `HttpOnly`.
* `SameSite=Lax`.
* Narrow path.
* Host-only where possible.
* Rotated on authentication and security-sensitive changes.

The server stores only a hash of the session token.

Use:

* Idle expiration.
* Absolute expiration.
* Session revocation.
* “Sign out all sessions.”
* Reauthentication for superadmin promotion, email changes, and similarly sensitive actions.

Session management directly connects authentication to authorization and must be treated as a security-critical subsystem.

### CSRF protection

For cookie-authenticated, state-changing operations:

* Require a CSRF token tied to the session.
* Validate `Origin`.
* Reject unexpected cross-site Fetch Metadata on unsafe methods.
* Keep CORS disabled except for explicitly approved origins.
* Never perform state changes through GET.

OWASP recommends rejecting cross-site unsafe requests and retaining token or origin-based fallback protections.

## 3.9 Authorization design

Create a centralized authorization module rather than embedding ad hoc role comparisons in handlers.

Example interface:

```rust
enum CalendarAction {
    ReadDetails,
    ReadFreeBusy,
    CreateEvent,
    EditAnyEvent,
    ManageSettings,
    ManageAcl,
    TransferOwnership,
    DeleteCalendar,
}

async fn authorize_calendar(
    actor: &AuthenticatedUser,
    calendar_id: CalendarId,
    action: CalendarAction,
) -> Result<AuthorizationDecision, AppError>;
```

Every handler follows:

1. Authenticate.
2. Load the minimum required resource metadata.
3. Authorize the requested action.
4. Execute the operation.
5. Audit security-sensitive changes.

Authorization tests must cover:

* Every role.
* Every action.
* Suspended users.
* Removed ACL entries.
* Cross-calendar identifier substitution.
* Event identifiers belonging to another calendar.
* Public versus private endpoints.
* Superadmin boundaries.
* Ownership transfer.
* Archived calendars.
* External-feed management.

Frontend hiding is usability only and is not an authorization control.

## 3.10 API outline

### Authentication

* `POST /api/v1/auth/login-links`
* `POST /api/v1/auth/login-links/consume`
* `POST /api/v1/auth/invitations/consume`
* `GET /api/v1/auth/session`
* `DELETE /api/v1/auth/session`
* `DELETE /api/v1/auth/sessions`
* `GET /api/v1/auth/csrf`

### Administration

* `GET /api/v1/admin/users`
* `POST /api/v1/admin/invitations`
* `POST /api/v1/admin/invitations/{id}/resend`
* `DELETE /api/v1/admin/invitations/{id}`
* `PATCH /api/v1/admin/users/{id}`
* `DELETE /api/v1/admin/users/{id}/sessions`
* `GET /api/v1/admin/audit`
* `GET /api/v1/admin/job-failures`

### Calendars

* `GET /api/v1/calendars`
* `POST /api/v1/calendars`
* `GET /api/v1/calendars/{id}`
* `PATCH /api/v1/calendars/{id}`
* `DELETE /api/v1/calendars/{id}`
* `POST /api/v1/calendars/{id}/transfer`
* `GET /api/v1/calendars/{id}/acl`
* `PUT /api/v1/calendars/{id}/acl/{userId}`
* `DELETE /api/v1/calendars/{id}/acl/{userId}`

### Events

* `GET /api/v1/events?calendar_id=...&from=...&to=...`
* `POST /api/v1/calendars/{id}/events`
* `GET /api/v1/events/{id}`
* `PATCH /api/v1/events/{id}`
* `DELETE /api/v1/events/{id}`

Updates use a version field or ETag with `If-Match`.

Recurring event updates require an explicit scope:

* This occurrence.
* This and following.
* Entire series.

### Composite views

* `GET /api/v1/views`
* `POST /api/v1/views`
* `GET /api/v1/views/{id}`
* `PATCH /api/v1/views/{id}`
* `DELETE /api/v1/views/{id}`
* `PUT /api/v1/views/{id}/calendars`
* `POST /api/v1/views/{id}/publish`
* `PATCH /api/v1/views/{id}/publication`
* `DELETE /api/v1/views/{id}/publication`

### Public access

* `GET /public/v1/views/{token}`
* `GET /public/v1/views/{token}/events?from=...&to=...`
* `GET /public/v1/views/{token}/calendar.ics`

### External feeds

* `GET /api/v1/calendars/{id}/feeds`
* `POST /api/v1/calendars/{id}/feeds`
* `PATCH /api/v1/feeds/{id}`
* `DELETE /api/v1/feeds/{id}`
* `POST /api/v1/feeds/{id}/refresh`

### Notifications

* `GET /api/v1/notification-preferences`
* `PUT /api/v1/notification-preferences`
* `GET /api/v1/notifications`
* `POST /api/v1/notifications/{id}/read`

## 3.11 Calendar range and recurrence rules

Every event-list request must include a bounded date range.

Server limits:

* Maximum requested range, configurable.
* Maximum expanded occurrences.
* Maximum recurrence-rule length.
* Maximum exception count.
* Maximum event payload size.

Recurrence is expanded only for the requested window.

Recurring source events and exceptions are stored separately. Editing one occurrence creates or updates an exception rather than rewriting the entire series.

Tests must cover:

* Daylight-saving transitions.
* Ambiguous local times.
* Nonexistent local times.
* Monthly rules near month end.
* Leap years.
* All-day recurrence.
* Deleted occurrences.
* Changed occurrences.
* Recurrence limits.

## 3.12 Safe ICS retrieval

External feed retrieval is an SSRF-sensitive subsystem. OWASP identifies user-controlled outbound URLs as a primary SSRF risk.

Required controls:

1. Permit HTTPS by default.
2. Optionally permit HTTP only through explicit deployment configuration.
3. Reject URL usernames and passwords.
4. Reject non-HTTP schemes.
5. Resolve DNS before connecting.
6. Reject loopback addresses.
7. Reject private network ranges.
8. Reject link-local ranges.
9. Reject multicast and unspecified addresses.
10. Reject cloud metadata destinations.
11. Revalidate each redirect.
12. Apply a low redirect limit.
13. Re-resolve and revalidate redirect destinations.
14. Set connection and total timeouts.
15. Limit response bytes.
16. Limit decompressed response bytes.
17. Limit calendar components and event counts.
18. Reject nested or unsupported encodings.
19. Do not forward application cookies or credentials.
20. Use a dedicated HTTP client.
21. Record safe error codes rather than returning internal network details.
22. Apply per-user and per-feed rate limits.
23. Add infrastructure-level egress restrictions where practical.
24. Test DNS rebinding and IPv4/IPv6 representations.

Feed synchronization:

1. Fetch using stored ETag and Last-Modified values.
2. Parse into a temporary normalized representation.
3. Reject the entire update if structural validation fails.
4. Apply changes transactionally.
5. Match events using feed, UID, and recurrence ID.
6. Use sequence and content hashes to detect changes.
7. Mark unseen mapped events as removed only after a successful complete parse.
8. Preserve the prior successful state after a failed refresh.
9. Schedule retries with bounded exponential backoff.

## 3.13 Notifications

### Job creation

Whenever an event or relevant preference changes:

* Recalculate notification jobs within a rolling scheduling horizon.
* Delete obsolete pending jobs.
* Insert new jobs using deterministic deduplication keys.
* Do not alter already delivered history.

### Delivery loop

The background worker:

1. Selects due jobs.
2. Claims a small batch transactionally.
3. Delivers outside the claim transaction.
4. Records success or retry state.
5. Uses bounded exponential backoff.
6. Stops retrying after a configured maximum.
7. Exposes permanent failures to administrators.

### Idempotency

The deduplication key should include:

* User.
* Event occurrence.
* Notification type.
* Channel.
* Scheduled instant.
* Event version or notification-plan version.

### Email privacy

* Do not place private event descriptions in email subjects.
* Respect the user’s current access at delivery time.
* Cancel notifications when access is revoked.
* Do not send private event information to suspended users.
* Avoid secrets in tracking links.
* Allow deployments to disable tracking pixels.

## 3.14 Public-view isolation

Public routes use a separate router stack without authenticated session requirements.

Public responses:

* Never set authentication cookies.
* Use strict output projection.
* Use `X-Robots-Tag: noindex, nofollow` by default.
* Use a dedicated Content Security Policy.
* Apply rate limits.
* Return generic not-found responses for invalid, expired, and revoked links.
* Do not disclose source calendar IDs.
* Do not include event fields outside the publication policy.
* Do not rely on frontend redaction.

Public-token rotation immediately invalidates the previous token.

## 3.15 Browser security

Apply:

* Strict Content Security Policy.
* `frame-ancestors 'none'`.
* `X-Content-Type-Options: nosniff`.
* Referrer policy.
* Permissions policy.
* HSTS after HTTPS deployment is verified.
* Safe React rendering without unsafe HTML insertion.
* Schema validation at API boundaries.
* Request body limits.

CSP is defense in depth and does not replace safe rendering or output handling.

## 3.16 Input and database security

* Use SQLx parameter binding.
* Never build SQL from untrusted field names or sort expressions.
* Map accepted sort keys through enums.
* Validate IDs before queries.
* Limit all list endpoints.
* Escape wildcard characters where user input is used in `LIKE`.
* Enforce database foreign keys.
* Use transactions for ACL and ownership changes.
* Reject unknown JSON fields on security-sensitive commands where practical.
* Do not deserialize unbounded recursive data structures.

## 3.17 Rate limiting

Apply rate limits by operation and identity:

* Login-link requests.
* Invitation acceptance attempts.
* Public-view requests.
* Manual feed refresh.
* Feed creation.
* Event mutation.
* User search.
* Administrative operations.

Use both:

* Per-IP limits.
* Per-account or per-resource limits.

Return generic responses on authentication endpoints.

## 3.18 Audit logging

Audit:

* Invitation creation, resend, revoke, and acceptance.
* Login and logout.
* Session revocation.
* Superadmin changes.
* User suspension.
* Calendar creation and deletion.
* Ownership transfer.
* ACL changes.
* Public-link creation, rotation, and revocation.
* Feed creation and deletion.
* Backup and restore operations.
* Security-sensitive configuration changes.

Audit logs should be append-oriented and inaccessible to normal users.

## 3.19 Container security

Backend container:

* Multi-stage build.
* Minimal runtime image.
* Runs as non-root.
* Read-only root filesystem.
* Writable mounts only where required.
* Drops Linux capabilities.
* `allowPrivilegeEscalation: false`.
* Runtime-default seccomp.
* Explicit CPU and memory requests and limits.
* No package manager in the final image when avoidable.

Frontend assets may be served by the Rust application to keep a same-origin security model.

## 3.20 Helm deployment

Use a StatefulSet:

* `replicas: 1`.
* One PVC.
* Rolling update configured to avoid two simultaneous application writers.
* Readiness withheld until migrations finish.
* Graceful shutdown.
* ConfigMap for non-secret settings.
* Secret references for sensitive settings.
* Service and Ingress.
* TLS configuration.
* NetworkPolicy.
* ServiceAccount with no unnecessary Kubernetes API permissions.
* Pod security context.
* Optional PodDisruptionBudget documented carefully for a single replica.

Do not enable autoscaling while SQLite remains the primary database.

## 3.21 Backups

Use SQLite’s online backup capability or a proven equivalent to create a consistent snapshot of the live database. SQLite documents that its online backup API can copy a live source without continuously blocking other readers and writers.

Backup process:

1. Create an application-consistent SQLite snapshot.
2. Run an integrity check against the snapshot.
3. Compress it.
4. Encrypt it.
5. Upload it to configured object storage.
6. Record backup metadata.
7. Apply retention policy.
8. Alert on repeated failures.

Never copy only the live `.db` file while ignoring WAL state.

Required operational tests:

* Restore into an empty test environment.
* Verify schema migrations.
* Verify user count, calendar count, and representative events.
* Verify the application can authenticate after restore.
* Record recovery duration.

Initial proposed objectives:

* RPO: 24 hours for MVP, configurable toward one hour.
* RTO: four hours for MVP.

These must be confirmed by the product owner before production.

## 3.22 Observability

Structured logs include:

* Timestamp.
* Level.
* Request ID.
* Route template.
* Status.
* Duration.
* Authenticated user ID where appropriate.
* Job ID.
* Feed ID.
* Error code.

Logs exclude:

* Session tokens.
* Magic links.
* CSRF secrets.
* Raw authorization headers.
* Full private event bodies.
* Sensitive query parameters.

Metrics:

* Request count and latency.
* HTTP errors.
* Database busy errors.
* Active database connections.
* Background queue depth.
* Notification delivery outcomes.
* Feed synchronization outcomes.
* Backup age and result.
* Authentication-rate-limit activity.

## 3.23 Test strategy

### Backend unit tests

* Permission mapping.
* Token hashing and expiration.
* Recurrence calculations.
* Notification-plan calculation.
* Public detail projection.
* URL and IP validation.
* Input validation.

### Backend integration tests

Use a real temporary SQLite database with migrations.

Test:

* Authentication flows.
* Calendar CRUD.
* ACL operations.
* Ownership transfer.
* Event operations.
* Concurrent update conflicts.
* Composite views.
* Public projections.
* Feed synchronization.
* Notification claiming.
* Audit generation.

### Authorization matrix tests

Create data for:

* Owner.
* Manager.
* Editor.
* Viewer.
* Free/busy viewer.
* Unrelated user.
* Suspended user.
* Superadmin without calendar ACL.

Exercise every protected operation.

### Frontend tests

* Component behavior.
* Form validation.
* Permission-aware controls.
* Mobile navigation.
* Calendar interaction.
* Public view redaction.
* Authentication expiration.

### End-to-end tests

Playwright flows:

* Bootstrap and activate superadmin.
* Invite a user.
* User activates.
* Create and share a calendar.
* Create a recurring event.
* Publish a composite view.
* Verify public redaction.
* Subscribe to a controlled ICS test server.
* Deliver an in-app notification.
* Verify mobile viewport behavior.

### Security tests

* IDOR/object substitution.
* CSRF.
* Stored and reflected XSS payloads.
* Session fixation.
* Expired and reused magic links.
* User enumeration.
* SSRF destinations.
* Redirect-based SSRF.
* Oversized ICS response.
* Malformed recurrence rules.
* Public-link revocation.
* Rate limiting.
* Security headers.
* Dependency and container scanning.

## 3.24 Delivery stages

### Stage 1: Foundation

* Repository.
* Backend skeleton.
* Frontend skeleton.
* SQLite migrations.
* CI.
* Docker development environment.

### Stage 2: Identity

* Bootstrap.
* Invitations.
* Magic-link login.
* Sessions.
* CSRF.
* User administration.

### Stage 3: Calendars

* Calendar CRUD.
* ACL.
* Events.
* Recurrence.
* Core calendar UI.

### Stage 4: Views and publishing

* Composite views.
* Public links.
* Public projection.
* Public responsive UI.

### Stage 5: Integrations and notifications

* Safe ICS retrieval.
* Feed synchronization.
* In-app notifications.
* Email delivery.

### Stage 6: Production readiness

* Helm.
* Backup and restore.
* Security verification.
* Performance testing.
* Operational documentation.

---

# 4. Handoff to Senior Architect

## Agent instruction

You are a senior software architect.

Review the staff tech lead proposal against the product brief. Identify missing decisions, unsafe assumptions, operational risks, and implementation ambiguities.

Then:

1. Produce binding architecture decisions.
2. Define system invariants.
3. Define release gates.
4. Break implementation into small, ordered prompts for low-reasoning coding agents.
5. Require TDD.
6. Require surgical changes without unrelated refactoring.
7. Keep Rust, SQLite, React, pnpm, Docker, k3s, and Helm.
8. Never introduce Yarn.
9. Preserve a single application replica while SQLite is used.

---

# 5. Senior Architect Review

## 5.1 Verdict

**Approved with mandatory amendments.**

The technical direction is appropriate for the expected initial scale, but the following items must become binding decisions rather than implementation suggestions.

## 5.2 Mandatory architecture decisions

### ADR-001: Modular monolith

The application is a modular monolith.

No separate calendar, authentication, feed, or notification microservices may be introduced during the MVP.

### ADR-002: Single active application replica

The production StatefulSet has exactly one active application replica while SQLite is used.

A deployment must not temporarily run two writable application Pods against separate copies or ambiguous mounts.

### ADR-003: Same-origin web application

The Rust application serves the production React assets or they are exposed through the same public origin.

This minimizes CORS complexity and makes cookie and CSRF policy easier to reason about.

### ADR-004: Server-side opaque sessions

Use opaque session cookies backed by hashed server-side session records.

Do not store authentication JWTs in `localStorage`.

### ADR-005: Passwordless identity

MVP authentication uses invitation links and login magic links.

Password creation, password reset, and password storage are not part of the MVP.

### ADR-006: Superadmin does not imply calendar access

Platform administration and calendar-content authorization are independent.

A superadmin can manage accounts and operations but cannot read arbitrary private calendar contents.

### ADR-007: Explicit authorization service

All calendar, event, view, feed, and notification operations call a centralized authorization service.

Handlers may not duplicate role logic.

### ADR-008: Public views are projections

A public view is generated through a server-side projection that selects permitted fields.

It is not an authenticated view with controls merely hidden by React.

### ADR-009: ICS is one-way and read-only

External ICS events cannot be edited locally.

No Google OAuth or private-feed authentication is included in the MVP.

### ADR-010: Safe outbound HTTP subsystem

Only the dedicated feed-fetching module may make requests to user-supplied URLs.

Other modules must not instantiate unrestricted clients for user-controlled destinations.

### ADR-011: Background work is database-backed

Notification and feed work is represented in SQLite.

In-memory timers may wake the worker but cannot be the sole source of job state.

### ADR-012: Recurrence behavior is a domain subsystem

Recurrence parsing, expansion, exception handling, and timezone conversion must be isolated and extensively tested.

Handlers and React components must not independently implement recurrence calculations.

### ADR-013: Backup correctness precedes production

Production deployment is blocked until a backup has been restored successfully into a clean environment.

### ADR-014: pnpm-only frontend

The only permitted JavaScript package manager is pnpm.

CI fails if a Yarn lockfile is added.

## 5.3 Additional gaps and resolutions

### Gap: Email delivery dependency

Resolution:

Define an `EmailSender` interface with:

* Production provider adapter.
* Test in-memory adapter.
* Development log adapter.

The development adapter must not print complete login tokens unless explicitly enabled in a local-only mode.

### Gap: Revoked access and notifications

Resolution:

Every notification delivery rechecks current access before including private details.

Pending jobs are cancelled when a calendar ACL is removed.

### Gap: Suspended users

Resolution:

Suspension:

* Revokes existing sessions.
* Rejects login-link consumption.
* Prevents new login-link issuance from creating a usable login.
* Cancels pending private notifications.
* Preserves owned calendars.
* Prevents ownership transfer to the suspended user.

A superadmin must resolve calendar ownership before permanent deletion.

### Gap: User deletion

Resolution:

MVP implements soft deletion.

Permanent deletion requires:

* No owned calendars.
* No active sessions.
* No pending invitations.
* A retention policy.
* An explicit administrative workflow.

### Gap: Public link analytics

Resolution:

Do not record raw public tokens or invasive visitor profiles.

Operational request counts may be collected without cross-site tracking.

### Gap: Timezone ownership

Resolution:

* Accounts have a preferred display timezone.
* Calendars have a default timezone.
* Timed events retain their creation timezone.
* Public views have an explicit display timezone.
* All notification execution times are calculated as UTC instants.
* All-day dates remain date values.

### Gap: Search and event range

Resolution:

Do not implement an unbounded “all events” endpoint.

Every event query requires an interval and has a maximum result count.

### Gap: Concurrent updates

Resolution:

Mutable resources carry a version.

Updates with a stale version return HTTP 409 or 412 with the current version metadata.

### Gap: Feed URL confidentiality

Resolution:

Some public ICS URLs contain secret-like path or query tokens.

Store the complete URL encrypted at rest using an application-level key supplied through a Kubernetes Secret.

Display only a redacted hostname and path summary.

Never log the full URL.

### Gap: Public link storage

Resolution:

Use a lookup prefix plus a strong token hash.

The prefix locates a small candidate set; constant-time hash comparison confirms the token.

### Gap: Migration recovery

Resolution:

Migrations are:

* Forward-only in production.
* Transactional where SQLite permits.
* Tested against representative database snapshots.
* Completed before readiness becomes true.

### Gap: Schema and API compatibility

Resolution:

The frontend and backend share a checked API contract.

CI must detect incompatible contract drift.

## 5.4 System invariants

The following invariants must always hold:

1. At least one active superadmin exists.
2. Every calendar has exactly one active owner.
3. The owner has effective owner permissions.
4. A calendar cannot be deleted by a non-owner.
5. A composite view grants no source-calendar access.
6. A public view contains only projected fields.
7. Revoked public tokens are unusable immediately.
8. Invitation and login tokens are single-use.
9. Raw authentication tokens are never stored.
10. Suspended users cannot create authenticated sessions.
11. An unrelated user cannot distinguish nonexistent private resources from inaccessible private resources where doing so would leak existence.
12. Imported feed events are read-only.
13. Failed feed synchronization does not erase the last successful state.
14. Notification delivery revalidates access.
15. Job delivery is idempotent.
16. Recurrence expansion is bounded.
17. Every state-changing protected request passes CSRF validation.
18. No user-controlled URL can reach prohibited network destinations.
19. The live SQLite database has one active application writer deployment.
20. No production release occurs without a verified restore procedure.
21. Yarn is never used.

## 5.5 Release gates

### Functional gate

All MVP acceptance scenarios pass.

### Authorization gate

The complete role/action matrix passes, including negative and object-substitution tests.

### Security gate

* ASVS Level 2 checklist reviewed.
* No unresolved critical or high security findings.
* CSRF, XSS, SSRF, session, and IDOR tests pass.
* Security headers pass automated validation.
* Dependency and container scans have no unaccepted critical findings.

### Data gate

* Migrations succeed from the previous release.
* Backup completes.
* Restore succeeds in a clean environment.
* SQLite integrity check succeeds after restore.

### Reliability gate

* Notification retries are verified.
* Feed failure preserves previous data.
* Graceful shutdown does not abandon claimed jobs permanently.
* Pod restart preserves data.

### UI gate

* Primary workflows pass desktop and mobile browser tests.
* Month, week, day, and agenda views are usable.
* Keyboard-accessible list view is available.
* No horizontal viewport overflow at supported mobile widths.

### Operational gate

* Helm install, upgrade, rollback, and uninstall behavior are documented.
* PVC retention behavior is tested.
* Alert conditions are documented.
* Initial superadmin recovery process is documented.

---

# 6. Low-Reasoning Agent Execution Contract

Prepend this contract to every implementation prompt.

```text
You are implementing one narrowly scoped task in the CommonCal repository.

Mandatory stack:
- Rust backend.
- SQLite through SQLx.
- React and TypeScript frontend.
- pnpm only.
- Never use Yarn or create yarn.lock.
- Docker, k3s, and Helm for deployment.

Working rules:
1. Inspect the current repository before editing.
2. Do not rewrite or refactor unrelated code.
3. Use test-driven development:
   a. Add the smallest relevant failing test.
   b. Run it and confirm that it fails for the intended reason.
   c. Implement the minimum production change.
   d. Run the focused test.
   e. Run the affected broader test suite.
4. Preserve existing public behavior unless this task explicitly changes it.
5. Do not add a dependency unless necessary.
6. Do not weaken authentication, authorization, CSRF, validation, or logging controls.
7. Use parameterized SQL only.
8. Do not log tokens, secrets, complete ICS URLs, or private event descriptions.
9. Update documentation only where this task changes an established contract.
10. Finish with:
   - Files changed.
   - Tests added.
   - Commands run.
   - Remaining limitations.
11. Stop after this task. Do not begin the next feature.
```

---

# 7. Ordered Implementation Prompts

## Prompt 01 — Repository foundation

```text
Create the minimum CommonCal repository foundation.

Scope:
- Create backend Rust crate.
- Create React TypeScript frontend using Vite and pnpm.
- Create pnpm-workspace.yaml.
- Add root Makefile commands for backend test, frontend test, lint, and all checks.
- Add CI that runs Cargo checks and pnpm frozen-lockfile checks.
- Make CI fail when yarn.lock exists.
- Add placeholder deploy/helm/commoncal and docs directories.
- Add a basic README with pnpm-only commands.

Tests:
- Add a backend smoke test.
- Add a frontend render smoke test.
- Add a CI script test that detects a temporary yarn.lock.

Do not implement authentication, database tables, calendars, or deployment resources yet.
```

## Prompt 02 — Backend application skeleton

```text
Implement the Rust application skeleton.

Scope:
- Add configuration loading with explicit development and production validation.
- Add Axum router construction.
- Add /health/live and /health/ready.
- Add structured tracing with request IDs.
- Add a consistent JSON error envelope.
- Add graceful shutdown.
- Do not expose stack traces or internal errors in HTTP responses.

Tests:
- Health endpoint tests.
- Request ID propagation test.
- Unknown route error test.
- Configuration rejection test for missing production secrets.

Do not add domain endpoints.
```

## Prompt 03 — SQLite connection and migrations

```text
Add SQLite persistence infrastructure.

Scope:
- Configure SQLx SQLite pool.
- Enable foreign keys.
- Enable WAL mode.
- Configure busy timeout.
- Add migration runner executed before readiness.
- Create only a schema_migrations verification table if SQLx does not already provide sufficient migration tracking.
- Read database path from validated configuration.
- Ensure tests use isolated temporary databases.

Tests:
- Migrations run on a fresh database.
- Re-running migrations is safe.
- Foreign-key violations fail.
- Readiness remains false when migration fails.
- WAL mode is active in the integration database.

Do not create product-domain tables yet.
```

## Prompt 04 — Core identity schema

```text
Create migrations and repository types for:
- users
- invitations
- login_tokens
- sessions
- audit_log

Requirements:
- Unique normalized email.
- Explicit user status.
- Invitation, login-token, and session hashes only.
- Expiration and revocation fields.
- At least one schema-level constraint for invalid status values.
- Audit entries cannot be updated through the normal repository API.

Tests:
- Duplicate normalized emails fail.
- Invalid status fails.
- Expired and revoked records can be queried correctly.
- Repository round-trip tests.
- No repository method accepts or returns raw token values as stored database fields.

Do not build HTTP authentication flows yet.
```

## Prompt 05 — Authorization domain

```text
Implement the centralized authorization domain without HTTP handlers.

Scope:
- Define platform role and calendar roles.
- Define CalendarAction.
- Implement a pure permission mapping.
- Deny by default.
- Define an authorization decision type that does not leak resource details.
- Add a documented role/action matrix.

Tests:
- Table-driven test for every role and action.
- Unrecognized or missing role denies.
- Superadmin has no implicit private-calendar read permission.
- Suspended users deny all normal calendar actions.
- Owner-only actions remain owner-only.

Do not query the database yet.
```

## Prompt 06 — Token and session primitives

```text
Implement security primitives for invitation tokens, login tokens, sessions, and CSRF.

Scope:
- Cryptographically secure random token generation.
- URL-safe external encoding.
- One-way token hashing with application-domain separation.
- Constant-time verification.
- Expiration checks.
- Single-use state model.
- Secure session-cookie builder.
- CSRF token generation and validation tied to a session.

Tests:
- Generated tokens have sufficient entropy and are not repeated in a deterministic test sample.
- Correct token verifies.
- Modified token fails.
- Tokens from different domains cannot be substituted.
- Expired token fails.
- Consumed token fails.
- Cookie has Secure, HttpOnly, SameSite, and expected Path.
- CSRF token from another session fails.

Do not add email sending or HTTP endpoints.
```

## Prompt 07 — Email abstraction

```text
Implement an EmailSender abstraction.

Scope:
- Define invitation-email and login-link-email commands.
- Add an in-memory test implementation.
- Add a development implementation that redacts tokens by default.
- Add a production provider interface without selecting multiple providers.
- Ensure email subjects contain no authentication token.
- Ensure logs contain no raw authentication link.

Tests:
- In-memory sender captures the intended recipient and message type.
- Development sender output is redacted.
- Error propagation uses safe internal error codes.
- Token values do not appear in captured structured logs.

Do not implement authentication handlers.
```

## Prompt 08 — Initial superadmin bootstrap

```text
Implement the one-time initial-superadmin bootstrap application command.

Scope:
- Permit bootstrap only when no users and no consumed bootstrap exist.
- Create a superadmin invitation.
- Do not create a default password.
- Make repeated bootstrap attempts fail safely.
- Audit successful and rejected bootstrap attempts.
- Make the command usable from the backend binary CLI.

Tests:
- Empty database permits bootstrap.
- Second bootstrap attempt is rejected.
- Existing user blocks bootstrap.
- Created invitation is superadmin-bound.
- Raw token is returned only to the command caller and is not stored.
- Audit entry contains no token.

Do not add normal superadmin invitations yet.
```

## Prompt 09 — Invitation activation flow

```text
Implement invitation consumption.

Scope:
- Add POST /api/v1/auth/invitations/consume.
- Validate token, expiry, revocation, and single-use status.
- Activate or create the invited user transactionally.
- Create a new session.
- Rotate away any prior pending token.
- Set secure session and CSRF cookies or return the documented CSRF bootstrap mechanism.
- Return the active user summary.
- Audit success and failure with safe reason codes.

Tests:
- Valid invitation activates user.
- Reused invitation fails.
- Expired invitation fails.
- Revoked invitation fails.
- Email collision resolves without duplicate user creation.
- Database rollback occurs when session creation fails.
- Response does not expose token hashes.
- Session fixation is not possible.

Do not implement login-link requests.
```

## Prompt 10 — Passwordless login flow

```text
Implement passwordless login-link request and consumption.

Scope:
- Add generic-response login-link request endpoint.
- Create a token only for an eligible active user.
- Apply per-IP and per-email rate limiting through an injectable limiter interface.
- Send through EmailSender.
- Add login-link consumption endpoint.
- Consume the token and create a rotated session.
- Reject suspended or deleted users.
- Update last_login_at.
- Audit safely.

Tests:
- Registered and unknown email requests return indistinguishable HTTP responses.
- Active user receives a link.
- Unknown user does not create a token.
- Suspended user cannot log in.
- Expired and reused links fail.
- Rate limit works.
- Successful login rotates the session.
- Logs do not reveal account existence or tokens.
```

## Prompt 11 — Session middleware and CSRF

```text
Implement authenticated-session middleware and CSRF enforcement.

Scope:
- Resolve the opaque session cookie to an active user.
- Enforce idle and absolute expiration.
- Update last-seen using a write-throttled strategy.
- Reject revoked sessions.
- Add session inspection endpoint.
- Add single-session logout and all-session logout.
- Require CSRF for POST, PUT, PATCH, and DELETE authenticated browser requests.
- Validate Origin and Fetch Metadata according to configuration.
- Exempt public and authentication token-consumption GET-free flows only where explicitly required.

Tests:
- Valid session authenticates.
- Revoked, idle-expired, and absolute-expired sessions fail.
- Unsafe request without CSRF fails.
- Wrong-session CSRF fails.
- Cross-site unsafe request fails.
- Safe GET does not require CSRF.
- All-session logout revokes every session.
```

## Prompt 12 — Superadmin user management

```text
Implement minimal superadmin user administration.

Scope:
- List users by status with pagination.
- Invite a user.
- Revoke and resend an invitation.
- Suspend and reactivate a user.
- Revoke a user’s sessions.
- Promote or demote a superadmin.
- Prevent removal of the final active superadmin.
- Apply centralized platform authorization.
- Audit every mutation.

Tests:
- Normal users receive denial.
- Final superadmin cannot be demoted or suspended.
- Duplicate pending invitation is handled deterministically.
- Resend invalidates the previous token.
- Suspending a user revokes sessions.
- User listing never exposes token hashes.
- Object identifier substitution does not bypass authorization.
```

## Prompt 13 — Calendar schema and repository

```text
Create calendar persistence.

Scope:
- Add calendars and calendar_acl migrations.
- Add calendar role constraints.
- Add calendar repository.
- Implement transactional calendar creation with owner ACL.
- Implement ownership-transfer repository operation.
- Add version field for optimistic concurrency.
- Enforce unique calendar/user ACL entries.

Tests:
- Calendar creation creates exactly one owner.
- Duplicate ACL fails.
- Ownership transfer updates owner and ACL atomically.
- Failed transfer rolls back.
- Stale version update fails.
- Foreign-key behavior is correct.

Do not add HTTP endpoints.
```

## Prompt 14 — Calendar CRUD API

```text
Implement calendar CRUD endpoints.

Scope:
- List only calendars visible to the authenticated user.
- Create calendar.
- Read calendar metadata according to authorization.
- Update calendar settings with optimistic concurrency.
- Archive and restore calendar.
- Delete calendar as owner only.
- Use centralized authorization.
- Return free/busy users only fields they are allowed to see.
- Audit create, archive, restore, and delete.

Tests:
- Owner, manager, editor, viewer, free/busy, and unrelated-user cases.
- User cannot retrieve a calendar by substituting another ID.
- Manager cannot delete.
- Editor cannot change settings.
- Stale update returns conflict.
- Deleted or inaccessible calendars return the documented non-leaking response.
```

## Prompt 15 — Calendar sharing API

```text
Implement calendar ACL management.

Scope:
- List ACL as owner or manager.
- Grant or update Manager, Editor, Viewer, or FreeBusy roles.
- Revoke an ACL.
- Transfer ownership through an explicit endpoint.
- Prevent removal of the owner ACL.
- Prevent ownership transfer to suspended or deleted users.
- Cancel affected pending notifications when access is revoked.
- Audit every ACL change.

Tests:
- Complete role/action matrix.
- Manager can manage sharing but cannot transfer ownership unless architecture contract explicitly permits it; follow the documented contract.
- Editor cannot manage ACL.
- Owner cannot remove self.
- Ownership transfer is atomic.
- Access disappears immediately after revocation.
```

## Prompt 16 — Event schema and basic repository

```text
Create the event persistence model.

Scope:
- Add events migration.
- Support timed and all-day events.
- Store timed UTC instants plus event timezone.
- Store all-day start and exclusive end dates.
- Add event status and version.
- Validate start before end.
- Add bounded range-query repository.
- Add indexes for calendar and time-range access.

Tests:
- Timed event round trip.
- All-day event round trip.
- Invalid ranges fail.
- Range query excludes non-overlapping events.
- Cross-calendar event lookup remains scoped.
- Stale version update fails.

Do not add recurrence yet.
```

## Prompt 17 — Basic event API

```text
Implement non-recurring event CRUD.

Scope:
- Create event on calendars where the user can edit.
- Read event with full-details or free/busy projection.
- Update with optimistic concurrency.
- Delete event.
- Move an event between calendars only when the user can edit both.
- Require bounded from/to range for list requests.
- Audit mutations without storing private descriptions in audit metadata.

Tests:
- Every calendar role.
- Free/busy projection excludes title, description, and location.
- Viewer cannot mutate.
- Event-ID substitution cannot bypass calendar authorization.
- Moving to an unauthorized calendar fails atomically.
- Stale version returns conflict.
```

## Prompt 18 — Recurrence domain

```text
Implement recurrence as a backend domain module.

Scope:
- Parse the supported RFC 5545 recurrence-rule subset.
- Expand occurrences only inside a requested interval.
- Enforce occurrence and complexity limits.
- Support excluded occurrences.
- Support modified single occurrences.
- Handle event timezone and daylight-saving transitions.
- Return deterministic domain errors for unsupported rules.

Tests:
- Daily, weekly, monthly, and yearly recurrence.
- COUNT and UNTIL.
- Leap year.
- Month-end behavior.
- Daylight-saving spring and autumn transitions.
- Excluded occurrence.
- Modified occurrence.
- Maliciously large or non-terminating rule is rejected or bounded.

Do not add HTTP recurring-edit workflows yet.
```

## Prompt 19 — Recurring event API

```text
Extend event APIs to recurring events.

Scope:
- Create a recurring series.
- List expanded occurrences for a bounded range.
- Update one occurrence.
- Delete one occurrence.
- Update entire series.
- Implement “this and following” only if the domain design supports it without ambiguous behavior; otherwise return a documented not-yet-supported response and keep it out of advertised MVP until implemented.
- Keep series and exceptions transactionally consistent.
- Recalculate notification plans after changes.

Tests:
- Series creation and expansion.
- Single occurrence update.
- Single occurrence deletion.
- Entire-series update.
- Concurrent series edits.
- Authorization applies to every operation.
```

## Prompt 20 — Composite view persistence and API

```text
Implement private composite calendar views.

Scope:
- Add shared_views and shared_view_calendars migrations.
- Create, read, update, and delete a user-owned view.
- Add, remove, reorder, and recolor source calendars.
- Verify current source-calendar access whenever the view is read.
- Automatically omit calendars that are no longer accessible.
- Do not copy events into the view.
- Query combined events through existing bounded event services.

Tests:
- View owner can manage the view.
- Another user cannot edit it.
- Revoked calendar access removes its events from the result.
- Adding an inaccessible calendar fails.
- View does not grant new calendar permissions.
- Combined results preserve source identity only for authorized private users.
```

## Prompt 21 — Public view publication

```text
Implement public composite-view publication.

Scope:
- Add public_view_links migration.
- Generate strong opaque public tokens.
- Store only lookup prefix and token hash.
- Create, configure, rotate, expire, and revoke a publication.
- Add unauthenticated public metadata and bounded event endpoints.
- Implement FullDetails, TitleAndTime, and FreeBusy projections server-side.
- Return generic not-found for invalid, expired, and revoked tokens.
- Set public security, caching, and noindex headers.
- Never set authenticated session cookies on public responses.

Tests:
- Each projection contains only permitted fields.
- Raw public token is not stored.
- Rotated token invalidates old token.
- Expired and revoked tokens fail.
- Source calendar and user identifiers are absent.
- Public API does not accept mutations.
- A source ACL change is reflected immediately.
```

## Prompt 22 — Safe ICS HTTP client

```text
Implement the dedicated safe outbound HTTP client for ICS retrieval.

Scope:
- Accept only configured HTTP schemes.
- Reject URL credentials.
- Resolve hostnames.
- Reject loopback, private, link-local, multicast, unspecified, and metadata destinations for IPv4 and IPv6.
- Revalidate every redirect.
- Limit redirects.
- Set connection and total timeouts.
- Limit compressed and decompressed response sizes.
- Do not send application cookies or credentials.
- Return safe categorized errors.
- Make DNS resolution and transport injectable for tests.

Tests:
- Public HTTPS target accepted.
- localhost and private ranges rejected.
- IPv4-mapped IPv6 bypass rejected.
- Alternative numeric address representation rejected where parser permits it.
- Redirect to a private address rejected.
- DNS result changing between validation and connection is safely handled by the chosen connection strategy.
- Oversized and slow responses fail.
- Logs redact the URL query and credentials.
```

## Prompt 23 — ICS parsing and normalization

```text
Implement ICS parsing into a normalized temporary model.

Scope:
- Parse calendar and event components.
- Support UID, DTSTART, DTEND, DURATION where valid, SUMMARY, DESCRIPTION, LOCATION, STATUS, RRULE, EXDATE, RECURRENCE-ID, SEQUENCE, DTSTAMP, and LAST-MODIFIED.
- Support timed and all-day events.
- Reject invalid combinations.
- Enforce component, event, text, and recurrence limits.
- Preserve no executable HTML behavior.
- Do not write to the database in this task.

Tests:
- Representative public Google Calendar ICS fixture.
- Timed and all-day events.
- Recurrence and exception fixture.
- Escaped text.
- Folded lines.
- Malformed input.
- Oversized components.
- Duplicate UID behavior is deterministic.
```

## Prompt 24 — External feed synchronization

```text
Implement external feed persistence and synchronization.

Scope:
- Add external_feeds and external_event_mapping migrations.
- Encrypt full source URLs at rest.
- Expose only redacted URL summaries.
- Create, disable, delete, list, and manually refresh feeds.
- Require calendar manager permission.
- Use ETag and Last-Modified.
- Parse completely before applying changes.
- Apply a successful synchronization transactionally.
- Preserve previous state after failure.
- Imported events are marked read-only.
- Record safe status and error codes.
- Schedule the next refresh.

Tests:
- Initial import.
- Unchanged 304 result.
- Update, addition, and removal.
- Failed parse preserves previous events.
- Duplicate external UIDs are handled consistently.
- Editor without manage permission cannot add a feed.
- Imported event update through normal event API is rejected.
- Complete URL is absent from logs and API responses.
```

## Prompt 25 — Notification preferences and planning

```text
Implement notification preference storage and notification-plan calculation.

Scope:
- Add notification_preferences, event_reminder_overrides, notification_jobs, and in_app_notifications migrations.
- Resolve effective preference using event, calendar, then account precedence.
- Generate deterministic notification jobs for event occurrences inside a rolling horizon.
- Remove obsolete pending jobs after event or preference changes.
- Preserve delivered records.
- Cancel private jobs after access revocation.

Tests:
- Preference precedence.
- Two users receive independent schedules.
- Recurring occurrences receive distinct deduplicated jobs.
- Event edit replaces obsolete pending jobs.
- Access revocation cancels pending jobs.
- All-day notification timezone behavior.
```

## Prompt 26 — Notification worker

```text
Implement notification-job delivery.

Scope:
- Claim due jobs transactionally in bounded batches.
- Recover jobs after an expired claim.
- Recheck user status and event access before delivery.
- Deliver in-app notifications.
- Deliver email through EmailSender.
- Mark delivered jobs idempotently.
- Retry transient failures with bounded backoff.
- Mark permanent failure after the configured limit.
- Emit metrics and safe logs.

Tests:
- One job is not delivered twice.
- Expired claim can be recovered.
- Concurrent claim attempts select one winner.
- Revoked access prevents private delivery.
- Suspended user receives nothing.
- Transient failure retries.
- Permanent failure reaches terminal state.
```

## Prompt 27 — React authentication shell

```text
Implement the frontend authentication shell.

Scope:
- Login-link request page.
- Invitation-consumption page.
- Login-link-consumption page.
- Authenticated application shell.
- Session loading.
- CSRF integration for mutations.
- Logout.
- Expired-session handling.
- Accessible loading and error states.
- pnpm only.

Tests:
- Generic login request confirmation.
- Invitation consumption success and failure.
- Authenticated shell renders user.
- Unsafe API call includes CSRF.
- Expired session returns to login without losing a safe redirect target.
- No token is persisted in localStorage or sessionStorage.
```

## Prompt 28 — Calendar and sharing management UI

```text
Implement responsive calendar-management pages.

Scope:
- Calendar list.
- Create and edit calendar.
- Archive and restore.
- Sharing dialog.
- Role selection.
- Ownership-transfer confirmation.
- Permission-aware controls.
- Mobile-responsive forms and dialogs.
- Accessible labels and focus management.

Tests:
- Viewer does not see edit controls.
- Owner can open sharing.
- Manager sees only permitted controls.
- Ownership transfer requires explicit confirmation.
- API authorization failure is handled even when a control was visible from stale state.
- Mobile viewport has no horizontal overflow.
```

## Prompt 29 — Calendar event UI

```text
Implement the authenticated calendar interface.

Scope:
- Month, week, day, and agenda/list views.
- Toggle visible calendars.
- Create event.
- Edit event.
- Drag and resize writable events.
- Display read-only and external events appropriately.
- Handle optimistic-concurrency conflicts.
- Add mobile navigation and touch-friendly event editing.
- Use backend recurrence expansion rather than implementing recurrence calculations in React.

Tests:
- View switching.
- Quick event creation.
- Viewer cannot drag or edit.
- External event is visibly read-only.
- Conflict response prompts reload or merge-safe retry.
- Agenda view is keyboard accessible.
- Narrow mobile layout remains usable.
```

## Prompt 30 — Composite and public view UI

```text
Implement private composite-view management and the public calendar page.

Scope:
- Create and edit composite views.
- Add permitted calendars.
- Reorder and override colors.
- Configure publication detail level and expiration.
- Show and rotate a public link.
- Create an unauthenticated public month and agenda view.
- Do not reuse authenticated data objects in a way that can accidentally expose private fields.
- Display expired or revoked links generically.

Tests:
- Inaccessible calendars cannot be selected.
- Public FullDetails, TitleAndTime, and FreeBusy fixtures render only permitted fields.
- Public page sends no authenticated mutation.
- Public mobile agenda works.
- Link rotation updates the displayed URL and invalidates the old fixture.
```

## Prompt 31 — Docker images

```text
Create production Docker packaging.

Scope:
- Multi-stage Rust build.
- Frontend build with pnpm --frozen-lockfile.
- Serve built frontend through the Rust application.
- Minimal non-root runtime.
- Read-only-compatible filesystem layout.
- Explicit writable database and temporary paths.
- Healthcheck support.
- No Yarn installation or commands.
- Add .dockerignore.

Tests:
- Image builds from a clean checkout.
- Container runs as non-root.
- Health endpoint responds.
- Static frontend loads.
- Application fails clearly when required production configuration is missing.
- Image inspection finds no source tokens or development environment files.
```

## Prompt 32 — Helm chart

```text
Implement the production Helm chart for k3s.

Scope:
- StatefulSet with replicas fixed to 1 by schema validation.
- PersistentVolumeClaim.
- Service.
- Ingress with configurable TLS.
- ConfigMap and Secret references.
- Startup, readiness, and liveness probes.
- Non-root security context.
- Read-only root filesystem.
- Dropped capabilities.
- RuntimeDefault seccomp.
- Resource requests and limits.
- Graceful termination period.
- NetworkPolicy.
- PVC retention behavior documented.
- No HPA.

Tests:
- helm lint.
- helm template assertions.
- Replicas greater than one are rejected.
- Security context fields are present.
- Database path maps to the PVC.
- Secrets are referenced rather than rendered as literal defaults.
```

## Prompt 33 — Backup and restore command

```text
Implement application-consistent backup and restore tooling.

Scope:
- Add an administrative backup command using the SQLite online backup mechanism or an equivalent verified library interface.
- Run integrity verification against the snapshot.
- Produce a compressed artifact.
- Support encryption and upload through an interface.
- Record backup metadata without storing encryption keys.
- Add a restore command that refuses to overwrite a running production database.
- Document clean-environment restoration.

Tests:
- Backup while the application performs controlled writes.
- Restored database passes integrity check.
- Restored representative records match.
- Corrupt snapshot is rejected.
- Encryption round trip.
- Upload failure retains a safe local recovery state according to policy.
- Tokens and secret configuration are absent from logs.
```

## Prompt 34 — Security headers and request limits

```text
Add application-wide HTTP hardening.

Scope:
- Content Security Policy.
- frame-ancestors protection.
- MIME sniffing protection.
- Referrer policy.
- Permissions policy.
- HSTS behind an explicit production HTTPS setting.
- Request body limits.
- JSON content-type enforcement.
- Safe cache policies for authenticated and public responses.
- No-store policy for authentication-token responses.
- Consistent Origin and Fetch Metadata checks.

Tests:
- Headers on HTML, API, public, and authentication responses.
- Oversized request rejected.
- Wrong content type rejected.
- Public response receives correct noindex policy.
- HSTS is absent in local HTTP mode and present in production HTTPS mode.
```

## Prompt 35 — Authorization regression suite

```text
Create a dedicated authorization regression suite.

Scope:
- Build fixtures for every calendar role, unrelated user, suspended user, and superadmin without ACL.
- Exercise every calendar, event, ACL, view, feed, and notification endpoint.
- Attempt identifier substitution across two calendars.
- Attempt event-to-calendar mismatch.
- Attempt public-token use on authenticated endpoints.
- Verify non-leaking denial responses.
- Produce a machine-readable authorization coverage report.

Tests:
- The suite itself is the deliverable.
- Add a deliberate test-only insecure fixture proving the suite catches a missing authorization check, then remove the insecure fixture while retaining the regression test.
- CI must run the suite.
```

## Prompt 36 — End-to-end MVP flow

```text
Add Playwright end-to-end coverage for the complete MVP journey.

Flow:
1. Bootstrap superadmin through test support.
2. Activate superadmin.
3. Invite a normal user.
4. Activate the normal user.
5. Create a calendar.
6. Share it as Editor.
7. Create a recurring event.
8. Create a composite view.
9. Publish title-and-time-only.
10. Verify unauthenticated public redaction.
11. Add a controlled ICS feed.
12. Verify imported read-only event.
13. Generate and display an in-app notification.
14. Repeat primary viewing flows at a mobile viewport.

Requirements:
- Use deterministic local email and ICS test adapters.
- No arbitrary sleeps.
- Preserve screenshots or traces only on failure.
- Run through pnpm.
```

## Prompt 37 — Threat model and ASVS release checklist

```text
Create security documentation grounded in the implemented system.

Scope:
- Document assets, trust boundaries, actors, entry points, and data flows.
- Cover authentication, authorization, sessions, public links, outbound ICS fetching, notifications, SQLite storage, backups, ingress, and administrator operations.
- Enumerate misuse cases and existing controls.
- Map applicable OWASP ASVS 5.0 Level 2 requirements to code, tests, configuration, or an explicit gap.
- Do not mark a requirement complete without evidence.
- Create a release-blocking checklist for unresolved high-risk gaps.

No application behavior changes are required unless documentation reveals a small, directly related defect with a testable fix. Do not perform broad refactoring.
```

## Prompt 38 — Production readiness verification

```text
Perform the final production-readiness task.

Scope:
- Run all backend tests.
- Run authorization regression tests.
- Run frontend tests.
- Run Playwright tests.
- Run Cargo formatting, lint, audit, and dependency checks.
- Run pnpm lint, typecheck, tests, and production build.
- Build and inspect the Docker image.
- Run Helm lint and template tests.
- Perform a backup and clean restore.
- Verify database integrity after restore.
- Verify no yarn.lock exists.
- Verify no documented command uses Yarn.
- Produce a release report listing evidence, failures, accepted risks, RPO, and RTO.

Do not silently fix broad unrelated issues. Apply only small release-blocking corrections with tests and document every correction.
```

---

# 8. Final Recommended Build Sequence

Execute the prompts in numerical order, with these checkpoints:

### Checkpoint A: secure identity

Complete prompts 01–12.

Exit criteria:

* Superadmin can activate.
* User can log in passwordlessly.
* Sessions and CSRF work.
* User administration is authorization-tested.

### Checkpoint B: private calendar product

Complete prompts 13–19 and 27–29.

Exit criteria:

* Users can create and share calendars.
* Events and recurrence work.
* Desktop and mobile calendar UI works.

### Checkpoint C: views and integrations

Complete prompts 20–26 and 30.

Exit criteria:

* Composite views work.
* Public projections are secure.
* Public ICS subscriptions work safely.
* User-specific notifications work.

### Checkpoint D: production

Complete prompts 31–38.

Exit criteria:

* Docker and Helm deployment work.
* Backup restore succeeds.
* Security gates pass.
* Release evidence is complete.

---

# 9. Most Important Architectural Warning

SQLite is suitable for the first version only if the deployment accepts:

* One active application replica.
* A temporary service interruption during certain failures or upgrades.
* Careful write-transaction design.
* Tested backup and restore.
* A documented future database migration threshold.

Triggers for reassessing SQLite should include:

* Sustained write contention.
* Required zero-downtime multi-replica service.
* Cross-region availability.
* Database size or backup duration exceeding the recovery objective.
* Notification or feed workloads materially affecting interactive requests.

The repository layer should keep database-specific code localized, but the MVP should not build an abstract persistence framework solely for a hypothetical migration.

