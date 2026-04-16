# Spec: Core Domain Model & Database Schema

## Overview

Establish the complete domain type system in `stitchd-core`, the full PostgreSQL
schema in `stitchd-db`, the ClickHouse schema in `stitchd-db`, the Postgres
repository layer with integration tests, and server wiring in `stitchd-server`.
This track produces the data foundation every subsequent module builds on.
No business logic beyond type invariants is included.

## Background

The platform is multi-tenant: Organisation → Projects → Environments. Feature flags
and variant definitions are project-scoped. Rules, segments, experiments, and events
are environment-scoped. SDK keys are per-environment. Users exist at organisation
level and are granted project-level access via a granular RBAC engine.

## Functional Requirements

### Domain Types (`stitchd-core`)

**ID Newtypes** — all UUID-based IDs are `struct FooId(Uuid)` with private inner
field and:
- `FooId::new()` → generates a new v4 UUID
- `FooId::from_uuid(uuid: Uuid)` → wraps an existing UUID
- Derives: `Display`, `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`,
  `Serialize`, `Deserialize`, `sqlx::Type`

IDs defined:
- Multi-tenancy: `OrganisationId`, `ProjectId`, `EnvironmentId`, `SdkKeyId`
- Identity: `UserId`, `RoleId`
- Flags: `FlagId`, `FlagKey` (string newtype, not UUID), `VariantId`
- Segmentation: `SegmentId`, `RuleId`
- Experimentation: `EventId`, `ExperimentId`, `MetricId`
- Audit: `AuditLogId`

`FlagKey` specifics:
- `FlagKey::new(s: &str)` → validates non-empty, max 255 chars;
  returns `Result<FlagKey, FlagKeyError>`

**Context Model**
- `ParameterValue`: enum `Int(i64)`, `Double(f64)`, `SemVer(semver::Version)`,
  `Str(String)`, `Bool(bool)`
- `Context`: `{ context_type: String, key: String, parameters: HashMap<String,
  ParameterValue>, private_parameters: HashSet<String> }`
- `Context::is_private(param_name: &str)` → bool
- Custom `Debug` impl: values whose key is in `private_parameters` print as
  `"[REDACTED]"`

**Flag Types**
- `FlagValueType`: enum `Bool | Int | Double | Str | Json`
- `VariantValue`: enum `BoolValue(bool)`, `IntValue(i64)`, `DoubleValue(f64)`,
  `StrValue(String)`, `JsonValue(serde_json::Value)`
- `VariantValue::matches_type(flag_type: &FlagValueType)` → bool
- `Variant`: `{ id: VariantId, key: String, value: VariantValue }`

**Multi-tenancy Types**
- All structs: `created_at: DateTime<Utc>`, `updated_at: DateTime<Utc>`,
  `deleted_at: Option<DateTime<Utc>>`, `version: i64`
- `Organisation`: `{ id, name: String }`
- `Project`: `{ id, organisation_id, name: String }`
- `Environment`: `{ id, project_id, name: String }`
- `SdkKey`: `{ id, environment_id, key_hash: String, is_active: bool,
  created_at, revoked_at: Option<DateTime<Utc>> }` — raw key never stored
- `SdkKey::has_active_key(keys: &[SdkKey])` → bool

**User Identity Types**
- `User`: `{ id, email: String, organisation_id, created_at, updated_at }`
- `Role`: `{ id, project_id, name: String, permissions: Vec<Permission> }`
- `Permission`: `{ resource_type: ResourceType, resource_pattern: String,
  action: Action }`
- `ResourceType`: enum `Environment | Flag | Segment`
- `Action`: enum `Read | Write | Publish | Admin`
- `Permission::matches(resource_type: &ResourceType, resource_name: &str)` → bool
  - Wildcard: `*` matches all; `prefix-*` matches any string starting with `prefix-`

### PostgreSQL Schema (`stitchd-db`)

