# Plan: Make the MCP Pod Reachable Through Its Service

## Finding

The MCP server defaults to `127.0.0.1:3001`, while Kubernetes forwards Service traffic to pod port `3001`. Loopback binding permits the pod-local TCP probes to pass but prevents traffic arriving on the pod network interface. The observed production `/mcp` endpoint returned an upstream error on the hostname configured at the time of inspection.

## Desired state

The container listens on `0.0.0.0:3001` in Kubernetes, remains ClusterIP-only behind ingress, and reports readiness through an HTTP check that exercises the application route.

## Implementation

1. Set `BIND_ADDRESS=0.0.0.0:3001` explicitly in MCP Helm production values.
2. Keep the Rust development default on loopback; do not change local exposure as a side effect.
3. Change startup, readiness, and liveness probes from raw TCP to the existing `/health/ready` and `/health/live` HTTP endpoints.
4. Confirm the Service `targetPort: http` resolves to container port `3001`.
5. Review the NetworkPolicy ingress namespace selector against the actual Traefik namespace and labels.

## Tests

- Helm assertions verify the rendered bind address and HTTP probe paths.
- A container integration test starts the image and reaches `/health/ready` through the container network address, not only localhost.
- A disposable-cluster smoke test reaches the service from an allowed ingress pod and fails from a disallowed namespace.

## Rollout and rollback

Deploy with a rolling health check from inside the cluster, then verify the public endpoint. Roll back the image/chart revision if readiness fails; do not weaken the NetworkPolicy as a quick workaround.

## Acceptance criteria

- Kubernetes Service traffic reaches the MCP process.
- Health probes detect HTTP routing failures rather than merely an open socket.
- NetworkPolicy continues to limit ingress to the intended controller.

