#!/usr/bin/env python3
"""Read-only MCP host/auth regression check. Never displays response bodies or tokens.

Use --proof-file with the private proof.json from auth-browser-proof.py, or
--token-file with a private file containing only an unexpired access token.
No tools/call request is made. A missing token runs negative checks only and
exits 2, so it cannot be mistaken for successful authenticated verification.
"""

import argparse
import http.client
import json
import stat
import sys
from pathlib import Path


HOST = "mcal.hajnal.space"
RESOURCE = f"https://{HOST}/mcp"
METADATA = f"https://{HOST}/.well-known/oauth-protected-resource"


def request(message, token=None, host=HOST, session=None, protocol=None):
    headers = {
        "Host": host,
        "User-Agent": "commoncal-deployment-check/1.0",
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if token is not None:
        headers["Authorization"] = f"Bearer {token}"
    if session:
        headers["Mcp-Session-Id"] = session
    if protocol:
        headers["MCP-Protocol-Version"] = protocol
    connection = http.client.HTTPSConnection(HOST, timeout=20)
    try:
        connection.request("POST", "/mcp", json.dumps(message), headers)
        response = connection.getresponse()
        # Bound response reads. Do not log headers, response bodies or errors.
        body = response.read(2_000_001)
        if len(body) > 2_000_000:
            raise ValueError("response exceeds limit")
        return response.status, dict(response.getheaders()), body
    finally:
        connection.close()


def rpc_result(status, body, expected_id):
    if status != 200:
        raise ValueError("MCP request did not return HTTP 200")
    try:
        messages = [json.loads(body)]
    except (ValueError, UnicodeError):
        # A POST SSE response can contain notifications before its final result.
        messages = []
        for event in body.decode().replace("\r\n", "\n").split("\n\n"):
            data = "\n".join(line[5:].lstrip() for line in event.splitlines()
                             if line.startswith("data:"))
            if data:
                messages.append(json.loads(data))
    for message in messages:
        if (isinstance(message, dict) and message.get("id") == expected_id
                and message.get("jsonrpc") == "2.0"
                and "result" in message and "error" not in message):
            return message["result"]
    raise ValueError("missing successful JSON-RPC result")


def initialize():
    return {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                   "clientInfo": {"name": "public-host-regression", "version": "1"}},
    }


def private_token(args):
    path = args.proof_file or args.token_file
    if path is None:
        return None
    mode = path.stat().st_mode
    if not stat.S_ISREG(mode) or mode & 0o077:
        raise ValueError("token input must be a private regular file")
    if args.proof_file:
        proof = json.loads(path.read_text())
        if proof.get("resource") != RESOURCE or proof.get("issuer") != "https://auth.hajnal.space":
            raise ValueError("proof issuer/resource mismatch")
        token = proof["tokens"]["access_token"]
    else:
        token = path.read_text().strip()
    if not isinstance(token, str) or not token or any(c.isspace() for c in token):
        raise ValueError("invalid token input")
    return token


def check(args):
    token = private_token(args)
    listing = {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}
    for message in (initialize(), listing):
        for label, invalid_token in (("missing", None), ("invalid", "invalid-regression-token")):
            status, headers, _ = request(message, invalid_token)
            challenge = next((v for k, v in headers.items() if k.lower() == "www-authenticate"), "")
            if status != 401 or f'resource_metadata="{METADATA}"' not in challenge:
                raise ValueError("authentication rejection/challenge mismatch")
            print(f"PASS: {message['method']} {label} token rejected (HTTP 401)")
    if token is None:
        print("INCOMPLETE: authenticated checks require a private token/proof file.")
        return 2
    for host in (HOST, f"{HOST}:443"):
        status, headers, body = request(initialize(), token, host)
        print(f"CHECK: authenticated initialize for {host} returned HTTP {status}", flush=True)
        if status == 403 and body.strip() == b"Forbidden: Host header is not allowed":
            print("FAIL: transport still rejects the public Host; verify deployed image and proxy forwarding.", flush=True)
        result = rpc_result(status, body, 1)
        protocol = result["protocolVersion"]
        session = next((v for k, v in headers.items() if k.lower() == "mcp-session-id"), None)
        print(f"PASS: authenticated initialize for {host}")
        status, _, _ = request({"jsonrpc": "2.0", "method": "notifications/initialized"},
                               token, host, session, protocol)
        print(f"CHECK: initialized notification returned HTTP {status}", flush=True)
        if status not in (200, 202, 204):
            raise ValueError("initialization notification rejected")
        status, _, body = request(listing, token, host, session, protocol)
        print(f"CHECK: authenticated tools/list for {host} returned HTTP {status}", flush=True)
        result = rpc_result(status, body, 2)
        if not isinstance(result.get("tools"), list):
            raise ValueError("tools/list result missing tools")
        print(f"PASS: authenticated tools/list for {host}")
    for host in ("unexpected-host.invalid", f"{HOST}.unexpected-host.invalid"):
        status, _, _ = request(initialize(), token, host)
        # Ingress may reject an unmatched host before the transport sees it.
        if status not in (400, 403, 404, 421):
            raise ValueError("unexpected host was not rejected")
        print(f"PASS: unexpected public Host rejected (HTTP {status})")
    print("PASS: public read-only MCP host/auth regression checks; no calendar tools called.")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--proof-file", type=Path)
    source.add_argument("--token-file", type=Path)
    args = parser.parse_args()
    try:
        return check(args)
    except Exception:
        # Exceptions from HTTP/JSON parsing may contain sensitive server data.
        print("FAIL: MCP regression check failed; response details suppressed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
