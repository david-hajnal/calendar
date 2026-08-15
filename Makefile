.PHONY: all authorization-regression backend-check backend-test check check-no-yarn ci-script-test deploy-script-test e2e frontend-build frontend-test lint mcp-check mcp-test validate-authorization-coverage

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

deploy-script-test:
	sh scripts/test-deploy-prod.sh

check: check-no-yarn ci-script-test backend-check backend-test authorization-regression mcp-check mcp-test frontend-test lint frontend-build deploy-script-test

# Production deployment. Requires:
#   SESSION_SECRET        - session encryption key
#   BACKUP_ENCRYPTION_KEY_HEX - hex-encoded backup encryption key
# Required: IMAGE_TAG. Optional: DOMAIN, NAMESPACE, HELM_RELEASE_NAME, DRY_RUN=1
deploy:
	deploy/deploy-prod.sh
