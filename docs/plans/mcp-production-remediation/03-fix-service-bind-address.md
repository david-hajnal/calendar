# Plan: Make the MCP Pod Reachable Through Its Service

## Finding

The MCP server defaults to `127.0.0.1:3001`, while Kubernetes sends Service traffic to pod port `3001`. Pod-local TCP probes can pass even though the listener is unreachable through the pod network interface.

## Desired state

The Kubernetes container listens on `0.0.0.0:3001`, remains ClusterIP-only behind ingress, and uses HTTP health probes that exercise application routing.

## Implementation

1. Set `BIND_ADDRESS=0.0.0.0:3001` explicitly in production values.
2. Preserve loopback as the local-development default.
3. Replace TCP probes with HTTP checks for `/health/ready` and `/health/live`.
4. Confirm the Service `targetPort: http` maps to container port `3001`.
5. Validate the NetworkPolicy against the actual Traefik namespace and labels.

## Verification

- Helm assertions verify the bind address and HTTP probe paths.
- A container test reaches readiness through the container network address, not only localhost.
- An in-cluster smoke test reaches the Service from ingress and rejects a disallowed namespace.
- The public endpoint returns an MCP/authentication response instead of `502`.

## Acceptance criteria

- Kubernetes Service traffic reaches the MCP process.
- Health probes detect routing failures, not merely an open socket.
- NetworkPolicy still restricts ingress to the intended controller.

