# Plan: Core Domain Model & Database Schema

## Phase 1: Core Domain Types (`stitchd-core`)
<!-- execution: parallel -->
<!-- depends: -->

- [ ] Task: ID Newtypes — `OrganisationId`, `ProjectId`, `EnvironmentId`,
  `SdkKeyId`, `UserId`, `RoleId`, `FlagId`, `FlagKey` (string newtype),
  `VariantId`, `SegmentId`, `RuleId`, `EventId`, `ExperimentId`, `MetricId`,
  `AuditLogId`
  - All UUID-based IDs: private inner field; `FooId::new()` → v4 UUID;
    `FooId::from_uuid(uuid)` → wraps existing
  - Derives: `Display`, `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`,
    `Serialize`, `Deserialize`, `sqlx::Type`
  - `FlagKey::new(s)` validates non-empty, max 255 chars;
    returns `Result<FlagKey, FlagKeyError>`
  - Unit tests: `FlagKey` validation (empty, too long, valid)
  <!-- files: crates/stitchd-core/src/id.rs -->

- [ ] Task: `ParameterValue` + `Context` model
  - `ParameterValue`: enum `Int(i64)`, `Double(f64)`, `SemVer(semver::Version)`,
    `Str(String)`, `Bool(bool)` — all variants derive `Clone`, `PartialEq`,
    `Serialize`, `Deserialize`
  - `Context`: `{ context_type: String, key: String, parameters: HashMap<String,
    ParameterValue>, private_parameters: HashSet<String> }`
  - `Context::is_private(param_name: &str)` → bool
  - Custom `Debug` impl: prints `"[REDACTED]"` for values in `private_parameters`
  - Unit tests: private param masking in debug output, `is_private`,
    non-private param access
  <!-- files: crates/stitchd-core/src/context.rs -->

- [ ] Task: Flag types — `FlagValueType`, `VariantValue`, `Variant`
  - `FlagValueType`: enum `Bool | Int | Double | Str | Json`
  - `VariantValue`: enum `BoolValue(bool)`, `IntValue(i64)`, `DoubleValue(f64)`,
    `StrValue(String)`, `JsonValue(serde_json::Value)`
  - `VariantValue::matches_type(flag_type: &FlagValueType)` → bool
  - `Variant`: `{ id: VariantId, key: String, value: VariantValue }`
  - Unit tests: all type match/mismatch combinations
  <!-- files: crates/stitchd-core/src/flag.rs -->

- [ ] Task: Multi-tenancy types — `Organisation`, `Project`, `Environment`,
  `SdkKey`
  - All structs: `created_at: DateTime<Utc>`, `updated_at: DateTime<Utc>`,
    `deleted_at: Option<DateTime<Utc>>`, `version: i64`
  - `SdkKey`: `{ id, environment_id, key_hash: String, is_active: bool,
    created_at, revoked_at: Option<DateTime<Utc>> }`
  - `SdkKey::has_active_key(keys: &[SdkKey])` → bool
  - Unit tests: `has_active_key` with zero active, one active, mixed
  <!-- files: crates/stitchd-core/src/tenant.rs -->

- [ ] Task: User identity types — `User`, `Role`, `Permission`,
  `ResourceType`, `Action`
  - `ResourceType`: enum `Environment | Flag | Segment`
  - `Action`: enum `Read | Write | Publish | Admin`
  - `Permission`: `{ resource_type: ResourceType, resource_pattern: String,
    action: Action }`
  - `Permission::matches(resource_type: &ResourceType, resource_name: &str)`
    → bool; wildcard: `*` matches all; `prefix-*` matches prefix
  - `User`: `{ id: UserId, email: String, organisation_id, created_at,
    updated_at }`
  - `Role`: `{ id, project_id: ProjectId, name: String,
    permissions: Vec<Permission> }`
  - Unit tests: wildcard matching (`*`, `payments-*`, exact, no match)
  <!-- files: crates/stitchd-core/src/user.rs -->

- [ ] Task: Wire `lib.rs` re-exports and verify full unit test suite
  - `pub mod id; pub mod context; pub mod flag; pub mod tenant; pub mod user;`
  - `cargo test -p stitchd-core` must pass with ≥90% coverage
  - `cargo clippy -p stitchd-core -- -D warnings` must pass clean
  <!-- files: crates/stitchd-core/src/lib.rs -->
  <!-- depends: task1, task2, task3, task4, task5 -->

## Phase 2: Database Schemas
<!-- execution: parallel -->
<!-- depends: -->

- [ ] Task: Set up `sqlx` migrations directory and configure `stitchd-db`
  - Create `crates/stitchd-db/migrations/` directory
  - Create `crates/stitchd-db/clickhouse-migrations/` directory
  - Add `DATABASE_URL` and `CLICKHOUSE_URL` to `.env.example`
  - Verify `sqlx-cli` available: `cargo install sqlx-cli
    --no-default-features --features rustls,postgres`
  <!-- files: crates/stitchd-db/migrations/.gitkeep,
    crates/stitchd-db/clickhouse-migrations/.gitkeep,
    .env.example -->

