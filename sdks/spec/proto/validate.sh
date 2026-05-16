#!/bin/bash
# Validate that every .proto under sdks/spec/proto/ compiles with protoc.
# Used by CI and by the sdks/spec/ conformance harness.
#
# Usage:
#   bash sdks/spec/proto/validate.sh
#   (run from repo root, with `proto/` for shared imports + `sdks/spec/proto/` for SDK protos)
#
# Exit 0 = all files compile; non-zero = at least one failed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO_ROOT"

if ! command -v protoc >/dev/null 2>&1; then
  echo "ERROR: protoc not found in PATH"
  exit 1
fi

PROTO_FILES=$(find sdks/spec/proto -name "*.proto" -type f | sort)

if [ -z "$PROTO_FILES" ]; then
  echo "ERROR: no .proto files found under sdks/spec/proto"
  exit 1
fi

COUNT=$(echo "$PROTO_FILES" | wc -l | tr -d ' ')
echo "Validating $COUNT proto file(s):"
echo "$PROTO_FILES" | sed 's/^/  - /'

# shellcheck disable=SC2086
protoc \
  -I proto \
  -I sdks/spec/proto \
  --descriptor_set_out=/dev/null \
  $PROTO_FILES

echo "✓ All proto files compile cleanly."
