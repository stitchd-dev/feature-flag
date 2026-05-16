#!/bin/bash
# Validate that every .schema.json under sdks/spec/schemas/ is well-formed
# Draft 2020-12 JSON Schema. Uses the `jsonschema` Python package's meta-schema.
#
# Usage:
#   bash sdks/spec/schemas/validate.sh
#
# Exit 0 = all schemas valid; non-zero = at least one is malformed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO_ROOT"

if ! python3 -c "import jsonschema" 2>/dev/null; then
  echo "ERROR: jsonschema not installed. Run: python3 -m pip install jsonschema"
  exit 1
fi

python3 - <<'PY'
import json, sys, glob, warnings
warnings.filterwarnings('ignore')
from jsonschema.validators import Draft202012Validator

paths = sorted(glob.glob('sdks/spec/schemas/*.schema.json'))
if not paths:
    print("ERROR: no *.schema.json files found")
    sys.exit(1)

ok = True
for p in paths:
    with open(p) as f:
        try:
            schema = json.load(f)
        except json.JSONDecodeError as e:
            print(f"✗ {p} — JSON parse error: {e}")
            ok = False
            continue
    try:
        Draft202012Validator.check_schema(schema)
        title = schema.get('title', '(no title)')
        print(f"✓ {p} — '{title}' is valid Draft 2020-12")
    except Exception as e:
        print(f"✗ {p} — schema error: {e}")
        ok = False
sys.exit(0 if ok else 1)
PY
