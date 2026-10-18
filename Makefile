.PHONY: all authorization-regression backend-check backend-test check check-no-yarn check-prod-tags ci-script-test deploy deploy-script-test docker-build docker-build-push docker-check docker-push e2e frontend-build frontend-test lab-verify lint mcp-check mcp-test sqlite-prod-test trivy-check validate-authorization-coverage

all: check

backend-check:
	cargo check --manifest-path backend/Cargo.toml --locked

backend-test:
	cargo test --manifest-path backend/Cargo.toml --locked

mcp-check:
	cargo check --manifest-path mcp-server/Cargo.toml --locked

mcp-test:
	cargo test --manifest-path mcp-server/Cargo.toml --locked

.PHONY: mcp-release-proof
mcp-release-proof:
	python3 scripts/test-verify-mcp-release.py
	python3 scripts/verify-mcp-release.py

validate-authorization-coverage:
	sh scripts/validate-authorization-coverage-report.sh

authorization-regression: validate-authorization-coverage
	cargo test --manifest-path backend/Cargo.toml --locked --test authorization_regression

frontend-test:
	pnpm --dir frontend test

lint:
	cargo fmt --manifest-path backend/Cargo.toml --check
	cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
	pnpm --dir frontend lint
	pnpm --dir frontend typecheck

frontend-build:
	pnpm --dir frontend build

e2e:
	pnpm e2e

check-no-yarn:
	sh scripts/check-no-yarn-lock.sh

ci-script-test:
	sh scripts/test-check-no-yarn-lock.sh

check-prod-tags:
	@echo "==> Checking production manifests for immutable tags..."
	@if grep -rn 'tag:.*latest' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null; then \
		echo "ERROR: Found 'latest' tag in production manifests"; exit 1; fi
	@if grep -rn 'tag:.*"main"' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null; then \
		echo "ERROR: Found 'main' tag in production manifests"; exit 1; fi
	@test "$$(grep -hE '^[[:space:]]*tag: "sha-[0-9a-f]{40}"$$' deploy/flux/overlays/production/charts/*-helmrelease.yaml | wc -l | tr -d ' ')" -eq 3 || \
		{ echo "ERROR: Expected three immutable sha-<40 hex commit> production tags"; exit 1; }

deploy-script-test:
	sh scripts/test-deploy-prod.sh

sqlite-prod-test:
	sh scripts/test-sqlite-prod.sh

check: check-no-yarn ci-script-test check-prod-tags backend-check backend-test authorization-regression mcp-check mcp-test frontend-test lint frontend-build deploy-script-test sqlite-prod-test docker-check trivy-check

# Production deployment. Requires:
#   SESSION_SECRET        - session encryption key
#   BACKUP_ENCRYPTION_KEY_HEX - hex-encoded backup encryption key
# Required: IMAGE_TAG. Optional: DOMAIN, NAMESPACE, HELM_RELEASE_NAME, DRY_RUN=1
deploy:
	deploy/deploy-prod.sh

docker-build:
	scripts/docker-build-push.sh --build-only

docker-push:
	scripts/docker-build-push.sh

docker-build-push:
	scripts/docker-build-push.sh

docker-check:
	scripts/docker-build-push.sh --build-only
	IMAGE_NAME=calendar-mcp LOCAL_TAG=commoncal-mcp:local DOCKERFILE=Dockerfile.mcp scripts/docker-build-push.sh --build-only
	IMAGE_NAME=calendar-auth LOCAL_TAG=commoncal-auth:local DOCKERFILE=Dockerfile.auth scripts/docker-build-push.sh --build-only

trivy-check: docker-check
	@if command -v trivy >/dev/null 2>&1; then \
		echo "==> Scanning images for CRITICAL/HIGH vulnerabilities..."; \
		trivy image --severity CRITICAL,HIGH --ignore-unfixed --exit-code 1 commoncal-auth:local && \
		trivy image --severity CRITICAL,HIGH --ignore-unfixed --exit-code 1 commoncal:local && \
		trivy image --severity CRITICAL,HIGH --ignore-unfixed --exit-code 1 commoncal-mcp:local; \
	elif [ "$${SKIP_TRIVY:-0}" = "1" ]; then \
		echo "SKIP_TRIVY=1: skipping Trivy scan"; \
	else \
		echo "ERROR: trivy is not installed. Install trivy or set SKIP_TRIVY=1 to skip." >&2; \
		exit 1; \
	fi

# Focused local verification for the slice1-lab (cross-client MCP connector).
# Starts PostgreSQL, builds the lab binaries, and runs the full proof harness.
lab-verify:
	docker compose -f slice1-lab/compose.yaml up -d postgres
	@echo "==> Waiting for PostgreSQL..."
	@for i in $$(seq 1 30); do \
		if docker compose -f slice1-lab/compose.yaml exec -T postgres pg_isready -U oidc >/dev/null 2>&1; then \
			break; \
		fi; \
		sleep 1; \
	done
	cargo build --manifest-path slice1-lab/Cargo.toml --bin mcp-echo --bin lab-prove
	slice1-lab/target/debug/lab-prove

# Production diagnosis commands.
# prod-render: Render effective production workloads from the committed
# Kustomize overlay, which is the single source of truth for what Flux applies.
# Requires: kustomize (or kubectl with kustomize plugin).
# Offline: renders from committed files without kubeconfig or cluster access.
# Any rendering failure is fatal — this surfaces defects that alternate
# --values files previously masked (mail, image, issuer settings, etc.).
prod-render:
	@echo "==> Rendering production overlay (effective HelmRelease manifests)..."
	@bash scripts/render-prod-offline.sh || \
		{ echo "ERROR: helm template failed — production config is invalid"; exit 1; }
	@echo "==> Done. Review the manifests above; they reflect what Flux will apply."

# prod-check: Validate production config against invariants.
# Offline: validates from committed files without kubeconfig or cluster access.
prod-check:
	@echo "==> Running production validation..."
	@bash scripts/validate-deploy.sh
	@echo "==> Done."

# prod-local-up: Start the local integration stack.
# Requires: Docker, docker-compose.
# Starts (via scripts/dev.sh start):
#   - app          : backend API + served frontend (port 3000)
#   - frontend-dev : Vite dev server (port 5173)
# NOT started (documented for prod-local-test):
#   - auth server (port 4000)
#   - MCP server (port 8081)
#   - mail capture (port 1025)
#   - ingress proxy
prod-local-up:
	@echo "==> Starting local integration stack..."
	@./scripts/dev.sh start

# prod-local-test: Run integration tests against the local stack.
# Requires: prod-local-up running (starts app + frontend-dev).
prod-local-test:
	@echo "==> Running integration tests..."
	@scripts/run-integration-tests.sh || \
		{ echo "ERROR: integration tests failed"; exit 1; }

# prod-doctor: Diagnose configuration state (offline by default).
# Lists unresolved prerequisites, secret names/keys, and required inputs.
# Does not request secret contents. For live cluster inspection, use
# 'make prod-doctor-live' or set PROD_DOCTOR_LIVE=1.
prod-doctor:
	@scripts/prod-doctor.sh

# prod-doctor-live: Diagnose configuration state with live cluster inspection.
# Requires: kubectl configured with read-only access to the production cluster.
# Performs read-only inspection of Flux HelmRelease resources.
prod-doctor-live:
	@scripts/prod-doctor.sh --live