All mutable entities include: `created_at TIMESTAMPTZ DEFAULT now()`,
`updated_at TIMESTAMPTZ DEFAULT now()`, `deleted_at TIMESTAMPTZ` (soft-delete),
`version BIGINT NOT NULL DEFAULT 1` (optimistic concurrency).

Migrations (one file per logical group):
1. `20260411000001_organisations_projects_environments.sql`
   - `organisations(id UUID PK, name TEXT NOT NULL)`
   - `projects(id, organisation_id FK, name TEXT NOT NULL)`
   - `environments(id, project_id FK, name TEXT NOT NULL)`
   - FK indexes on all foreign keys
2. `20260411000002_sdk_keys.sql`
   - `sdk_keys(id, environment_id FK, key_hash TEXT NOT NULL,
     is_active BOOL NOT NULL DEFAULT true, revoked_at TIMESTAMPTZ, created_at)`
   - Index: `(environment_id, is_active)`
3. `20260411000003_users_roles_permissions.sql`
   - `users(id, organisation_id FK, email TEXT NOT NULL, password_hash TEXT NOT NULL)`
     UNIQUE `(email, organisation_id)`
   - `roles(id, project_id FK, name TEXT NOT NULL)`
   - `permissions(id, role_id FK, resource_type TEXT NOT NULL,
     resource_pattern TEXT NOT NULL, action TEXT NOT NULL)`
   - `user_project_roles(user_id FK, project_id FK, role_id FK)`
     UNIQUE `(user_id, project_id, role_id)`
4. `20260411000004_feature_flags_variants.sql`
   - `feature_flags(id, project_id FK, key TEXT NOT NULL, value_type TEXT NOT NULL,
     enabled BOOL NOT NULL DEFAULT false)` UNIQUE `(key, project_id)`
   - `variants(id, flag_id FK, key TEXT NOT NULL, value JSONB NOT NULL)`
     UNIQUE `(key, flag_id)`
5. `20260411000005_segments.sql`
   - `segments(id, environment_id FK, key TEXT NOT NULL,
     segment_type TEXT NOT NULL CHECK (segment_type IN ('rule', 'list')))`
     UNIQUE `(key, environment_id)`
6. `20260411000006_audit_log.sql`
   - `audit_log(id UUID PK, actor_id UUID, resource_type TEXT NOT NULL,
     resource_id UUID NOT NULL, action TEXT NOT NULL, diff JSONB NOT NULL DEFAULT '{}',
     created_at TIMESTAMPTZ NOT NULL DEFAULT now())`
   - Indexes: `(resource_type, resource_id)`, `(actor_id)`, `(created_at)`
   - No `updated_at`, `deleted_at`, or `version` — append-only

### ClickHouse Schema (`stitchd-db`)

Migrations as `.sql` files in `crates/stitchd-db/clickhouse-migrations/`:
1. `0001_events.sql`
   - `events` table: `event_id UUID`, `environment_id UUID`,
     `context_type String`, `context_key String`, `metric_key String`,
     `metric_value_bool Nullable(UInt8)`, `metric_value_int Nullable(Int64)`,
     `metric_value_double Nullable(Float64)`, `occurred_at DateTime64(3)`
   - Engine: `MergeTree()` partitioned by `toYYYYMM(occurred_at)`,
     ordered by `(environment_id, metric_key, occurred_at)`
2. `0002_experiment_assignments.sql`
   - `experiment_assignments` table: `experiment_id UUID`, `flag_id UUID`,
     `context_key String`, `variant_key String`, `assigned_at DateTime64(3)`
   - Engine: `ReplacingMergeTree()` ordered by `(experiment_id, context_key)`

### Repository Layer (`stitchd-db`)

**Error type:**
- `RepositoryError`: `NotFound { id: String }`, `VersionConflict { expected: i64,
  actual: i64 }`, `UniqueViolation { field: String }`,
  `Database(#[from] sqlx::Error)`, `Unexpected(#[from] anyhow::Error)`
- Implements `thiserror::Error`

**Traits** (all `#[async_trait]`, return `Result<T, RepositoryError>`):
- `OrganisationRepository`: `find_by_id`, `list_all`, `create`, `update`,
  `soft_delete`
