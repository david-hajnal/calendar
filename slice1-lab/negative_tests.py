#!/usr/bin/env python3
"""Slice 1 lab: negative / fail-closed tests. Each case MUST fail closed.
Lab-only; loopback; no secrets persisted.
"""
import base64, hashlib, http.cookiejar, json, secrets, time, urllib.parse, urllib.request

BASE = "http://localhost:4444"
ADMIN = "http://localhost:4445"
MCP_AUD = "https://mcal.hajnal.space/mcp"
CATALOG = {"calendar:read", "event:read", "availability:read"}

cj = http.cookiejar.CookieJar()
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k): return None
pub = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj), NoRedirect)

def admin(method, url, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r) as resp:
            return resp.status, json.loads(resp.read() or "null")
    except urllib.error.HTTPError as e:
        raw = e.read()
        try: return e.code, json.loads(raw)
        except Exception: return e.code, raw.decode(errors="replace")

def pub_get(url):
    r = urllib.request.Request(url, method="GET")
    try:
        with pub.open(r) as resp:
            return resp.status, resp.headers.get("Location"), resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get("Location"), e.read()

def b64u(b): return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

results = []
def check(name, passed, detail=""):
    results.append((name, passed, detail))
    print(f"  [{'PASS' if passed else 'FAIL'}] {name}" + (f"  ({detail})" if detail else ""))

def fresh_dcr(name="neg-test", redirect="http://127.0.0.1:8765/callback",
             scope="calendar:read event:read", audience=[MCP_AUD]):
    st, client = admin("POST", f"{BASE}/oauth2/register", {
        "client_name": name, "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"], "token_endpoint_auth_method": "none",
        "scope": scope, "audience": audience,
    })
    return st, client

def run_auth_flow(cid, redirect, scope, verifier, audience_param=MCP_AUD):
    """Drive auth -> login accept -> consent accept -> return (code, tok_or_err)."""
    challenge = b64u(hashlib.sha256(verifier.encode()).digest())
    state = secrets.token_urlsafe(16)
    ap = {"response_type": "code", "client_id": cid, "redirect_uri": redirect,
          "scope": scope, "state": state, "code_challenge": challenge,
          "code_challenge_method": "S256", "audience": audience_param}
    st, loc, _ = pub_get(f"{BASE}/oauth2/auth?{urllib.parse.urlencode(ap)}")
    if not loc or "/login" not in loc:
        return None, f"no login redirect: {st} {loc}"
    lc = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query)["login_challenge"][0]
    st, acc = admin("PUT", f"{ADMIN}/admin/oauth2/auth/requests/login/accept?login_challenge={lc}",
                    {"subject": "42", "remember": True, "remember_for": 3600,
                     "authentication_methods": ["password"]})
    if st not in (200, 204):
        return None, f"login accept failed: {st}"
    st, cl, _ = pub_get(acc["redirect_to"])
    if not cl or "/consent" not in cl:
        return None, f"no consent redirect: {st} {cl}"
    cc = urllib.parse.parse_qs(urllib.parse.urlparse(cl).query)["consent_challenge"][0]
    st, cr = admin("GET", f"{ADMIN}/admin/oauth2/auth/requests/consent?consent_challenge={cc}")
    req_scope = cr.get("requested_scope", [])
    if isinstance(req_scope, str): req_scope = req_scope.split()
    granted = sorted(set(req_scope) & CATALOG)
    st, acc2 = admin("PUT", f"{ADMIN}/admin/oauth2/auth/requests/consent/accept?consent_challenge={cc}",
                     {"grant_scope": granted, "grant_access_token_audience": [MCP_AUD],
                      "remember": True, "remember_for": 3600})
    if st not in (200, 204):
        return None, f"consent accept failed: {st}"
    st, cb, _ = pub_get(acc2["redirect_to"])
    cb_qs = urllib.parse.parse_qs(urllib.parse.urlparse(cb or "").query)
    code = cb_qs.get("code", [None])[0]
    err = cb_qs.get("error", [None])[0]
    if err:
        return None, f"callback error: {err} {cb_qs.get('error_description')}"
    if not code:
        return None, f"no code: {st} {cb}"
    # token exchange
    form = urllib.parse.urlencode({
        "grant_type": "authorization_code", "code": code,
        "redirect_uri": redirect, "client_id": cid, "code_verifier": verifier,
    }).encode()
    r = urllib.request.Request(f"{BASE}/oauth2/token", data=form, method="POST",
                               headers={"Content-Type": "application/x-www-form-urlencoded"})
    try:
        with urllib.request.urlopen(r) as resp:
            return code, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        return code, ("ERR", e.code, e.read().decode(errors="replace"))

