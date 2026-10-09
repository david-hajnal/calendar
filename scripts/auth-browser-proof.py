#!/usr/bin/env python3
"""Manual production OAuth canary. Token files are private; tokens are never printed."""
import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import time
import urllib.parse
import urllib.request
import urllib.error
import webbrowser


def request(url, data=None, form=False):
    # Compact JSON also works with older auth images that replay registration bodies.
    body = None if data is None else (urllib.parse.urlencode(data).encode() if form else json.dumps(data, separators=(",", ":")).encode())
    headers = {"User-Agent": "commoncal-recovery-proof/1.0", "Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/x-www-form-urlencoded" if form else "application/json"
    try:
        with urllib.request.urlopen(urllib.request.Request(url, data=body, headers=headers), timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        endpoint = urllib.parse.urlsplit(url).path
        print(f"HTTP failure: {endpoint} returned {error.code}")
        # Only print a recognized protocol error, never descriptions or raw bodies.
        try:
            protocol_error = json.loads(error.read(8192)).get("error")
            if protocol_error in ("invalid_request", "invalid_client", "invalid_grant", "invalid_scope", "invalid_target", "unauthorized_client", "unsupported_grant_type", "invalid_redirect_uri", "invalid_client_metadata", "access_denied", "temporarily_unavailable"):
                print(f"OAuth error: {protocol_error}")
        except (ValueError, AttributeError, TypeError):
            pass
        raise


def save(path, data):
    # A new private directory is required, to avoid overwriting other token files.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(data, output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    args.directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    issuer = "https://auth.hajnal.space"
    resource = "https://mcal.hajnal.space/mcp"
    metadata = request(issuer + "/.well-known/oauth-authorization-server")
    if metadata.get("issuer") != issuer:
        raise RuntimeError("Unexpected issuer")
    for key in ("registration_endpoint", "authorization_endpoint", "token_endpoint"):
        if urllib.parse.urlsplit(metadata[key]).scheme != "https" or urllib.parse.urlsplit(metadata[key]).netloc != "auth.hajnal.space":
            raise RuntimeError("Unexpected OAuth endpoint")
    state = secrets.token_urlsafe(32)
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
    result = {}

    class Callback(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            parsed = urllib.parse.urlsplit(self.path)
            query = urllib.parse.parse_qs(parsed.query)
            valid = parsed.path == "/mcp/oauth/callback" and query.get("state") == [state]
            if valid:
                result.update(query)
            self.send_response(200 if valid else 400)
            self.end_headers()
            self.wfile.write(b"OAuth callback received. Return to your terminal." if valid else b"Invalid callback.")

    with http.server.HTTPServer(("127.0.0.1", 0), Callback) as server:
        redirect = f"http://127.0.0.1:{server.server_port}/mcp/oauth/callback"
        client = request(metadata["registration_endpoint"], {
            "client_name": "CommonCal recovery proof", "redirect_uris": [redirect],
            "token_endpoint_auth_method": "none", "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
        })
        url = metadata["authorization_endpoint"] + "?" + urllib.parse.urlencode({
            "client_id": client["client_id"], "redirect_uri": redirect, "response_type": "code",
            "scope": "openid offline_access commoncal.event.read.basic", "resource": resource,
            "code_challenge": challenge, "code_challenge_method": "S256", "state": state,
            "prompt": "consent",
        })
        print("Open this URL on this same laptop; sign in with a disposable test account and approve consent:\n" + url)
        webbrowser.open(url)
        server.timeout = 1
        deadline = time.monotonic() + 600
        while not result and time.monotonic() < deadline:
            server.handle_request()
        if not result.get("code") or result.get("error"):
            raise RuntimeError("OAuth did not return an authorization code")
    tokens = request(metadata["token_endpoint"], {
        "grant_type": "authorization_code", "client_id": client["client_id"], "code": result["code"][0],
        "code_verifier": verifier, "redirect_uri": redirect, "resource": resource,
    }, form=True)
    if not tokens.get("access_token") or not tokens.get("refresh_token"):
        raise RuntimeError("Expected both access and refresh tokens")
    save(args.directory / "proof.json", {"issuer": issuer, "resource": resource, "client_id": client["client_id"], "tokens": tokens})
    print("PASS: browser login, consent, PKCE exchange, and refresh-token issuance. Private proof.json saved.")
    print("Take a fresh encrypted backup now. Do not refresh or revoke this token before the isolated restore test.")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # HTTP bodies and callback query strings may contain credentials.
        print(f"FAIL: {type(error).__name__}. No token values printed.")
        raise SystemExit(1)
