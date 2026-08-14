# syntax=docker/dockerfile:1

FROM node:22-bookworm-slim AS frontend-build
WORKDIR /build
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY frontend/ frontend/
RUN corepack enable && pnpm install --frozen-lockfile
RUN pnpm --filter @commoncal/frontend build && \
    test -d /build/frontend/dist/assets && \
    test -f /build/frontend/dist/index.html || (echo "Frontend build produced no output" && exit 1)

FROM rust:1-bookworm AS backend-build
WORKDIR /build/backend
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/migrations migrations
COPY backend/src src
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates busybox-static \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system commoncal \
    && useradd --system --gid commoncal --home-dir /app --shell /usr/sbin/nologin commoncal \
    && mkdir -p /app/data /app/tmp /app/frontend \
    && chown -R commoncal:commoncal /app
WORKDIR /app
COPY --from=backend-build /build/backend/target/release/commoncal-backend /usr/local/bin/commoncal-backend
COPY --from=frontend-build /build/frontend/dist /app/frontend

USER commoncal
ENV APP_ENV=production \
    APP_ORIGIN=http://127.0.0.1:3000 \
    BIND_ADDRESS=0.0.0.0:3000 \
    DATABASE_PATH=/app/data/commoncal.sqlite \
    FRONTEND_DIR=/app/frontend \
    TMPDIR=/app/tmp
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/bin/busybox", "wget", "--spider", "-q", "http://127.0.0.1:3000/health/ready"]
ENTRYPOINT ["/usr/local/bin/commoncal-backend"]
