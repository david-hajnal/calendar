# Problem: Fix Failing Backup Jobs

## Goal
Investigate and resolve the failing backup CronJob for the core backend. All backup jobs show Error status.

## Current State

### Backup Configuration
- **Enabled:** `true` in `core-helmrelease.yaml:79`
- **Schedule:** `0 2 * * *` (daily at 2:00 AM Europe/Budapest)
- **Concurrency policy:** `Forbid` (no parallel backups)
- **Failed jobs history limit:** 3
- **Successful jobs history limit:** 3
- **PVC size:** 5Gi
- **PVC storage class:** inherited from `persistence.storageClass` (empty string = default)

### Backup Mechanism
- **CronJob:** `commoncal-backup` (CronJob kind)
- **Image:** Same as core backend (`ghcr.io/david-hajnal/calendar-core:<tag>`)
- **Command:** `/usr/local/bin/commoncal-backend backup /backup`
- **Data volume:** PVC `commoncal-data` mounted read-only at `/app/data`
- **Backup volume:** PVC `commoncal-backup` mounted at `/backup`
- **Encryption:** `BACKUP_ENCRYPTION_KEY_HEX` from `commoncal-session` secret

### Helm Values (defaults in `values.yaml:114-127`)
```yaml
backup:
  enabled: false          # overridden to true in production overlay
  schedule: "0 2 * * *"
  backoffLimit: 1
  resources:
    requests:
      cpu: 50m
      memory: 64Mi
    limits:
      cpu: 500m
      memory: 256Mi
  persistence:
    size: 5Gi
    storageClass: ""
```

### PVCs
- **Data PVC:** `commoncal-data` (10Gi, ReadWriteOnce) — attached to StatefulSet `commoncal-0`
- **Backup PVC:** `commoncal-backup` (5Gi, ReadWriteOnce) — mounted by CronJob
- **Annotation on backup PVC:** `helm.sh/resource-policy: keep` (persists after Helm uninstall)

## Investigation Steps

### 1. Check CronJob status
```bash
kubectl get cronjob -n commoncal commoncal-backup
kubectl get jobs -n commoncal -l app.kubernetes.io/instance=commoncal
kubectl get pods -n commoncal --field-selector=status.phase=Failed
```

### 2. Check failed pod logs
```bash
kubectl logs <failed-backup-pod> -n commoncal
kubectl describe pod <failed-backup-pod> -n commoncal
```

### 3. Check PVC status
```bash
kubectl get pvc -n commoncal
kubectl describe pvc commoncal-backup -n commoncal
```

### 4. Check StatefulSet data PVC
```bash
kubectl get pvc commoncal-data -n commoncal
kubectl describe pvc commoncal-data -n commoncal
```

## Likely Failure Causes

### A. Image tag mismatch
The CronJob uses the same image as the core StatefulSet (`image.repository:image.tag`). If the tag in the HelmRelease is invalid or the image doesn't contain the `backup` subcommand, the job will fail.

### B. PVC binding issues
- Backup PVC may not be bound (storage class issue)
- Data PVC may be unbound or in Lost state
- ReadWriteOnce PVC may not be accessible from CronJob pod (scheduling constraints)

### C. Missing or invalid secrets
- `BACKUP_ENCRYPTION_KEY_HEX` in `commoncal-session` secret may be missing or invalid
- Validation in `deploy-prod.sh:33-36` requires even number of hex chars (at least 32)
- `SESSION_SECRET` may be missing

### D. Storage class issues
- `storageClass: ""` in values means use cluster default
- If no default storage class exists, PVC binding fails
- K3s may have different storage provisioning behavior

### E. Backup command failure
- The `backup` subcommand may fail due to:
  - SQLite database corruption or lock
  - Insufficient permissions on `/app/data` (mounted read-only)
  - Disk space on backup PVC
  - Encryption key format issues

## What Needs to Be Done

1. **Gather failure evidence** — check CronJob status, pod logs, PVC status
2. **Identify root cause** — match logs/errors to likely causes above
3. **Fix the root cause** — depends on findings:
   - If PVC issue: fix storage class or PVC binding
   - If secret issue: verify `commoncal-session` secret contents
   - If image/command issue: verify image contains `backup` subcommand
   - If encryption key issue: regenerate valid key
4. **Test manually** — trigger a backup CronJob and verify success
5. **Verify next scheduled run** — confirm CronJob completes on next cycle

## Related Files

| File | Purpose |
|------|---------|
| `deploy/flux/overlays/production/charts/core-helmrelease.yaml:78-82` | Backup config in HelmRelease |
| `deploy/helm/commoncal/templates/cronjob-backup.yaml` | CronJob template |
| `deploy/helm/commoncal/templates/pvc-backup.yaml` | Backup PVC template |
| `deploy/helm/commoncal/values.yaml:114-127` | Default backup values |
| `deploy/deploy-prod.sh:300-302` | Secret creation (BACKUP_ENCRYPTION_KEY_HEX) |

## Verification

After fix:
1. `kubectl get jobs -n commoncal -l app.kubernetes.io/instance=commoncal` — latest job should have `COMPLETIONS: 1/1`
2. `kubectl logs <successful-backup-pod> -n commoncal` — no errors
3. `kubectl get pvc commoncal-backup -n commoncal` — PVC should have data
4. Wait for next scheduled run (2:00 AM) and verify it completes
