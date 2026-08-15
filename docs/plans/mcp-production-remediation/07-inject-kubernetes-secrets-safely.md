# Plan: Inject MCP Kubernetes Secrets Safely

## Finding

The MCP Helm Deployment historically rendered `.Values.env` entries as plaintext and lacked `secretKeyRef` integration. A separate raw manifest also contains a `CHANGE_ME` placeholder, creating an unsafe alternate deployment path.

## Desired state

- Secret material is created outside Git and referenced by name.
- Helm values contain secret names and key mappings, never secret contents.
- Backend and MCP receive the same managed internal API key.
- Missing secret keys prevent startup.

## Implementation

1. Add values for an existing Secret and explicit key mappings for `MCP_INTERNAL_API_KEY` and `MCP_SESSION_SECRET`.
2. Render individual `secretKeyRef` entries so required keys are auditable.
3. Extend `values.schema.json` to require a non-empty Secret name in production overlays.
4. Manage secrets through the cluster's established encrypted or external-secret mechanism.
5. Make the backend reference the same managed internal-key source.
6. Convert `mcp-server/k8s/secret.yaml` into a non-deployable example or remove the stale path.
7. Document a bounded credential-rotation procedure for both services.

## Verification

- Helm tests require `secretKeyRef` and prohibit literal secret values.
- CI scans tracked manifests for placeholders, development secret literals, and likely credentials.
- Missing-key deployment tests prove the pod fails closed.
- Rotation tests prove the previous key stops working after migration.

## Acceptance criteria

- No production secret is stored in Git or passed through Helm command-line arguments.
- MCP and backend authenticate with a managed, rotated key.
- Missing secret references cannot select fallback credentials.

