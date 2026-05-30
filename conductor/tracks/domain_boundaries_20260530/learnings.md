# Track Learnings: domain_boundaries_20260530

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

> Full catalog: `conductor/patterns.md` (197 lines). Seeded below are the
> patterns most relevant to a lean-gateway / boundary refactor, carried over
> from the archived `boundaries_20260518` track.

### Gateway / boundary (from `gateway_lean_20260518`, `sdk_rewrite_20260516`)
- **Gateway lean principle:** When the gateway accumulates direct DB clients or
  domain logic beyond auth+routing, extract those into a dedicated service. The
  gateway should only hold gRPC client channels. Direct DB deps in the gateway
  signal a service boundary violation. (Directly the target state of this track.)
- **Fire-and-forget gRPC for analytics:** Use `tokio::spawn` for non-critical
  analytics/telemetry gRPC calls. Log errors but don't propagate — callers
  never block on telemetry side-effects.
- **Shared `require_permission` in `routes/mod.rs`:** Extract repeated
  permission-checking helpers to the route module root (`super::require_permission`)
  to prevent copy-paste drift. (A dedup target.)
- **Gateway is the sole SDK trust boundary:** Backend services NEVER validate
  SDK keys — they trust the `x-env-id` gRPC metadata header propagated by the
  gateway. Backends reject requests missing it with `Unauthenticated`.
- **gRPC service registration gotcha:** Implementing a tonic service trait is
  not enough — register via `.add_service(XxxServiceServer::new(impl_))` in
  `main.rs`, or it returns `Unimplemented` with no startup warning. (Relevant
  when moving logic into new/extended RPCs.)

### Domain model change order (from `flags_crud_20260512`)
- **Field-add chain:** `stitchd-core` structs → DB repo queries → domain service
  → proto definition → proto mapping (`mapping.rs`) → gateway handler. Skipping
  a step causes compile errors deep in the chain. (Applies when moving logic
  into services + extending protos.)
- **Admin vs SDK response shape:** Keep a separate `AdminFooJson` (full data)
  from the minimal SDK-facing `FooJson`. Never bloat the SDK response for UI.

### Worktree / build gotchas (from `env_sdk_rbac_20260429`)
- **Cargo runs from the worktree root:** Running `cargo test/clippy` from the
  main repo root compiles `main`, silently ignoring worktree changes. Always
  `cd .worktrees/domain_boundaries_20260530/` (or `-C <path>`) before Cargo.
- **Verify the binary on a port:** `ps -o comm=` to confirm a service on a port
  is from the current worktree — stale binaries silently lack new gRPC methods.

### Infra credentials (from `sdk_rewrite_20260516`)
- **ClickHouse creds at startup:** services writing to CH need
  `CLICKHOUSE_USER=stitchd CLICKHOUSE_PASSWORD=stitchd CLICKHOUSE_DB=stitchd`;
  the client defaults to `user=default`/no-password and fails auth silently.
- **Local test infra (this session):** Postgres + ClickHouse + ScyllaDB run via
  OrbStack (`docker compose up postgres clickhouse scylladb`). Local Postgres
  creds are `stitchd:stitchd` against DB `stitchd` (NOT `postgres`/`stitchd_test`);
  `#[sqlx::test]` connects to the `stitchd` DB to clone per-test DBs. ClickHouse
  HTTP on `:8123`, Scylla CQL on `:9042`. CI's Coverage job starts only
  postgres+clickhouse (no Scylla) and runs with `cargo test` fail-fast.

### Verification gotcha discovered this session (pre-track)
- **CI fail-fast masks cascades:** CI's `cargo llvm-cov` (= `cargo test` without
  `--no-fail-fast`) STOPS at the first failing test binary, so later failures
  stay hidden until earlier ones are fixed. When chasing "CI still failing,"
  always run the full suite locally with `--no-fail-fast` to enumerate ALL
  failures at once instead of fixing them one CI round at a time.

---

<!-- Learnings from implementation will be appended below -->
