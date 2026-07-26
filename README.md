# CommonCal

CommonCal is a multi-user calendar platform. This repository currently contains
the initial Rust HTTP application and React project foundation.

## Prerequisites

- Rust stable
- Node.js 22
- pnpm 9.12.3

Yarn is not supported. Do not create a `yarn.lock`.

## Setup

Install frontend dependencies from the repository root:

```sh
pnpm install --frozen-lockfile
```

Run all checks:

```sh
make check
```

Focused commands:

```sh
make backend-test
make frontend-test
make lint
```

Start the frontend development server:

```sh
pnpm --dir frontend dev
```

Start the backend development server:

```sh
cargo run --manifest-path backend/Cargo.toml
```

The backend defaults to `APP_ENV=development`,
`BIND_ADDRESS=127.0.0.1:3000`, and `APP_ORIGIN=http://127.0.0.1:3000`.
Production startup requires `APP_ENV=production` and a non-empty
`SESSION_SECRET`. Set `APP_ORIGIN` to the browser-visible application origin;
authenticated unsafe requests must match it.
