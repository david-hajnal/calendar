#!/usr/bin/env python3
"""Slice 1 lab: drive Hydra auth-code + S256 PKCE flow with a session cookie jar,
accept login/consent via the admin API (as a stub adapter would), and inspect the
JWT access-token claims. Lab-only; loopback; no secrets persisted.
"""
import base64, hashlib, http.cookiejar, json, secrets, urllib.parse, urllib.request

# Use `localhost` (not 127.0.0.1) for ALL Hydra requests. Hydra's issuer is
# http://localhost:4444/ and its redirect_to URLs use `localhost`. The session
# cookie is scoped to the request host, so mixing 127.0.0.1 and localhost drops
# the CSRF cookie and the consent step fails with "No CSRF value available".
BASE = "http://localhost:4444"
ADMIN = "http://localhost:4445"
MCP_AUD = "https://mcal.hajnal.space/mcp"
TEST_SUB = "42"
CATALOG = {"calendar:read", "event:read", "availability:read"}

cj = http.cookiejar.CookieJar()
# Public API opener: no auto-redirect (we capture Location), keeps cookies.
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k): return None
pub = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj), NoRedirect)

def admin(method, url, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r) as resp:
            raw = resp.read()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try: return e.code, json.loads(raw)
        except Exception: return e.code, raw.decode(errors="replace")

def pub_get(url):
    """GET a public URL; return (status, location_header_or_None, body)."""
    r = urllib.request.Request(url, method="GET")
    try:
        with pub.open(r) as resp:
            return resp.status, resp.headers.get("Location"), resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get("Location"), e.read()

