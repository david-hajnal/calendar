# Helm chart checks

Run the chart checks with Helm 3:

```sh
helm lint deploy/helm/commoncal
deploy/helm/commoncal/tests/template_assertions.sh
```

The assertions intentionally verify rendered Kubernetes resources, including the
single-replica guard, security context, PVC database mount, and use of an
existing Secret for `SESSION_SECRET`.

If Helm is unavailable, `template_assertions.sh` performs a self-contained
source check for those acceptance gates, including the schema's `const: 1`
guard. This fallback does not validate Helm rendering; run the two Helm commands
above in CI or before release.
