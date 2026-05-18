#!/usr/bin/env bash
# check_scylla_containment.sh
#
# Asserts that only the allowed crates depend directly on the `scylla` crate.
#
# Allowed list:
#   - stitchd-db              (shared persistence library; owns ScyllaSegmentStore)
#   - stitchd-segmentation-service  (only binary that opens a Scylla connection)
#
# xtask is intentionally absent: it uses stitchd-db transitively, not directly.
#
# Usage (from workspace root):
#   bash scripts/check_scylla_containment.sh
#
# Exit codes:
#   0  all good
#   1  one or more violations found
#
# Compatible with bash 3.2+ (macOS default).

set -euo pipefail

ALLOWED_1="crates/stitchd-db/Cargo.toml"
ALLOWED_2="crates/stitchd-segmentation-service/Cargo.toml"

is_allowed() {
  local path="$1"
  [[ "$path" == "$ALLOWED_1" || "$path" == "$ALLOWED_2" ]]
}

violations=""
found_list=""

# grep -l exits 1 when nothing matches; swallow that with || true
while IFS= read -r found; do
  [[ -z "$found" ]] && continue
  found_list="${found_list} ${found}"
  if ! is_allowed "$found"; then
    violations="${violations}  - ${found}\n"
  fi
done < <(grep -rl 'scylla = ' crates/*/Cargo.toml 2>/dev/null || true)

if [[ -n "$violations" ]]; then
  echo "ERROR: ScyllaDB containment violation(s) detected!"
  echo ""
  echo "The following crates have an unexpected direct dependency on 'scylla':"
  printf "%b" "$violations"
  echo ""
  echo "Only the following crates may depend on 'scylla' directly:"
  echo "  - $ALLOWED_1"
  echo "  - $ALLOWED_2"
  echo ""
  echo "See crates/stitchd-segmentation-service/SCYLLA_OWNERSHIP.md for policy details."
  exit 1
fi

echo "OK: ScyllaDB containment check passed."
echo "Direct scylla deps found in:${found_list:- none}"
exit 0
