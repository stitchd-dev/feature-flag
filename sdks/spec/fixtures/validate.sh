#!/bin/bash
# Validate the conformance fixtures:
#  1. Every scenario has the required files (description.md, flag_definitions.json,
#     segment_definitions.json, requests.json, expected.json)
#  2. requests.json entries validate against eval_request.schema.json
#  3. expected.json entries validate against eval_result.schema.json OR
#     eval_result_with_reasoning.schema.json (latter if `reasoning` field present)
#  4. hashing/reference_vectors.json parses as JSON
#
# Uses python3 jsonschema package.
# Usage: bash sdks/spec/fixtures/validate.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO_ROOT"

if ! python3 -c "import jsonschema" 2>/dev/null; then
  echo "ERROR: jsonschema not installed. Run: python3 -m pip install jsonschema"
  exit 1
fi

python3 - <<'PY'
import json, sys, glob, os, warnings
warnings.filterwarnings('ignore')
from jsonschema import Draft202012Validator
from referencing import Registry, Resource

def load(p):
    with open(p) as f:
        return json.load(f)

ROOT = 'sdks/spec/fixtures'

# Build a Registry so cross-file $ref (https://stitchd.dev/...) resolves
# against the local schema files, not the public internet.
registry = Registry()
for schema_path in glob.glob('sdks/spec/schemas/*.schema.json'):
    schema = load(schema_path)
    sid = schema.get('$id')
    if sid:
        registry = registry.with_resource(uri=sid, resource=Resource.from_contents(schema))

eval_req_schema = load('sdks/spec/schemas/eval_request.schema.json')
eval_res_schema = load('sdks/spec/schemas/eval_result.schema.json')
eval_res_reason_schema = load('sdks/spec/schemas/eval_result_with_reasoning.schema.json')

req_validator = Draft202012Validator(eval_req_schema, registry=registry)
res_validator = Draft202012Validator(eval_res_schema, registry=registry)
res_reason_validator = Draft202012Validator(eval_res_reason_schema, registry=registry)

ok = True

# Hashing fixtures
hash_path = f'{ROOT}/hashing/reference_vectors.json'
try:
    hv = load(hash_path)
    assert 'vectors' in hv and isinstance(hv['vectors'], list) and hv['vectors']
    print(f"✓ {hash_path} — {len(hv['vectors'])} reference vectors")
except Exception as e:
    print(f"✗ {hash_path} — {e}")
    ok = False

# Evaluation scenarios
scenarios = sorted(glob.glob(f'{ROOT}/evaluation/*/'))
if not scenarios:
    print("ERROR: no evaluation scenarios found")
    sys.exit(1)

REQUIRED = ['description.md', 'flag_definitions.json', 'segment_definitions.json',
            'requests.json', 'expected.json']

for s in scenarios:
    name = s.rstrip('/').split('/')[-1]
    print(f"  scenario {name}:")
    # Required files
    for f in REQUIRED:
        path = os.path.join(s, f)
        if not os.path.exists(path):
            print(f"    ✗ missing {f}")
            ok = False
    # requests.json
    reqs = load(os.path.join(s, 'requests.json'))
    if not isinstance(reqs, list) or not reqs:
        print(f"    ✗ requests.json must be a non-empty array")
        ok = False
        continue
    for i, r in enumerate(reqs):
        errs = sorted(req_validator.iter_errors(r), key=lambda e: e.path)
        if errs:
            for e in errs:
                print(f"    ✗ requests.json[{i}]: {e.message}")
            ok = False
    # expected.json — pick reasoning vs non-reasoning schema based on presence of `reasoning` key
    exps = load(os.path.join(s, 'expected.json'))
    if not isinstance(exps, list) or len(exps) != len(reqs):
        print(f"    ✗ expected.json length ({len(exps) if isinstance(exps, list) else 'n/a'}) != requests.json length ({len(reqs)})")
        ok = False
        continue
    for i, ex in enumerate(exps):
        if 'reasoning' in ex:
            # Allow "evaluated_at": "IGNORE" as a sentinel
            patched = json.loads(json.dumps(ex))
            if patched.get('reasoning', {}).get('evaluated_at') == 'IGNORE':
                patched['reasoning']['evaluated_at'] = '2026-01-01T00:00:00.000Z'
            errs = sorted(res_reason_validator.iter_errors(patched), key=lambda e: e.path)
            schema_name = 'eval_result_with_reasoning'
        else:
            errs = sorted(res_validator.iter_errors(ex), key=lambda e: e.path)
            schema_name = 'eval_result'
        if errs:
            for e in errs:
                print(f"    ✗ expected.json[{i}] (vs {schema_name}): {e.message}")
            ok = False
    if all(os.path.exists(os.path.join(s, f)) for f in REQUIRED):
        print(f"    ✓ all files present; requests + expected validate")

sys.exit(0 if ok else 1)
PY
