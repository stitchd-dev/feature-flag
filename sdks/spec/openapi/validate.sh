#!/bin/bash
# Validate that sdks/spec/openapi/sdk.yaml is a well-formed OpenAPI 3.1 document.
# Requires: python3 with pyyaml + openapi-spec-validator (install via pip).
#
# Usage:
#   bash sdks/spec/openapi/validate.sh
#
# Exit 0 = valid; non-zero = invalid (validator emits details).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO_ROOT"

if ! python3 -c "import openapi_spec_validator" 2>/dev/null; then
  echo "ERROR: openapi-spec-validator not installed. Run:"
  echo "  python3 -m pip install pyyaml openapi-spec-validator"
  exit 1
fi

python3 - <<'PY'
import yaml, sys, warnings
warnings.filterwarnings('ignore')
from openapi_spec_validator import validate

specs = ['sdks/spec/openapi/sdk.yaml']
ok = True
for path in specs:
    with open(path) as f:
        spec = yaml.safe_load(f)
    try:
        validate(spec)
        print(f"✓ {path} — valid OpenAPI 3.1 ({len(spec['paths'])} paths, "
              f"{len(spec['components']['schemas'])} schemas)")
    except Exception as e:
        print(f"✗ {path} — {e}")
        ok = False
sys.exit(0 if ok else 1)
PY