- `ProjectRepository`: `find_by_id`, `list_by_organisation`, `create`, `update`,
  `soft_delete`
- `EnvironmentRepository`: `find_by_id`, `list_by_project`, `create`, `update`,
  `soft_delete`
- `SdkKeyRepository`: `find_by_id`, `list_by_environment`, `create`, `revoke`
  - `revoke` returns `RepositoryError::UniqueViolation` if it would leave zero
    active keys
- `UserRepository`: `find_by_id`, `find_by_email`, `list_by_organisation`,
  `create`, `update`; plus `find_permissions_for_user(user_id, project_id)` →
  `Vec<Permission>`
- `FlagRepository`: `find_by_id`, `find_by_key`, `list_by_project`, `create`,
  `update`, `soft_delete`
- `VariantRepository`: `find_by_flag`, `create`, `update`, `delete`
- `SegmentRepository`: `find_by_id`, `find_by_key`, `list_by_environment`,
  `create`, `update`, `soft_delete`
- `AuditLogger`: `log(actor_id, resource_type, resource_id, action, diff)` →
  `Result<(), RepositoryError>`

**Implementations:**
- `PgFooRepository` for each trait using `sqlx::query_as!` macros throughout
- All `update` methods: increment `version`, check for conflicts
- All `list_*` queries: filter `WHERE deleted_at IS NULL`
- `AuditLogger` called inside all Pg repository mutation methods

**Integration tests** using `#[sqlx::test]` (fresh transaction per test,
auto-rollback):
- One test module per repository in `crates/stitchd-db/tests/`
- Each aggregate: create → find, update (correct version), update (stale →
  `VersionConflict`), soft_delete → absent from list, audit log entry present
- `SdkKeyRepository`: revoke last active key → error
- `UserRepository`: `find_permissions_for_user` returns correct permissions

### Server Wiring (`stitchd-server`)

- `DATABASE_URL` read at startup; `anyhow::bail!` with clear message if missing
  or unreachable
- `AppState`: add `db: PgPool` field
- `GET /health` extended: `SELECT 1` ping; response
  `{"status":"ok","db":"ok"}` or `{"status":"degraded","db":"error"}`
- `cargo sqlx prepare --workspace` run and `.sqlx/` cache committed
- CI updated to rely on `.sqlx/` cache (`SQLX_OFFLINE=true`)

## Non-Functional Requirements

- All `sqlx::query!` / `sqlx::query_as!` macros compile-time verified
- `Context::fmt` must never expose `private_parameters` values
- `SdkKey` raw key must never appear in logs, error messages, or query parameters
- Repository tests use `#[sqlx::test]` macro (fresh transaction per test,
  auto-rollback)
- ClickHouse migrations are idempotent (`CREATE TABLE IF NOT EXISTS`)

## Acceptance Criteria

- [x] `cargo test --workspace` passes with ≥90% coverage on `stitchd-core`
  and `stitchd-db`
- [x] `cargo sqlx prepare --workspace` generates clean `.sqlx/` cache
- [x] All 6 Postgres migration files run cleanly against a fresh PostgreSQL
  instance
- [x] Both ClickHouse migration files run cleanly against a fresh ClickHouse
  instance
- [x] `Context` debug output masks private parameter values as `[REDACTED]`
- [x] `VariantValue::matches_type` rejects mismatched types
- [x] `SdkKey::has_active_key` enforces min-1-active invariant
- [x] Optimistic concurrency returns `VersionConflict` on stale update
- [x] Audit log records all repository mutations
- [x] `GET /health` returns `db` status field
- [x] `cargo clippy -- -D warnings` passes

## Out of Scope

- Auth middleware and JWT validation (auth track)
- Rule engine logic (rule_engine track)
- Segment rule/list evaluation (segmentation track)
- Flag rule evaluation and percentage allocation (flag_eval track)
- Event ingestion endpoint (events track)
- Experiment management logic (experimentation track)
- gRPC or REST endpoint implementations beyond `/health`
- Admin UI
