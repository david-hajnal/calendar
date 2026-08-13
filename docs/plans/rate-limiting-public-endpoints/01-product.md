# Product: rate limiting on public endpoints

## Problem
Public endpoints (view by token, list events) are unprotected against abuse — anyone can hammer them with unlimited requests, consuming server resources and degrading service for legitimate users.

## Success metric
Zero successful requests above 100 req/min per IP on public endpoints in production, measured via rate limit 429 response count in logs.

## Announcement — the blog post before the feature
We're adding rate limiting to our public endpoints to keep things fast and reliable for everyone. If you share a public view link, it still works exactly the same. The limit is generous — 100 requests per minute — so normal use won't be affected.

## Screens
no UI — backend-only feature
