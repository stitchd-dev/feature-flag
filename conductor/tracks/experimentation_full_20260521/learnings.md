# Track Learnings: experimentation_full_20260521

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

The following patterns from `conductor/patterns.md` are load-bearing for this track. Re-read before starting each phase.

### ClickHouse (critical for Phases 4-5)
- **AggregatingMergeTree insert/read combiners** — use `*State` on insert, `*Merge` on read; never `finalizeAggregation` in aggregated context.
- **`sumState(Nullable(Float64))` type mismatch** — wrap with `ifNull(expr, 0.0)`.
- **Weekly partition key** — `toMonday(event_date)` + `TTL ... + INTERVAL 52 WEEK`.
- **`toFloat64OrNull` only accepts String** — never wrap numeric columns; use it ONLY for `properties['<key>']` String → Float64 coercion.
- **`clickhouse-rs` binds `?` placeholders by SQL position, not vec index** — push binds in SQL-appearance order or values bind to the wrong placeholders (cryptic `Cannot parse uuid <step-event-key>` errors). Critical for funnel + assignment JOIN queries.
- **`CREATE INDEX CONCURRENTLY` inside a transaction** — sqlx migrations wrap each file in a transaction; split into separate files for production.

### Architecture
- **Gateway lean principle** — gateway holds only gRPC clients; never direct DB. New endpoints (`/exposures`, `/timeseries`, `/recompute`, `/default-rule-distribution`) must call existing or new gRPC RPCs.
- **Fire-and-forget gRPC for analytics** — `tokio::spawn` for non-critical telemetry calls; log errors, don't block.
- **Admin vs SDK response shape** — separate `AdminFooJson` (full data) from SDK-facing `FooJson` (minimal). Don't bloat SDK responses for UI needs.
- **Bool-type flag invariant** — boolean flags always have exactly 2 variants (`true`/`false`). The `default_rule_distribution` for a bool flag must respect this.
- **gRPC service registration gotcha** — implementing `impl XxxService for Impl` is not enough; must also `.add_service(XxxServiceServer::new(impl_))` in `main.rs`.
- **Stale worktree binary on shared port** — when restarting services for testing, `ps -o comm=` to verify the binary serving a port is from the current worktree.

### Events + Metrics (from `events_metrics_20260519`)
- **Discriminated-union serde + protobuf oneof alignment** — tagged-union Rust enums with `#[serde(tag = "kind", rename_all = "snake_case")]` + `#[serde(flatten)]` match protobuf `oneof` wire format exactly.
- **Adding tonic RPCs breaks every impl — fix MockServices too** — grep `impl <Service> for` and add `Unimplemented` stubs to MockServices when adding RPC methods.
- **Proto field deprecation needs `reserved`** — `reserved <N>; reserved "<name>";` for any deleted tag.
- **`Count` metric_type is an occurrence marker** — ingestion accepts missing `value`; SDKs omit it.

### Admin UI (Frontend)
- **Formik + Yup is the only form pattern** — never ad-hoc `useState` for form state. Primitives in `admin/src/components/form/`; schemas in `admin/src/lib/validation/`.
- **`validateOnChange={false}` for async Yup validators** — prevents per-keystroke API calls.
- **`enableReinitialize` for async-loaded edit forms** — needed when `initialValues` depend on async data.
- **`key={mode}` for mode-switching Formik forms** — forces remount + validation reset.
- **`verbatimModuleSyntax`** — use `import type` for type-only imports.
- **TypeScript CLI** — `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`; never `npx tsc`.
- **Admin UI modal primitives** — reuse `Modal`, `Dropdown`, `EmptyState`, `LockOverlay` from `admin/src/components/`.
- **RBAC UI gating pattern** — `disabled` + `opacity: 0.35` (never `display:none`) for permission-gated actions.

### Auth + Gateway
- **RBAC permissions expanded from role in `crates/stitchd-auth-service/src/rbac.rs`** — explicit role→permission expansion required.
- **`require_non_system_org` middleware** — management routes (including new experiment + flag endpoints) sit behind both JWT auth and this middleware.
- **Shared `require_permission` in `routes/mod.rs`** — all sub-modules use `super::require_permission`.

### Cargo + Worktree
- **Cargo must run from the worktree root** — `cd .worktrees/experimentation_full_20260521/` or `cargo -C <worktree_path>`.
- **`bd close --no-auto` is mandatory for parallel waves** — prevents beads from claiming downstream tasks before orchestrator verifies the milestone.
- **Fix gaps as discovered** — inline-fix in-scope bugs; file beads bug (`bd create --priority 2`) for out-of-scope issues.

### Rust 2024
- **`std::env::set_var` is `unsafe`** — wrap in `unsafe {}` with `// SAFETY:` comment.
- **`gen` is reserved in Rust 2024** — use `active_gen`, `cur_gen`, `generation`.
- **Recursive types** — use `Box<T>` for recursive variants (e.g., distribution AST if any).

### Dep Constraints
- **`major.minor` is workspace canon** — pin to `major.minor`, not bare-major or patch.
- **Rust toolchain stays on `stable`** — `rust-toolchain.toml` + `dtolnay/rust-toolchain@stable`; MSRV in `[workspace.package].rust-version` is the sole enforcement.
- **`tonic 0.14` codec split** — `tonic-prost` (runtime) + `tonic-prost-build` (build); `tonic_prost_build::configure()`.
- **`clickhouse 0.15` async insert** — `client.insert::<Row>("table").await?`; explicit type annotation required.

---

<!-- Learnings from implementation will be appended below -->