- [ ] Task: PG Migration 001 — `organisations`, `projects`, `environments`
  - All tables: `id UUID PK DEFAULT gen_random_uuid()`, `created_at`,
    `updated_at`, `deleted_at TIMESTAMPTZ`, `version BIGINT NOT NULL DEFAULT 1`
  - `projects.organisation_id REFERENCES organisations(id)`
  - `environments.project_id REFERENCES projects(id)`
  - Index on all FK columns
  <!-- files: crates/stitchd-db/migrations/20260411000001_organisations_projects_environments.sql -->

- [ ] Task: PG Migration 002 — `sdk_keys`
  - `environment_id FK`, `key_hash TEXT NOT NULL`,
    `is_active BOOL NOT NULL DEFAULT true`, `revoked_at TIMESTAMPTZ`,
    `created_at`
  - Index: `(environment_id, is_active)`
  <!-- files: crates/stitchd-db/migrations/20260411000002_sdk_keys.sql -->

- [ ] Task: PG Migration 003 — `users`, `roles`, `permissions`,
  `user_project_roles`
  - `users`: UNIQUE `(email, organisation_id)`; `password_hash TEXT NOT NULL`
  - `roles`: `project_id FK`, `name TEXT NOT NULL`
  - `permissions`: `role_id FK`, `resource_type TEXT NOT NULL`,
    `resource_pattern TEXT NOT NULL`, `action TEXT NOT NULL`
  - `user_project_roles`: UNIQUE `(user_id, project_id, role_id)`
  <!-- files: crates/stitchd-db/migrations/20260411000003_users_roles_permissions.sql -->

- [ ] Task: PG Migration 004 — `feature_flags`, `variants`
  - `feature_flags`: UNIQUE `(key, project_id)`; `value_type TEXT NOT NULL`;
    `enabled BOOL NOT NULL DEFAULT false`
  - `variants`: UNIQUE `(key, flag_id)`; `value JSONB NOT NULL`
  <!-- files: crates/stitchd-db/migrations/20260411000004_feature_flags_variants.sql -->

- [ ] Task: PG Migration 005 — `segments`
  - UNIQUE `(key, environment_id)`
  - `segment_type TEXT NOT NULL CHECK (segment_type IN ('rule', 'list'))`
  <!-- files: crates/stitchd-db/migrations/20260411000005_segments.sql -->

- [ ] Task: PG Migration 006 — `audit_log`
  - `id UUID PK`, `actor_id UUID`, `resource_type TEXT NOT NULL`,
    `resource_id UUID NOT NULL`, `action TEXT NOT NULL`,
    `diff JSONB NOT NULL DEFAULT '{}'`,
    `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`
  - Indexes: `(resource_type, resource_id)`, `(actor_id)`, `(created_at)`
  - No `updated_at`, `deleted_at`, or `version` — append-only
  <!-- files: crates/stitchd-db/migrations/20260411000006_audit_log.sql -->

- [ ] Task: ClickHouse Migration 001 — `events`
  - `events(event_id UUID, environment_id UUID, context_type String,
    context_key String, metric_key String,
    metric_value_bool Nullable(UInt8), metric_value_int Nullable(Int64),
    metric_value_double Nullable(Float64), occurred_at DateTime64(3))`
  - Engine: `MergeTree()` partitioned by `toYYYYMM(occurred_at)`,
    ordered by `(environment_id, metric_key, occurred_at)`
  - `CREATE TABLE IF NOT EXISTS` (idempotent)
  <!-- files: crates/stitchd-db/clickhouse-migrations/0001_events.sql -->

- [ ] Task: ClickHouse Migration 002 — `experiment_assignments`
  - `experiment_assignments(experiment_id UUID, flag_id UUID,
    context_key String, variant_key String, assigned_at DateTime64(3))`
  - Engine: `ReplacingMergeTree()` ordered by `(experiment_id, context_key)`
  - `CREATE TABLE IF NOT EXISTS` (idempotent)
  <!-- files: crates/stitchd-db/clickhouse-migrations/0002_experiment_assignments.sql -->

- [ ] Task: Run all PG migrations against local PostgreSQL and verify schema
  - `cargo sqlx migrate run`
  - Verify all tables created: `psql $DATABASE_URL -c '\dt'`
  <!-- depends: task1, task2, task3, task4, task5, task6, task7 -->

- [ ] Task: Run ClickHouse migrations against local ClickHouse and verify
  - Execute both SQL files against ClickHouse instance
  - Verify tables: `SHOW TABLES` in ClickHouse
  <!-- depends: task1, task8, task9 -->

## Phase 3: Repository Layer (`stitchd-db`)
<!-- execution: sequential -->
<!-- depends: phase1, phase2 -->

