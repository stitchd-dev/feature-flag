#!/usr/bin/env python3
"""
Compare the pre-decomposition OpenAPI snapshot against the gateway's router
to surface any REST contract gaps introduced by the microservice decomposition.

Usage:
    python3 scripts/check_openapi_contract.py [--snapshot docs/openapi-pre-decomposition.json]
"""

import json
import re
import sys
from pathlib import Path

SNAPSHOT = Path(__file__).parent.parent / "docs/openapi-pre-decomposition.json"
ROUTER   = Path(__file__).parent.parent / "crates/stitchd-gateway/src/router.rs"

# All pre-decomposition routes are now proxied through the gateway.
KNOWN_GAPS: set[tuple[str, str]] = set()


def load_spec_routes(path: Path) -> set[tuple[str, str]]:
    with open(path) as f:
        spec = json.load(f)
    routes = set()
    for path_str, methods in spec.get("paths", {}).items():
        for method in methods:
            routes.add((method.upper(), path_str))
    return routes


def extract_gateway_routes(router_path: Path) -> set[tuple[str, str]]:
    """
    Parse router.rs lines like:
        .route("/v1/...", get(handler).post(other))
    and extract (METHOD, path) pairs.
    Uses balanced-paren scanning to handle nested function calls.
    """
    METHOD_NAMES = {"get", "post", "put", "delete", "patch", "head", "options"}
    src = router_path.read_text()
    collapsed = re.sub(r"\s+", " ", src)

    routes: set[tuple[str, str]] = set()
    pos = 0
    while True:
        m = re.search(r'\.route\(\s*"([^"]+)"', collapsed[pos:])
        if not m:
            break
        path = m.group(1)
        start = pos + m.start()
        # Scan forward balancing parentheses to find the end of .route(...)
        depth = 0
        i = start + len(".route(") - 1
        route_call = ""
        while i < len(collapsed):
            c = collapsed[i]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    route_call = collapsed[start:i+1]
                    break
            i += 1
        for method in METHOD_NAMES:
            if re.search(rf'\b{method}\s*\(', route_call):
                routes.add((method.upper(), path))
        pos = start + 1
    return routes


def normalise(path: str) -> str:
    """Normalise param names so {env_id} == {id} etc."""
    return re.sub(r"\{[^}]+\}", "{}", path)


def main() -> int:
    spec_routes    = load_spec_routes(SNAPSHOT)
    gateway_routes = extract_gateway_routes(ROUTER)

    # Normalise for comparison
    norm_gateway = {(m, normalise(p)) for m, p in gateway_routes}

    missing: list[tuple[str, str]] = []
    for method, path in sorted(spec_routes):
        if (method, normalise(path)) not in norm_gateway:
            missing.append((method, path))

    unexpected_gaps = [(m, p) for m, p in missing if (m, p) not in KNOWN_GAPS]

    print("=== OpenAPI Contract Check ===\n")
    print(f"Pre-decomposition routes : {len(spec_routes)}")
    print(f"Gateway routes           : {len(gateway_routes)}")
    print()

    if not missing:
        print("✓ All routes covered — no contract breakage.\n")
        return 0

    if unexpected_gaps:
        print("✗ UNEXPECTED GAPS (not in KNOWN_GAPS):")
        for method, path in unexpected_gaps:
            print(f"  {method:6}  {path}")
        print()

    print("Known intentional gaps (not proxied yet):")
    known_present = [(m, p) for m, p in missing if (m, p) in KNOWN_GAPS]
    for method, path in known_present:
        print(f"  {method:6}  {path}")
    print()

    if unexpected_gaps:
        print("Action required: add routes to gateway or add to KNOWN_GAPS with a comment.\n")
        return 1

    print("✓ All gaps are documented in KNOWN_GAPS — no unexpected contract breakage.\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
