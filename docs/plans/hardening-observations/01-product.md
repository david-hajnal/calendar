# Product: Security hardening — recurrence bounds, key derivation, crypto

## Problem
Backend has three security weaknesses:
1. UNTIL date accepts unbounded future values (9999-12-31) causing denial-of-service via infinite expansion
2. Key derivation uses single SHA-256 with no salt/iterations — trivial brute-force target
3. Custom HMAC-based stream cipher instead of standard AES-GCM — violates crypto best practices

## Success metric
Zero custom crypto constructions remaining. All recurrence rules bounded to 5-year span and 1M iterations.

## Announcement — the blog post before the feature
We're hardening the calendar engine against recurrence-based denial-of-service attacks and upgrading our encryption to industry-standard AES-GCM. These changes make the service more resilient and secure without affecting any user-facing behavior.

## Screens
no UI — backend-only security changes
