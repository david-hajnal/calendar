# Build stage
FROM rust:1.82-slim AS builder

WORKDIR /app
COPY . .

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN addgroup --system --gid 1001 mcp && \
    adduser --system --uid 1001 --gid 1001 mcp

WORKDIR /app
COPY --from=builder /app/target/release/mcp-server /app/mcp-server
COPY --from=builder /app/migrations /app/migrations

RUN chown -R mcp:mcp /app

USER mcp

ENV RUST_LOG=info
ENV RUST_BACKTRACE=0

EXPOSE 8080

CMD ["/app/mcp-server"]
