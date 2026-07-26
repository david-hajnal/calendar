.PHONY: all backend-check backend-test check check-no-yarn ci-script-test frontend-build frontend-test lint

all: check

backend-check:
	cargo check --manifest-path backend/Cargo.toml --locked

backend-test:
	cargo test --manifest-path backend/Cargo.toml --locked

frontend-test:
	pnpm --dir frontend test

lint:
	cargo fmt --manifest-path backend/Cargo.toml --check
	cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
	pnpm --dir frontend lint
	pnpm --dir frontend typecheck

frontend-build:
	pnpm --dir frontend build

check-no-yarn:
	sh scripts/check-no-yarn-lock.sh

ci-script-test:
	sh scripts/test-check-no-yarn-lock.sh

check: check-no-yarn ci-script-test backend-check backend-test frontend-test lint frontend-build
