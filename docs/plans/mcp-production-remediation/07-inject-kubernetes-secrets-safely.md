# Plan: Inject MCP Kubernetes Secrets Safely

## Finding

The MCP Helm Deployment renders only literal `.Values.env` entries and has no `secretKeyRef` or `envFrom` integration. As a result, production cannot safely supply `MCP_INTERNAL_API_KEY` and `MCP_SESSION_SECRET` through the chart. A separate raw manifest contains `CHANGE_ME`, which creates another unsafe deployment path.

## Desired state

- Secret material is created outside Git and referenced by name from the workload.
- Helm values contain secret names and key mappings, never secret contents.
- The backend and MCP workloads receive the same internal API key through controlled references.
- Secret rotation is documented and testable.

## Implementation

1. Add chart values such as `existingSecret.name` and explicit key names for `MCP_INTERNAL_API_KEY` and `MCP_SESSION_SECRET`.
2. Render individual `secretKeyRef` entries so required keys are auditable and missing keys prevent pod startup.
3. Extend `values.schema.json` to require a non-empty existing Secret name for production overlays.
4. Manage the Secret through the cluster's established mechanism (for example SOPS/age, Sealed Secrets, or an external secret controller). Do not commit plaintext Kubernetes Secret data.
5. Make the backend reference the same managed internal-key source.
6. Remove or convert `mcp-server/k8s/secret.yaml` into a non-deployable `.example` manifest with placeholders.
7. Document rotation: create next key, support a bounded dual-key window if necessary, roll both services, verify, then revoke the previous key.

## Tests

- Helm source and render assertions require `secretKeyRef` and prohibit literal secret values.
- CI scans tracked manifests for `stringData`, `CHANGE_ME`, development secret literals, and likely credential patterns.
- A missing-secret-key deployment test confirms the pod fails closed.
- A rotation test verifies old credentials stop working after the bounded migration window.

## Acceptance criteria

- No production secret value is stored in Git or passed through Helm command-line arguments.
- MCP and backend authenticate with a rotated, managed key.
- Missing secret references prevent startup instead of selecting fallback credentials.

