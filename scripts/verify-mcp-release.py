#!/usr/bin/env python3
"""Run slice 15 proofs without deploying or accepting unrun checks as passes.

Exit 1: an automated check failed. Exit 2: required proof is blocked, missing or
skipped. This command always leaves manual live-client/recovery proof pending;
it cannot alone authorize release. No credentials are inputs.
"""

import argparse
from collections import Counter
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[1]
AUTH = ROOT / "slice1-lab/auth-server"


def check(name, command, cwd=ROOT, requires=(), env=None):
    return {"name": name, "command": command, "cwd": cwd,
            "requires": requires, "env": env or {}}


def checks(image, live):
    result = [
        check("auth-unit", ["node", "--test", *[str(p.relative_to(AUTH)) for p in sorted((AUTH / "test").glob("*.test.mjs"))]], AUTH),
        check("auth-contract-restart-restore", ["./scripts/dev.sh", "auth-check"]),
        check("auth-encrypted-storage", ["node", "tests/sqlite-storage-proof.mjs"], AUTH, ("age", "age-keygen")),
        check("backend-login-consent-grants", ["cargo", "test", "--manifest-path", "backend/Cargo.toml", "--locked"]),
        check("authorization-coverage", ["make", "validate-authorization-coverage"]),
        check("mcp-protocol-e2e", ["cargo", "test", "--manifest-path", "mcp-server/Cargo.toml", "--locked"]),
    ]
    for verifier in ["verify", "verify-discovery", "verify-oauth-lifecycle", "verify-security-isolation"]:
        result.append(check(f"black-box-{verifier}", ["node", f"mcp-acceptance/{verifier}.js"]))
    result += [
        check("helm-flux", ["bash", "scripts/validate-deploy.sh"], requires=("helm", "kubectl", "python3")),
        check("cutover-manifests", ["python3", "scripts/test-auth-cutover-manifests.py"], requires=("helm", "kubectl")),
        check("auth-tls", ["python3", "scripts/test-auth-tls.py"], requires=("openssl",)),
        check("immutable-production-tags", ["make", "check-prod-tags"]),
        check("deploy-contract", ["make", "deploy-script-test"]),
        check("core-sqlite-recovery", ["make", "sqlite-prod-test"]),
        check("mcp-sqlite-recovery", ["sh", "scripts/test-mcp-backup-restore.sh"], requires=("sqlite3",)),
        check("public-host-checker", ["python3", "scripts/test-check-mcp-public-host.py"]),
    ]
    for client in ["codex", "claude", "opencode"]:
        result.append(check(f"install-{client}", ["node", "mcp-acceptance/client-snippets.js", client], requires=(client,)))
    if image:
        result.append(check("auth-node22-image-storage", ["./scripts/dev.sh", "auth-storage-check"], requires=("docker",)))
    if live:
        result.append(check("production-discovery", ["node", "mcp-acceptance/discovery.js"], env={
            "MCP_URL": "https://mcal.hajnal.space/mcp", "MCP_OAUTH_ISSUER": "https://auth.hajnal.space"}))
    return result


def execute(spec, directory, timeout):
    name = spec["name"]
    record = {"name": name, "command": shlex.join(spec["command"]),
              "cwd": str(spec["cwd"].relative_to(ROOT)) or ".", "environment": spec["env"]}
    missing = [tool for tool in (spec["command"][0], *spec["requires"])
               if not (os.access(spec["cwd"] / tool, os.X_OK) if "/" in tool else shutil.which(tool))]
    if missing:
        return {**record, "status": "blocked", "reason": "Missing tools: " + ", ".join(missing)}
    print(f"RUN {name}", flush=True)
    logfile = directory / f"{name}.log"
    started = time.monotonic()
    env = {**os.environ, **spec["env"], "MCP_RECORD_DIR": str(directory)}
    try:
        with logfile.open("w") as stream:
            process = subprocess.Popen(spec["command"], cwd=spec["cwd"], env=env,
                                       stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = process.wait(timeout=timeout)
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                raise
        output = logfile.read_text(errors="replace")
        skipped = bool(re.search(r"^\s*SKIP:", output, re.MULTILINE))
        status = "failed" if code else "blocked" if skipped else "passed"
        record.update(status=status, exit_code=code, log=logfile.name)
        if skipped:
            record["reason"] = "Child command skipped a check; inspect log"
    except subprocess.TimeoutExpired:
        record.update(status="failed", reason=f"Timed out after {timeout}s", log=logfile.name)
    except OSError as error:
        record.update(status="blocked", reason=type(error).__name__, log=logfile.name)
    record["seconds"] = round(time.monotonic() - started, 2)
    print(f"{record['status'].upper()} {name} ({record['seconds']}s)", flush=True)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, help="New directory for JSON report and logs")
    parser.add_argument("--with-image", action="store_true", help="Build/test disposable auth image through scripts/dev.sh")
    parser.add_argument("--live-discovery", action="store_true", help="Read-only production discovery probe; no OAuth login or tool call")
    parser.add_argument("--timeout", type=int, default=1200, help="Per-command timeout in seconds")
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("--timeout must be positive")
    timestamp = datetime.now(timezone.utc)
    directory = (args.output_dir or ROOT / ".agent/runs/mcp-connector-slice-15" /
                 timestamp.strftime("release-%Y%m%dT%H%M%S%fZ")).resolve()
    directory.mkdir(parents=True, exist_ok=False, mode=0o700)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    report = {"slice": 15, "started_at": timestamp.isoformat(), "revision": revision,
              "dirty_worktree": dirty, "deployment_performed": False, "checks": []}
    def save():
        counts = Counter(item["status"] for item in report["checks"])
        report["counts"] = dict(counts)
        report["status"] = "failed" if counts["failed"] else "blocked" if counts["blocked"] else "passed"
        (directory / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    # Keep these explicit: automated mocks and local restores cannot satisfy them.
    pending = [
        ("live-production-recovery", "Operator proof of live restart, clean-target backup restore, key rotation and rollback required"),
        *[(f"live-{client}-matrix", "Released-client browser consent, real calendar read, refresh, deny/revoke/remove and recovery proof required")
          for client in ["codex", "claude", "opencode"]],
    ]
    if not args.with_image:
        pending.append(("auth-node22-image-storage", "Rerun with --with-image"))
    if not args.live_discovery:
        pending.append(("production-discovery", "Rerun with --live-discovery"))
    report["checks"] = [{"name": name, "status": "blocked", "reason": reason} for name, reason in pending]
    save()
    for spec in checks(args.with_image, args.live_discovery):
        report["checks"].append(execute(spec, directory, args.timeout))
        save()
    report["finished_at"] = datetime.now(timezone.utc).isoformat()
    save()
    print(f"Report: {directory / 'report.json'}\n{report['status'].upper()}: {report['counts']}", flush=True)
    return 1 if report["status"] == "failed" else 2 if report["status"] == "blocked" else 0


if __name__ == "__main__":
    sys.exit(main())
