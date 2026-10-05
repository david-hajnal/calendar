# CommonCal Helm chart

This chart deploys CommonCal as exactly one StatefulSet replica. The application
uses SQLite, so adding replicas would risk concurrent access to one database;
`values.schema.json` rejects any `replicaCount` other than `1`.

Create the session and SMTP credential secrets before installation. The chart
references them and never renders their values. Supply an image tag and an
operator values file with the real SMTP host and verified sender:

```sh
kubectl create secret generic commoncal-session \
  --from-literal=SESSION_SECRET='replace-with-a-long-random-value'
helm upgrade --install commoncal deploy/helm/commoncal \
  --set-string image.tag=YOUR_IMAGE_TAG --values mail-values.yaml
```

Set `config.appOrigin` and `ingress.hosts` to the public HTTPS host. Configure
TLS by setting `ingress.tls` with a pre-provisioned certificate secret (or a
certificate controller annotation). k3s's default Traefik class is selected by
default and can be changed through `ingress.className`.

`mail.host` and `mail.from` are required. `mail.port` defaults to 587 with
mandatory STARTTLS; 465 uses implicit TLS. `mail.existingSecret` maps username
and password keys from a separately managed Secret. Password login must remain
enabled for invited accounts. See [account email setup](../../../docs/external/account-email.md)
for the full configuration, SMTP egress, secret rotation, and Flux prerequisites.

Rate limiting is active in production (critical: 10/min, standard: 30/min,
permissive: 60/min per user). Superadmins bypass general write limits, while
invitation and account email actions retain their dedicated limits.

## Operational hardening

The StatefulSet runs as a non-root user with a read-only root filesystem,
RuntimeDefault seccomp, no Linux capabilities, and no service-account token
mounted in the pod. It provides startup, readiness, and liveness health probes,
resource requests and limits, and a 30-second termination grace period. The
default NetworkPolicy permits HTTP ingress only from the k3s Traefik namespace;
adjust `networkPolicy.ingress.from` if your ingress controller runs elsewhere.
The chart intentionally does not include an HPA because the SQLite PVC supports
only one replica.

## Data retention

The chart creates a standalone `PersistentVolumeClaim`, rather than a
StatefulSet claim template. It sets `helm.sh/resource-policy: keep` so Helm
retains the PVC on upgrade and uninstall, preserving SQLite data until it is
deliberately removed. The storage class's reclaim policy still governs the
backing volume after PVC deletion.