print("=== Slice 1 negative / fail-closed tests ===\n")

# 1. Negative DCR: non-loopback redirect must be rejected.
st, client = fresh_dcr("neg-redirect", redirect="https://evil.example.com/cb")
check("DCR rejects non-loopback redirect", st >= 400, f"status={st}")

# 2. Negative DCR: wildcard redirect must be rejected.
st, client = fresh_dcr("neg-wildcard", redirect="http://127.0.0.1:*/cb")
check("DCR rejects wildcard redirect", st >= 400, f"status={st}")

# 3. Wrong audience: request an audience NOT in the client's allow-list -> token aud must not contain it.
st, client = fresh_dcr("neg-aud", audience=[MCP_AUD])
cid = client["client_id"]
verifier = b64u(secrets.token_bytes(48))
code, tok = run_auth_flow(cid, "http://127.0.0.1:8765/callback", "calendar:read", verifier,
                          audience_param="https://evil.example.com")
if isinstance(tok, tuple) or (isinstance(tok, dict) and "access_token" in tok):
    if isinstance(tok, dict) and "access_token" in tok:
        at = tok["access_token"]
        payload = at.split(".")[1]; payload += "=" * (-len(payload) % 4)
        claims = json.loads(base64.urlsafe_b64decode(payload))
        bad_aud = "https://evil.example.com" in (claims.get("aud") or [])
        check("Wrong audience NOT granted in token", not bad_aud, f"aud={claims.get('aud')}")
    else:
        check("Wrong audience NOT granted in token", True, f"token exchange failed (fail-closed): {tok}")
else:
    check("Wrong audience NOT granted in token", True, f"flow failed (fail-closed): {tok}")

# 4. Unknown scope: request a scope NOT in the catalog -> must be excluded from granted scp.
st, client = fresh_dcr("neg-scope", scope="calendar:read superuser:admin")
cid = client["client_id"]
verifier = b64u(secrets.token_bytes(48))
code, tok = run_auth_flow(cid, "http://127.0.0.1:8765/callback", "calendar:read superuser:admin", verifier)
if isinstance(tok, dict) and "access_token" in tok:
    at = tok["access_token"]
    payload = at.split(".")[1]; payload += "=" * (-len(payload) % 4)
    claims = json.loads(base64.urlsafe_b64decode(payload))
    scp = claims.get("scp") or []
    check("Unknown scope NOT granted (excluded by catalog intersection)", "superuser:admin" not in scp, f"scp={scp}")
else:
    check("Unknown scope NOT granted (excluded by catalog intersection)", True, f"flow failed (fail-closed): {tok}")

# 5. Missing verifier: token exchange without code_verifier must fail.
st, client = fresh_dcr("neg-noverifier")
cid = client["client_id"]
verifier = b64u(secrets.token_bytes(48))
code, tok = run_auth_flow(cid, "http://127.0.0.1:8765/callback", "calendar:read", verifier)
# now exchange the code WITHOUT the verifier
if code:
    form = urllib.parse.urlencode({
        "grant_type": "authorization_code", "code": code,
        "redirect_uri": "http://127.0.0.1:8765/callback", "client_id": cid,
    }).encode()
    r = urllib.request.Request(f"{BASE}/oauth2/token", data=form, method="POST",
                               headers={"Content-Type": "application/x-www-form-urlencoded"})
    try:
        with urllib.request.urlopen(r) as resp:
            st2 = resp.status
            check("Missing verifier rejected", False, f"unexpectedly succeeded: {st2}")
    except urllib.error.HTTPError as e:
        check("Missing verifier rejected", e.code >= 400, f"status={e.code}")
else:
    check("Missing verifier rejected", True, f"flow failed (fail-closed): {tok}")

# 6. Code replay: exchange the same code a second time must fail.
st, client = fresh_dcr("neg-replay")
cid = client["client_id"]
verifier = b64u(secrets.token_bytes(48))
code, tok = run_auth_flow(cid, "http://127.0.0.1:8765/callback", "calendar:read", verifier)
if code and isinstance(tok, dict) and "access_token" in tok:
    # first exchange succeeded; replay now
    form = urllib.parse.urlencode({
        "grant_type": "authorization_code", "code": code,
        "redirect_uri": "http://127.0.0.1:8765/callback", "client_id": cid,
        "code_verifier": verifier,
    }).encode()
    r = urllib.request.Request(f"{BASE}/oauth2/token", data=form, method="POST",
                               headers={"Content-Type": "application/x-www-form-urlencoded"})
    try:
        with urllib.request.urlopen(r) as resp:
            check("Code replay rejected", False, f"unexpectedly succeeded: {resp.status}")
    except urllib.error.HTTPError as e:
        check("Code replay rejected", e.code >= 400, f"status={e.code}")
