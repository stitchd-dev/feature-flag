# ScyllaDB Dependency Ownership

This document records which crates in the workspace depend **directly** on the
`scylla` crate, the reason each dependency is considered correct, and the policy
that must be upheld going forward.

---

## Audit results (run: `grep -l 'scylla = ' crates/*/Cargo.toml`)

| Crate | Direct `scylla` dep? | Verdict |
|---|---|---|
| `stitchd-db` | yes | **ALLOWED** |
| `stitchd-segmentation-service` | yes | **ALLOWED** |
| `stitchd-analytics-service` | no | compliant |
| `stitchd-auth-service` | no | compliant |
| `stitchd-core` | no | compliant |
| `stitchd-event-writer` | no | compliant |
| `stitchd-experimentation-service` | no | compliant |
| `stitchd-flag-service` | no | compliant |
| `stitchd-gateway` | no | compliant |
| `stitchd-proto` | no | compliant |
| `stitchd-stats-service` | no | compliant |
| `xtask` | no | compliant (uses `stitchd-db` transitively) |

**Result: workspace is fully compliant.** No violations detected.

---

## Why each allowed crate owns a direct `scylla` dependency

### `stitchd-db`

`stitchd-db` is the **shared persistence library** for the workspace. It is the
canonical home for `ScyllaSegmentStore` — the struct that implements the
`SegmentStore` trait using ScyllaDB as the backing store. All ScyllaDB session
construction, schema types, and query helpers live here. Owning `scylla`
directly is required for the library to compile.

### `stitchd-segmentation-service`

`stitchd-segmentation-service` is the **only production binary whose runtime
actually opens a ScyllaDB connection**. It bootstraps the `ScyllaSegmentStore`
from `stitchd-db` and therefore needs the `scylla` crate directly to pass
`Session` objects and feature flags (e.g., Scylla session builder configuration)
at binary startup. This direct dependency is intentional and expected.

### `xtask` (note: transitive only, not direct)

`xtask` does **not** list `scylla` as a direct dependency. It pulls in
`stitchd-db` for running Scylla schema migrations, acquiring the `scylla` crate
transitively. This is the correct pattern — `xtask` never constructs Scylla
types itself; it delegates entirely to `stitchd-db`.

---

## Policy: no other production binary may add `scylla` as a direct dep

The following binaries handle only business logic and communicate with ScyllaDB
**exclusively through gRPC calls to `stitchd-segmentation-service`** or through
shared library abstractions in `stitchd-db`. They MUST NOT add `scylla` as a
direct dependency:

- `stitchd-gateway`
- `stitchd-auth-service`
- `stitchd-flag-service`
- `stitchd-analytics-service`
- `stitchd-experimentation-service`
- `stitchd-stats-service`

Adding `scylla` to any of these crates would break the DDD service boundary
established in the boundaries refactor and would allow uncontrolled ScyllaDB
access outside the segmentation domain.

A CI guard script at `scripts/check_scylla_containment.sh` enforces this policy
automatically. It must pass in CI before any `Cargo.toml` change is merged.