def b64url(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

def main():
    # 1. DCR
    # `audience` sets the client's allowed audiences -> becomes the JWT `aud` claim.
    st, client = admin("POST", f"{BASE}/oauth2/register", {
        "client_name": "slice1-lab-tracer",
        "redirect_uris": ["http://127.0.0.1:8765/callback"],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "scope": " ".join(sorted(CATALOG)),
        "audience": [MCP_AUD],
    })
    assert st in (200, 201), f"DCR failed: {st} {client}"
    cid = client["client_id"]
    print(f"[ok] DCR client_id={cid}")
    print(f"[debug] DCR audience={client.get('audience')!r} scope={client.get('scope')!r}")

    # 2. PKCE S256
    verifier = b64url(secrets.token_bytes(48))
    challenge = b64url(hashlib.sha256(verifier.encode()).digest())
    state = secrets.token_urlsafe(16)

    # 3. Authorization request -> 302 to login (sets session cookie)
    auth_params = {
        "response_type": "code", "client_id": cid,
        "redirect_uri": "http://127.0.0.1:8765/callback",
        "scope": " ".join(sorted(CATALOG)), "state": state,
        "code_challenge": challenge, "code_challenge_method": "S256",
        "audience": MCP_AUD,
    }
    auth_url = f"{BASE}/oauth2/auth?{urllib.parse.urlencode(auth_params)}"
    st, login_loc, _ = pub_get(auth_url)
    assert login_loc and "/login" in login_loc, f"no login redirect: {st} {login_loc}"
    login_challenge = urllib.parse.parse_qs(urllib.parse.urlparse(login_loc).query)["login_challenge"][0]
    print(f"[ok] login redirect (session cookie captured={len(cj)==1 or len(list(cj))>0})")

    # 4. Accept login via admin API (stub)
    st, acc = admin("PUT", f"{ADMIN}/admin/oauth2/auth/requests/login/accept?login_challenge={login_challenge}", {
        "subject": TEST_SUB, "remember": True, "remember_for": 3600,
        "authentication_methods": ["password"],
    })
    assert st in (200, 204), f"accept login failed: {st} {acc}"
    print(f"[ok] login accepted (sub={TEST_SUB})")

    # 5. Follow redirect_to -> 302 to consent (needs session cookie)
    st, consent_loc, _ = pub_get(acc["redirect_to"])
    assert consent_loc and "/consent" in consent_loc, f"no consent redirect: {st} {consent_loc}"
    consent_challenge = urllib.parse.parse_qs(urllib.parse.urlparse(consent_loc).query)["consent_challenge"][0]
    print(f"[ok] consent redirect")

    # 6. Accept consent via admin API (stub) — intersection + MCP audience
    st, cr = admin("GET", f"{ADMIN}/admin/oauth2/auth/requests/consent?consent_challenge={consent_challenge}")
    assert st == 200, f"consent request failed: {st} {cr}"
    scope_field = cr.get("requested_scope")
    if isinstance(scope_field, list):
        requested = set(scope_field)
    else:
        requested = set((scope_field or "").split())
    granted = sorted(requested & CATALOG)
    print(f"[info] requested_scope={scope_field!r} granted(intersection)={granted}")
    # Hydra's AcceptOAuth2ConsentRequest uses `grant_scope` and
    # `grant_access_token_audience` (NOT `scope`/`audience`). Wrong names are
    # silently ignored -> empty `scp`/`aud` JWT claims.
    st, acc2 = admin("PUT", f"{ADMIN}/admin/oauth2/auth/requests/consent/accept?consent_challenge={consent_challenge}", {
        "grant_scope": granted, "grant_access_token_audience": [MCP_AUD],
        "remember": True, "remember_for": 3600,
    })
    print(f"[debug] consent accept -> {st} redirect_to={'yes' if (isinstance(acc2,dict) and acc2.get('redirect_to')) else 'NO'}")
    if isinstance(acc2, dict):
        print(f"[debug] consent accept body keys={list(acc2.keys())}")
    assert st in (200, 204), f"accept consent failed: {st} {acc2}"
    print(f"[ok] consent accepted (audience={MCP_AUD})")

    # 7. Follow the consent accept's redirect_to -> loopback callback with the code.
    st, cb_loc, _ = pub_get(acc2["redirect_to"])
    cb_qs = urllib.parse.parse_qs(urllib.parse.urlparse(cb_loc or "").query)
    code = cb_qs.get("code", [None])[0]
    assert code, f"no code in callback: {st} {cb_loc}"
    print(f"[ok] callback code captured")

    # 8. Token exchange with PKCE verifier (form-encoded, per RFC 6749)
    form = urllib.parse.urlencode({
        "grant_type": "authorization_code", "code": code,
        "redirect_uri": "http://127.0.0.1:8765/callback",
        "client_id": cid, "code_verifier": verifier,
    }).encode()
    r = urllib.request.Request(f"{BASE}/oauth2/token", data=form, method="POST",
                               headers={"Content-Type": "application/x-www-form-urlencoded"})
    try:
        with urllib.request.urlopen(r) as resp:
            st, tok = resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        st, tok = e.code, json.loads(e.read().decode() or "null")
    assert st == 200, f"token exchange failed: {st} {tok}"
    at = tok.get("access_token", "")
    print(f"[ok] token exchange; access_token is JWT={at.count('.')==2} refresh={bool(tok.get('refresh_token'))}")

    # 9. Decode + inspect JWT claims (shape only; no signature verification here)
    payload = at.split(".")[1]; payload += "=" * (-len(payload) % 4)
    claims = json.loads(base64.urlsafe_b64decode(payload))
    print("\n=== JWT access-token claims ===")
    print(json.dumps(claims, indent=2, sort_keys=True))

    # 9b. JWKS signature verification (discovery-advertised jwks_uri).
    disc = json.loads(pub_get(f"{BASE}/.well-known/openid-configuration")[2])
    jwks_uri = disc["jwks_uri"]
    jwks = json.loads(pub_get(jwks_uri)[2])
    header = json.loads(base64.urlsafe_b64decode(at.split(".")[0] + "=="))
    kid = header.get("kid")
    key = next(k for k in jwks["keys"] if k.get("kid") == kid)
    from cryptography.hazmat.primitives.asymmetric import padding as _pad, rsa as _rsa
    from cryptography.hazmat.primitives import hashes
    def _b64u_int(s):
        return int.from_bytes(base64.urlsafe_b64decode(s + "=" * (-len(s) % 4)), "big")
    n = _b64u_int(key["n"])
    e = _b64u_int(key["e"])
    pub = _rsa.RSAPublicNumbers(e, n).public_key()
    signing_input = f"{at.split('.')[0]}.{at.split('.')[1]}".encode()
    sig = base64.urlsafe_b64decode(at.split(".")[2] + "=" * (-len(at.split(".")[2]) % 4))
    pub.verify(sig, signing_input, _pad.PKCS1v15(), hashes.SHA256())
    print(f"[ok] JWKS signature verified (kid={kid}, alg={header.get('alg')})")

    # 10. Assertions on the required claim shape (per 02-architecture.md:
    # sub, client_id, scope, aud). The scope claim in a JWT access token is `scp`.
    checks = {
        "iss present": "iss" in claims,
        "aud == [MCP resource]": claims.get("aud") in ([MCP_AUD], MCP_AUD),
        "sub numeric (CommonCal user id)": str(claims.get("sub", "")).isdigit(),
        "client_id present": "client_id" in claims,
        "scp (scope) present + non-empty": bool(claims.get("scp")),
        "jti present": "jti" in claims,
        "iat present": "iat" in claims,
        "exp present": "exp" in claims,
    }
    print("\n=== claim-shape assertions ===")
    for k, v in checks.items():
        print(f"  [{'PASS' if v else 'FAIL'}] {k}")
    print("\n[RESULT]", "ALL required JWT claims present." if all(checks.values()) else "SOME required claims MISSING — see above.")

if __name__ == "__main__":
    main()
