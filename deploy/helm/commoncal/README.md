# CommonCal Helm chart

This chart deploys CommonCal as exactly one StatefulSet replica. The application
uses SQLite, so adding replicas would risk concurrent access to one database;
`values.schema.json` rejects any `replicaCount` other than `1`.

Create the session secret before installation. The chart references it and never
renders a secret value:

```sh
kubectl create secret generic commoncal-session \
  --from-literal=SESSION_SECRET='replace-with-a-long-random-value'
helm upgrade --install commoncal deploy/helm/commoncal
```

Set `config.appOrigin` and `ingress.hosts` to the public HTTPS host. Configure
TLS by setting `ingress.tls` with a pre-provisioned certificate secret (or a
certificate controller annotation). k3s's default Traefik class is selected by
default and can be changed through `ingress.className`.

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
StatefulSet claim template. Helm retains this PVC by default on upgrade and
uninstall, preserving SQLite data until it is deliberately removed. The storage
class's reclaim policy still governs the backing volume after PVC deletion.