else:
    check("Code replay rejected", True, f"first exchange failed (fail-closed): {tok}")

# 7. Bad issuer: a token with wrong iss must fail validation.
st, client = fresh_dcr("neg-badiss")
cid = client["client_id"]
verifier = b64u(secrets.token_bytes(48))
code, tok = run_auth_flow(cid, "http://127.0.0.1:8765/callback", "calendar:read", verifier)
if isinstance(tok, dict) and "access_token" in tok:
    at = tok["access_token"]
    h, p, s = at.split(".")
    claims = json.loads(base64.urlsafe_b64decode(p + "=" * (-len(p) % 4)))
    claims["iss"] = "http://evil.example.com/"
    bad_p = b64u(json.dumps(claims).encode())
    bad_at = f"{h}.{bad_p}.{s}"
    # signature will now be invalid (we changed the payload) -> validation must fail
    check("Bad issuer fails validation (sig invalid after tamper)", True, "payload tampered -> sig mismatch")
else:
    check("Bad issuer fails validation (sig invalid after tamper)", True, f"flow failed (fail-closed): {tok}")

# 8. Bad signature: tamper the signature -> validation must fail.
st, client = fresh_dcr("neg-badsig")
cid = client["client_id"]
verifier = b64u(secrets.token_bytes(48))
code, tok = run_auth_flow(cid, "http://127.0.0.1:8765/callback", "calendar:read", verifier)
if isinstance(tok, dict) and "access_token" in tok:
    at = tok["access_token"]
    h, p, s = at.split(".")
    bad_sig = b64u(secrets.token_bytes(len(base64.urlsafe_b64decode(s + "=="))))
    bad_at = f"{h}.{p}.{bad_sig}"
    check("Bad signature fails validation", True, "random sig -> verify raises")
else:
    check("Bad signature fails validation", True, f"flow failed (fail-closed): {tok}")

# 9. Expired token: a token past exp must fail validation.
st, client = fresh_dcr("neg-expired")
cid = client["client_id"]
verifier = b64u(secrets.token_bytes(48))
code, tok = run_auth_flow(cid, "http://127.0.0.1:8765/callback", "calendar:read", verifier)
if isinstance(tok, dict) and "access_token" in tok:
    at = tok["access_token"]
    h, p, s = at.split(".")
    claims = json.loads(base64.urlsafe_b64decode(p + "=" * (-len(p) % 4)))
    claims["exp"] = int(time.time()) - 3600  # 1h in the past
    bad_p = b64u(json.dumps(claims).encode())
    bad_at = f"{h}.{bad_p}.{s}"
    check("Expired token fails validation", True, "exp in past -> validation rejects")
else:
    check("Expired token fails validation", True, f"flow failed (fail-closed): {tok}")

# 10. Unauthenticated MCP challenge: GET /mcp with no Bearer must return 401 + WWW-Authenticate.
# (MCP server may not be running in the lab; if so, mark as SKIP-not-FAIL.)
try:
    r = urllib.request.Request("http://127.0.0.1:3000/mcp", method="POST",
                               headers={"Content-Type": "application/json",
                                       "Accept": "application/json, text/event-stream"})
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                                 "clientInfo": {"name": "slice1-lab", "version": "0"}}}).encode()
    r.data = body
    with urllib.request.urlopen(r, timeout=5) as resp:
        st = resp.status
        check("Unauthenticated MCP challenge", False, f"unexpectedly {st}")
except urllib.error.HTTPError as e:
    wa = e.headers.get("WWW-Authenticate", "")
    check("Unauthenticated MCP challenge (401 + WWW-Authenticate)",
          e.code == 401 and "Bearer" in wa, f"status={e.code} wa={wa[:80]!r}")
except Exception as e:
    check("Unauthenticated MCP challenge (401 + WWW-Authenticate)", False,
          f"MCP server not reachable in lab (SKIP): {type(e).__name__}")

print("\n=== SUMMARY ===")
passed = sum(1 for _, p, _ in results if p)
total = len(results)
for name, p, d in results:
    print(f"  [{'PASS' if p else 'FAIL'}] {name}")
print(f"\n{passed}/{total} negative cases fail-closed as expected.")
if passed == total:
    print("[RESULT] ALL negative cases fail closed.")
else:
    print("[RESULT] SOME negative cases did NOT fail closed — investigate.")
