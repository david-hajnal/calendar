#!/usr/bin/env sh
set -eu
chart_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT HUP INT TERM

render() {
  helm template commoncal "$chart_dir" --set-string image.tag=test-mail \
    --set-string mail.host=smtp.example.test \
    --set-string mail.from=no-reply@example.test "$@"
}

render --set backup.enabled=true --set mail.port=465 \
  --set-string mail.existingSecret.name=custom-mail \
  --set-string mail.existingSecret.usernameKey=username \
  --set-string mail.existingSecret.passwordKey=password \
  --set-string 'mail.egressCIDRs[0]=192.0.2.0/24' > "$fixture/rendered.yaml"
python3 - "$fixture/rendered.yaml" <<'PY'
import sys
import yaml
documents = [d for d in yaml.safe_load_all(open(sys.argv[1], encoding="utf-8")) if d]
def kind(name):
    return next(d for d in documents if d["kind"] == name and d["metadata"]["name"].startswith("commoncal"))
config = kind("ConfigMap")["data"]
assert config["APP_ENV"] == "production"
assert config["PASSWORD_LOGIN_ENABLED"] == "true"
assert config["SMTP_HOST"] == "smtp.example.test"
assert config["SMTP_FROM"] == "no-reply@example.test"
assert config["SMTP_PORT"] == "465"
assert "SMTP_PASSWORD" not in config and "SMTP_USERNAME" not in config
pod = kind("StatefulSet")["spec"]["template"]
assert pod["metadata"]["annotations"]["checksum/config"]
container = pod["spec"]["containers"][0]
env = {e["name"]: e for e in container["env"]}
for setting, key in [("SMTP_USERNAME", "username"), ("SMTP_PASSWORD", "password")]:
    assert env[setting]["valueFrom"]["secretKeyRef"] == {"name": "custom-mail", "key": key}
    assert "value" not in env[setting]
assert container["envFrom"] == [{"configMapRef": {"name": "commoncal"}}]
egress = kind("NetworkPolicy")["spec"]["egress"]
assert any(rule.get("to") == [{"ipBlock": {"cidr": "192.0.2.0/24"}}] and
           rule.get("ports") == [{"protocol": "TCP", "port": 465}] for rule in egress)
backup = kind("CronJob")["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]
assert backup["envFrom"] == [{"configMapRef": {"name": "commoncal"}}]
assert not any(e["name"].startswith("SMTP_") for e in backup["env"])
assert not any(d["kind"] == "Secret" for d in documents)
PY

reject() {
  if render "$@" > "$fixture/rejected.yaml" 2> "$fixture/error"; then
    echo "invalid mail settings accepted: $*" >&2
    exit 1
  fi
}
reject --set-string mail.host=
reject --set-string mail.from=
reject --set mail.port=0
reject --set mail.port=65536
reject --set config.passwordLoginEnabled=false
reject --set-string mail.existingSecret.name=
reject --set-string mail.existingSecret.usernameKey=
reject --set-string mail.existingSecret.passwordKey=
reject --set-string mail.password=must-never-be-inline
reject --set-string mail.username=must-never-be-inline
reject --set-json 'mail.egressCIDRs=[]'

render > "$fixture/default.yaml"
python3 - "$fixture/default.yaml" <<'PY'
import sys
import yaml
docs = [d for d in yaml.safe_load_all(open(sys.argv[1], encoding="utf-8")) if d]
config = next(d for d in docs if d["kind"] == "ConfigMap")["data"]
assert config["SMTP_PORT"] == "587"
policy = next(d for d in docs if d["kind"] == "NetworkPolicy" and d["metadata"]["name"] == "commoncal")
assert any(r.get("ports") == [{"protocol": "TCP", "port": 587}] for r in policy["spec"]["egress"])
PY
echo 'commoncal mail assertions passed'