- [ ] Task: `RepositoryError` enum
  - Variants: `NotFound { id: String }`,
    `VersionConflict { expected: i64, actual: i64 }`,
    `UniqueViolation { field: String }`, `Database(#[from] sqlx::Error)`,
    `Unexpected(#[from] anyhow::Error)`
  - Implement `thiserror::Error`

- [ ] Task: Repository traits — one per aggregate root
  - `OrganisationRepository`: `find_by_id`, `list_all`, `create`, `update`,
    `soft_delete`
  - `ProjectRepository`: `find_by_id`, `list_by_organisation`, `create`,
    `update`, `soft_delete`
  - `EnvironmentRepository`: `find_by_id`, `list_by_project`, `create`,
    `update`, `soft_delete`
  - `SdkKeyRepository`: `find_by_id`, `list_by_environment`, `create`,
    `revoke`
  - `UserRepository`: `find_by_id`, `find_by_email`, `list_by_organisation`,
    `create`, `update`;
    `find_permissions_for_user(user_id, project_id)` → `Vec<Permission>`
  - `FlagRepository`: `find_by_id`, `find_by_key`, `list_by_project`,
    `create`, `update`, `soft_delete`
  - `VariantRepository`: `find_by_flag`, `create`, `update`, `delete`
  - `SegmentRepository`: `find_by_id`, `find_by_key`, `list_by_environment`,
    `create`, `update`, `soft_delete`
  - `AuditLogger`: `log(actor_id, resource_type, resource_id, action, diff)`
    → `Result<(), RepositoryError>`
  - All `#[async_trait]`, return `Result<T, RepositoryError>`

- [ ] Task: `PgOrganisationRepository`, `PgProjectRepository`,
  `PgEnvironmentRepository`
  - All `update` methods: increment `version`, check for conflicts
  - All `list_*`: filter `WHERE deleted_at IS NULL`
  - `sqlx::query_as!` throughout; no string-formatted SQL
  - `PgAuditLogger` called inside every mutation

- [ ] Task: `PgSdkKeyRepository`
  - `revoke`: sets `is_active = false`, `revoked_at = now()`
  - Returns `RepositoryError::UniqueViolation` if it would leave zero active
    keys (check count before revoking)

- [ ] Task: `PgUserRepository` + permission resolution
  - `find_permissions_for_user(user_id, project_id)` → `Vec<Permission>`
    via JOIN: `user_project_roles → roles → permissions`

- [ ] Task: `PgFlagRepository` + `PgVariantRepository`
  - `FlagRepository::find_by_key(key, project_id)` → unique lookup
  - `VariantRepository::find_by_flag(flag_id)` → `Vec<Variant>`

- [ ] Task: `PgSegmentRepository`
  - `find_by_key(key, environment_id)` → unique lookup

- [ ] Task: `AuditLogger` trait + `PgAuditLogger`
  - `async fn log(&self, actor_id: UserId, resource_type: &str,
    resource_id: Uuid, action: &str, diff: serde_json::Value)
    → Result<(), RepositoryError>`
  - Inserted into all `Pg*Repository` mutation methods

- [ ] Task: Integration tests using `#[sqlx::test]`
  - One test module per repository in `crates/stitchd-db/tests/`
  - Per aggregate: create → find, update (correct version),
    update (stale → `VersionConflict`), soft_delete → absent from list,
    audit log entry created
  - `SdkKeyRepository`: revoke last active key → error
  - `UserRepository`: `find_permissions_for_user` returns correct permissions

- [ ] Task: Wire `stitchd-db` `lib.rs` and verify full integration test suite
  - Re-export all public types, traits, and Pg implementations
  - `cargo test -p stitchd-db` must pass with ≥90% coverage
  - `cargo clippy -p stitchd-db -- -D warnings` must pass clean

## Phase 4: Server Wiring & sqlx Offline Mode
<!-- execution: sequential -->
<!-- depends: phase3 -->

- [ ] Task: Wire `DATABASE_URL` + `PgPool` into `AppState` in `stitchd-server`
  - Read at startup; `anyhow::bail!` with clear message if missing or
    unreachable
  - `AppState`: add `db: PgPool` field; pass to all Axum handlers via
    `State<AppState>`

- [ ] Task: Extend `GET /health` to include database liveness
  - `SELECT 1` ping; response:
    `{"status":"ok","db":"ok"}` or `{"status":"degraded","db":"error"}`

- [ ] Task: Generate sqlx offline cache
  - With local DB running: `cargo sqlx prepare --workspace`
  - Commit `.sqlx/` directory to repo

- [ ] Task: Verify `SQLX_OFFLINE=true cargo build --workspace` passes clean

- [ ] Task: Update CI — add `SQLX_OFFLINE: true` env var; rely on `.sqlx/`
  cache; CI validates cache is up to date on every PR
