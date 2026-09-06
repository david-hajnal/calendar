#!/usr/bin/env python3
"""Validate production Flux resources against the installed CRD schemas.

Self-contained (PyYAML only). Checks, for every resource rendered by the
production kustomization whose apiVersion belongs to a Flux CRD group:

- every key exists in the CRD openAPIV3Schema (unknown field = error)
- required fields are present
- enum values are allowed
- string patterns match
- basic types match
- array items validate against the item schema
- additionalProperties constraints are honored

Also asserts the bundle contains exactly two active HelmReleases and no
obsolete image-policy resources. The retained auth HelmRelease is excluded
while its external database dependency is unavailable.

Usage: scripts/validate-flux-crds.py <gotk-components.yaml> <rendered-bundle.yaml>
Exits 0 on success, 1 on any failure.
"""
import re
import sys
import yaml


def fail(msgs):
    for m in msgs:
        print("FAIL:", m)
    sys.exit(1)


def load_docs(path):
    with open(path) as f:
        return [d for d in yaml.safe_load_all(f) if d]


def type_ok(value, t):
    if t == "string":
        return isinstance(value, str)
    if t == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if t == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if t == "boolean":
        return isinstance(value, bool)
    if t == "array":
        return isinstance(value, list)
    if t == "object":
        return isinstance(value, dict)
    return True


def validate(instance, schema, path, errors):
    if schema is None:
        return
    if "anyOf" in schema or "oneOf" in schema:
        branches = schema.get("anyOf") or schema.get("oneOf")
        if any(validate(instance, b, path, []) is None for b in branches):
            return
        errors.append(f"{path}: matches no anyOf/oneOf branch")
        return

    t = schema.get("type")
    if t and not type_ok(instance, t):
        errors.append(f"{path}: expected {t}, got {type(instance).__name__} ({instance!r})")
        return

    if instance is None:
        return

    if "enum" in schema and instance not in schema["enum"]:
        errors.append(f"{path}: {instance!r} not in enum {schema['enum']}")

    if isinstance(instance, str) and "pattern" in schema:
        if not re.search(schema["pattern"], instance):
            errors.append(f"{path}: {instance!r} does not match pattern {schema['pattern']!r}")

    if isinstance(instance, list) and "items" in schema:
        item_schema = schema["items"]
        if isinstance(item_schema, dict):
            for i, item in enumerate(instance):
                validate(item, item_schema, f"{path}[{i}]", errors)
        return

    if isinstance(instance, dict):
        props = schema.get("properties")
        if props is not None:
            for req in schema.get("required", []):
                if req not in instance:
                    errors.append(f"{path}: missing required field {req!r}")
            for key, value in instance.items():
                if key in props:
                    validate(value, props[key], f"{path}.{key}", errors)
                else:
                    ap = schema.get("additionalProperties", True)
                    if isinstance(ap, dict):
                        validate(value, ap, f"{path}.{key}", errors)
                    else:
                        # The API server silently prunes unknown fields on
                        # CRDs, so flag them here to catch typos.
                        errors.append(f"{path}: unknown field {key!r}")
    return None


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    components_path, bundle_path = sys.argv[1], sys.argv[2]

    components = load_docs(components_path)
    bundle = load_docs(bundle_path)

    errors = []

    # Index CRDs: (group, version, kind) -> spec schema
    crd_schemas = {}
    crd_names = set()
    for d in components:
        if d.get("kind") != "CustomResourceDefinition":
            continue
        crd_names.add(d["metadata"]["name"])
        group = d["spec"]["group"]
        kind = d["spec"]["names"]["kind"]
        for v in d["spec"]["versions"]:
            schema = v.get("schema", {}).get("openAPIV3Schema", {})
            crd_schemas[(group, v["name"], kind)] = schema

    # 1. Bundle must contain the expected deployment resources only.
    def count(kind):
        return sum(1 for d in bundle if d.get("kind") == kind)

    expected = {
        "HelmRelease": 2,
        "ImageRepository": 0,
        "ImagePolicy": 0,
        "ImageUpdateAutomation": 0,
    }
    for kind, want in expected.items():
        got = count(kind)
        if got != want:
            errors.append(f"bundle: expected {want} {kind}, got {got}")

    # 2. Validate every Flux-CRD resource in the bundle against its CRD schema
    validated = 0
    for d in bundle:
        api_version = d.get("apiVersion", "")
        kind = d.get("kind", "")
        if "/" not in api_version:
            continue
        group, version = api_version.split("/", 1)
        schema = crd_schemas.get((group, version, kind))
        if schema is None:
            continue  # core k8s or non-Flux resource
        spec_schema = schema.get("properties", {}).get("spec")
        if spec_schema is None:
            continue
        res_errors = []
        validate(d.get("spec", {}), spec_schema, "spec", res_errors)
        validated += 1
        if res_errors:
            errors.append(
                f"{kind}/{d['metadata']['name']}: " + "; ".join(res_errors)
            )

    if validated < 2:
        errors.append(f"expected >=2 Flux CRD resources validated, got {validated}")

    if errors:
        fail(errors)
    print(
        f"OK: {validated} Flux CRD resources conform to installed schemas; "
        "expected resource set present"
    )


if __name__ == "__main__":
    main()
