.PHONY: all authorization-regression backend-check backend-test check check-no-yarn check-prod-tags ci-script-test deploy deploy-script-test docker-build docker-build-push docker-check docker-push e2e frontend-build frontend-test lint mcp-check mcp-test sqlite-prod-test trivy-check validate-authorization-coverage

all: check

backend-check:
	cargo check --manifest-path backend/Cargo.toml --locked

backend-test:
	cargo test --manifest-path backend/Cargo.toml --locked

mcp-check:
	cargo check --manifest-path mcp-server/Cargo.toml --locked

mcp-test:
	cargo test --manifest-path mcp-server/Cargo.toml --locked

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
	@echo "==> Checking production manifests for mutable tags..."
	@if grep -rn 'tag:.*latest' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null; then \
		echo "ERROR: Found 'latest' tag in production manifests"; exit 1; fi
	@if grep -rn 'tag:.*"main"' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null; then \
		echo "ERROR: Found 'main' tag in production manifests"; exit 1; fi

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

trivy-check: docker-check
	@if command -v trivy >/dev/null 2>&1; then \
		echo "==> Scanning images for CRITICAL/HIGH vulnerabilities..."; \
		trivy image --severity CRITICAL,HIGH --ignore-unfixed --exit-code 1 commoncal:local && \
		trivy image --severity CRITICAL,HIGH --ignore-unfixed --exit-code 1 commoncal-mcp:local; \
	elif [ "$${SKIP_TRIVY:-0}" = "1" ]; then \
		echo "SKIP_TRIVY=1: skipping Trivy scan"; \
	else \
		echo "ERROR: trivy is not installed. Install trivy or set SKIP_TRIVY=1 to skip." >&2; \
		exit 1; \
	fi
