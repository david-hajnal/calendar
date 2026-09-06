#!/usr/bin/env sh
set -eu

application_namespace=${APPLICATION_NAMESPACE:-commoncal}
flux_namespace=${FLUX_NAMESPACE:-flux-system}
ingress_namespace=${INGRESS_NAMESPACE:-traefik}
origin_host=${ORIGIN_HOST:-cal.hajnal.space}

for command in kubectl flux curl; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command is required" >&2
    exit 1
  fi
done

echo "Kubernetes context: $(kubectl config current-context)"
echo "Allowing ingress from namespace: $ingress_namespace"

kubectl get namespace "$ingress_namespace" >/dev/null

for release in commoncal commoncal-mcp; do
  flux suspend helmrelease "$release" -n "$flux_namespace"
done

for policy in commoncal commoncal-mcp; do
  kubectl patch networkpolicy "$policy" \
    -n "$application_namespace" \
    --type=json \
    -p="[{\"op\":\"replace\",\"path\":\"/spec/ingress/0/from/0/namespaceSelector/matchLabels/kubernetes.io~1metadata.name\",\"value\":\"$ingress_namespace\"}]"
done

origin_ip=$(kubectl get ingress commoncal \
  -n "$application_namespace" \
  -o jsonpath='{.status.loadBalancer.ingress[0].ip}')

if [ -z "$origin_ip" ]; then
  echo "commoncal ingress has no load-balancer IP" >&2
  exit 1
fi

curl --fail --silent --show-error --insecure \
  --resolve "$origin_host:443:$origin_ip" \
  "https://$origin_host/health/ready"
echo

cat <<EOF
Ingress recovery succeeded.

Commit and push the matching Git configuration before resuming Flux. Then run:
  flux resume helmrelease commoncal -n $flux_namespace
  flux resume helmrelease commoncal-mcp -n $flux_namespace
  flux reconcile kustomization flux-system -n $flux_namespace --with-source
EOF
