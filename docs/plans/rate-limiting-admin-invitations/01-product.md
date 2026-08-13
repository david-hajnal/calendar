# Product: Rate limiting on admin invitations

## Problem
Admins in the superadmin panel can send unlimited invitations in rapid succession. Each invitation triggers an email send, so unbounded invites waste email quotas, increase costs, and can be abused (accidental or intentional) to flood the system.

## Success metric
0 wasted email sends from admin invitation rate-limiting in production — rate limiter catches abuse before email is sent.

## Announcement — the blog post before the feature
Superadmins can now send up to 5 invitations per minute from the admin panel. If you exceed this limit, you'll see a brief message asking you to wait. This protects our email infrastructure from accidental spam and keeps costs predictable.

## Screens
Admin panel invitation form — rate limit error shown as inline toast/banner when limit is exceeded (existing UI, no new screens)
