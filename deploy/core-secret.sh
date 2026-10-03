#!/usr/bin/env bash
# Source from deployment scripts. Never log secret material. Existing objects
# are preserved wholesale; intentional rotation is an independent operation.
ensure_core_secret() {
  local existing supplied
  existing=$(kubectl get secret commoncal-session -n "$NAMESPACE" --ignore-not-found -o 'jsonpath={.data.SESSION_SECRET}') || {
    echo 'ERROR: cannot read core secret; refusing replacement' >&2
    return 1
  }
  if [[ -n "$existing" ]]; then
    supplied=$(printf '%s' "$SESSION_SECRET" | openssl base64 -A)
    if [[ "$existing" != "$supplied" ]]; then
      echo 'ERROR: SESSION_SECRET differs from existing key. Normal deployment refuses rotation; use explicit rotate-session-secret.sh after reviewing migration impact.' >&2
      return 1
    fi
    echo '==> Preserving existing commoncal-session secret.'
    return 0
  fi
  # Distinguish missing object from an existing object with an empty key.
  local name
  name=$(kubectl get secret commoncal-session -n "$NAMESPACE" --ignore-not-found -o 'jsonpath={.metadata.name}') || return 1
  if [[ -n "$name" ]]; then
    echo 'ERROR: existing commoncal-session has missing/empty SESSION_SECRET; repair explicitly.' >&2
    return 1
  fi
  kubectl create secret generic commoncal-session \
    --from-literal=SESSION_SECRET="$SESSION_SECRET" \
    --from-literal=BACKUP_ENCRYPTION_KEY_HEX="$BACKUP_ENCRYPTION_KEY_HEX" \
    -n "$NAMESPACE" ${core_secret_create_args[@]+"${core_secret_create_args[@]}"}
}
