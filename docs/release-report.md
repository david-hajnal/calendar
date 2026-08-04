# Production readiness report — 2026-08-04

## Release decision

**Do not release to the Internet yet.** Local implementation checks listed below
passed, but the unresolved production blocks in `docs/threat-model.md` have not
been accepted or closed. In particular, production TLS/HSTS deployment,
session-secret rotation, encryption at rest/key custody, production email, and
(when feeds are enabled) egress controls still require operator evidence.

## Evidence collected

| Check | Result | Evidence |
| --- | --- | --- |
| Rust formatting | Pass | `cargo fmt --manifest-path backend/Cargo.toml`; `git diff --check` passed. |
| Backend tests | Pass | `CARGO_TARGET_DIR=/tmp/commoncal-p38.0ePS6X cargo test --manifest-path backend/Cargo.toml --locked`: all integration tests passed. |
| Authorization regression | Pass | 4/4 tests passed in `authorization_regression`. |
| Backup/restore | Pass (test drill) | 11/11 tests passed in `backup`, including encrypted artifact creation, clean restore, integrity/corruption rejection, and representative-record recovery. No operator-managed production backup/restore drill was run. |
| Rust lint | Pass | `cargo clippy --all-targets --locked -- -D warnings` passed using the isolated target directory. |
| Dependency resolution | Pass | `cargo tree --locked --edges normal` completed. |
| Cargo audit | Not run | `cargo audit` is not installed; no dependency vulnerability claim is made. No installation/download was attempted. |
| Yarn lockfiles | Pass | `scripts/check-no-yarn-lock.sh` and its regression test passed through `make check`; no `yarn.lock` or Yarn command was accepted. |
| Frontend lint/typecheck/tests/build | Pass | `make check` passed lint, typecheck, 45/45 Vitest tests, and the Vite production build. |
| Playwright | Pending / configuration inspected | `e2e/playwright.config.ts` defines desktop and mobile projects and a local web server. The prior `pnpm e2e` zero-output/zero-exit observation is not evidence of a passing run; likely the package-manager invocation did not emit the nested script output. Run `pnpm --dir e2e test -- --list` and then the full command with installed browsers before release. |
| Docker image build/inspection | Not run | No Docker command was attempted under the bounded gate; image evidence is required before release. |
| Helm lint/template tests | Not run | Helm evidence is required before release. |

## Small correction applied

`cargo fmt` reformatted the current Rust changes. No other release-only source
correction was applied. The worktree remains intentionally dirty and no files
were staged or committed.

## Risks and acceptance status

**Unaccepted release blocks:** encrypted backups by default plus key custody,
rotation and retention; production ingress TLS with HTTPS `APP_ORIGIN` and
verified HSTS; production email sender/provider controls; high-entropy session
secret with tested rotation; SQLite PVC and backup-destination encryption at
rest; and feed egress controls if external feeds are enabled. These are the
explicit blocks in `docs/threat-model.md` and must be closed or formally
accepted by the release authority.

**Accepted only for this local verification:** isolated test artifacts were
created under `/tmp`; they are not production backup evidence. `cargo audit`,
E2E, Docker, and Helm remain incomplete, not passed.

## Recovery assumptions

No production RPO/RTO is demonstrated by this repository. For planning only,
set **RPO to the successful encrypted-backup interval** and **RTO to the
measured time to provision a replacement PVC/pod, restore, run SQLite
integrity verification, and pass readiness**. Do not publish an RPO or RTO
number until an operator runs and records an encrypted restore drill against
the chosen storage, key-management, and deployment environment.
