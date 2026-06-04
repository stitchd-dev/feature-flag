# Track Learnings: flag_lifecycle_20260604

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

> Read `conductor/patterns.md` in full before starting — only the directly-relevant
> patterns are surfaced here.

### New tables / repositories (most relevant — this track adds 3 PG tables)
- **`sqlx::query_as` for new tables:** New repository modules should use
  `sqlx::query_as::<_, Row>(r"...")` raw strings instead of `sqlx::query!` macros to
  avoid offline-compilation failures until the `.sqlx` cache is populated.
  (from: scheduled_stats_20260423)
- **`cargo sqlx prepare` skips `#[cfg(test)]`** and may *delete* previously-cached
  test-only entries — always compile tests against a live `DATABASE_URL`, never
  `SQLX_OFFLINE=true`; re-verify after every `prepare`. (from: scheduled_stats_20260423)
- **`STITCHD_DATABASE_URL` vs `DATABASE_URL`:** alias `export DATABASE_URL="$STITCHD_DATABASE_URL"`
  before any sqlx-cli / `cargo sqlx prepare` command. (from: boundaries_20260518)
- **`CREATE INDEX CONCURRENTLY` cannot run inside a migration transaction** — split into
  its own file or run manually in prod. (from: db_optim_20260516)

### Scheduler (this track adds stitchd-schedule-service)
- `ticker.tick()` fires once immediately on entry, then every interval.
- **`chrono::Duration` is NOT `std::time::Duration`** — convert via `.to_std().unwrap()`.
- Graceful shutdown: `tokio::select!` over `ctrl_c()` + SIGTERM (`#[cfg(unix)]`).
- Prometheus: `PrometheusBuilder::new().install_recorder()` → handle as Axum state.

### Evaluation / prerequisites
- `evaluate_flag` is a **pure function** (engine.rs:73); prerequisite gate slots at
  engine.rs:129 (after disabled-flag check, before rule iteration).
- Cross-flag resolution already runs a **topological sort + cycle detection** in
  `rule_engine/orchestrator.rs:17-77`, pre-populating the `evaluated_flags` map that
  `Condition::FlagEvaluatedAs` (eval_leaf.rs:125) reads. Extend this for prerequisite edges.
- ID type names: confirm in `crates/stitchd-core/src/id.rs` (e.g. `OrganisationId`, not `OrgId`).
- **Recursive types** (expression/graph trees) need `Box<T>` for recursive variants.

### Proto / transport
- Backward-compatible additions only (new messages/fields/RPCs; never renumber).
- SDK-sync `FeatureFlag` leaves admin-only fields empty; populate `prerequisites` +
  `fallback_variant_key` in BOTH definition-sync and evaluate-preview snapshots so SDK +
  preview gate identically.

### Parallel worker-wave discipline (this track is worker-wave)
- Each worker in its own `git worktree`; run `cargo test/clippy` from inside the worktree.
- Workers close beads tasks with plain `bd close <id>` (`--force` if a phantom dep on an
  open sibling phase blocks it); the documented `--no-auto` is unreliable in current beads.
- After `--no-ff` merge, delete worker branches with `git branch -D` (not `-d`).
- Write the **file-ownership table** into each worker prompt; shared seams (e.g. a
  `Service::new(...)` ctor) named explicitly in both prompts.
- **Fix in-scope gaps inline**; file clearly out-of-scope issues as `bd create -p 2`.

### CI gotcha (stats-service live-CH step) — applies if adding self-seeding tests
- The Coverage job has a SEPARATE "Live-ClickHouse integration tests (stats-service)"
  step that names each `--test` target explicitly. This track adds `stitchd-schedule-service`,
  not stats-service tests — but if any self-seeding live-DB `tests/*.rs` is added there,
  keep that `--test` list in `.github/workflows/ci.yml` in sync or CI goes red on next push.

---

<!-- Learnings from implementation will be appended below -->

## 2026-06-04 — Phase 1 Task 1 (deps + tech-stack)
- Added `chrono-tz = "0.10"` + `rrule = "0.14"` to `[workspace.dependencies]`. Both are
  compatible with workspace `chrono` 0.4; rrule 0.14 depends on chrono 0.4 + chrono-tz 0.10.
- `cargo metadata` does NOT pull a `[workspace.dependencies]` entry into the graph (or
  Cargo.lock) until a member crate actually consumes it via `dep = { workspace = true }`.
  So after this task alone the deps are absent from Cargo.lock — that is expected; they
  appear once task 4 wires them into `stitchd-core`. Verified `cargo metadata` resolves clean.
- Documented both deps + the `stitchd-schedule-service` architecture decision in
  `conductor/tech-stack.md` (comment block + Key Dependencies rows) per the workflow
  tech-stack-before-use rule.

## 2026-06-04 — Phase 1 Task 2 (PG migration)
- `20260604000001_lifecycle_automation.sql`: `scheduled_changes`,
  `scheduled_change_runs`, `flag_prerequisites` (+ `feature_flags.fallback_variant_id`),
  `entity_dependencies`. Matched baseline conventions: `gen_random_uuid()` PK default,
  `TIMESTAMPTZ NOT NULL DEFAULT now()`, soft-delete via `deleted_at TIMESTAMPTZ`,
  `version BIGINT DEFAULT 1`, named `CHECK` constraints, partial soft-delete indexes.
- `created_by` does not exist as a column convention elsewhere (audit_log uses `actor_id`);
  used a nullable `created_by UUID` per spec (system/scheduler actor = NULL).
- GOTCHA: `cargo sqlx migrate run` against the shared dev Postgres FAILS with
  "migration 20260525000001 ... has been modified" — the shared DB's `_sqlx_migrations`
  history was recorded against a different baseline checksum (sibling worktree). This is
  NOT a problem with the new migration. `#[sqlx::test(migrations = "./migrations")]`
  provisions a FRESH isolated DB and runs every migration from scratch, so it both proves
  the new migration applies cleanly on top of the full baseline AND sidesteps the shared-DB
  checksum drift. 5 runtime-query smoke tests pass (no `query!` macros — offline cache empty).
- Postgres reachable at `postgres://stitchd:stitchd@localhost:5432/stitchd`; the
  `$STITCHD_DATABASE_URL` env var is set by the user profile but does NOT persist into the
  agent's bash shell — export it explicitly per command.

## 2026-06-04 — Phase 1 Task 3 (proto)
- flag_sync.proto: new `FlagPrerequisite` message (prerequisite_flag_id/key,
  required_variant_id/key) + `FeatureFlag.prerequisites` (tag 15, repeated) +
  `FeatureFlag.fallback_variant_key` (tag 16). Carries both UUID + key so it rides BOTH
  SDK definition-sync (keys) and admin/preview (UUIDs) snapshots — they gate identically.
- flag_service.proto: `SetPrerequisites`/`GetPrerequisites` RPCs + req/resp messages.
- NEW proto/schedule/v1/schedule_service.proto: ScheduledChange + ScheduledChangeRun +
  4 enums (ScheduleEntityType/ScheduleKind/ScheduleStatus/ScheduleRunOutcome); RPCs
  Create/List/Get/Cancel/Pause/ResumeScheduledChange + internal ListDueChanges. Registered
  in build.rs + lib.rs (`pub mod schedule::v1`).
- GOTCHA: there is NO proto error enum in this codebase. 409 FLAG_LOCKED_BY_EXPERIMENT is a
  gateway-side sentinel STRING (`flag_locked_by_experiment:<uuid>` on a tonic
  FailedPrecondition status, decoded in stitchd-gateway/src/error.rs into a structured
  variant). DEPENDENCY_EXISTS mirrors this as a `dependency_exists:` sentinel — that's a
  Phase 4/6 service+gateway concern, NOT a proto change. Documented the sentinel convention
  in the schedule proto's ScheduledChangeRun.detail comment.
- NO experiment/segment proto changes needed: `TransitionExperiment` (full ExperimentStatus
  = start/pause/resume/stop/archive) and segment `UpdateAdminSegment`/`MutateSegment` already
  exist for the scheduler to dispatch to. Convention: payloads as JSON strings + timestamps
  as epoch-ms int64 (no google.protobuf well-known types anywhere in the tree).
- `cargo build -p stitchd-proto` regenerates stubs; 25 proto compilation tests green
  (added FlagPrerequisite round-trip, prerequisite RPC types, schedule types + enums + stubs).

## 2026-06-04 — Phase 1 Task 4 (core domain types)
- `crates/stitchd-core/src/prerequisite.rs`: `FlagPrerequisite { prerequisite_flag_id,
  required_variant_id }` + `PrerequisiteGate { prerequisites, fallback_variant_id }`,
  serde + openapi-gated `utoipa::ToSchema` like sibling types. (Gate APPLICATION in
  evaluate_flag is Phase 2 — only the types here.)
- `crates/stitchd-core/src/schedule.rs`: `ScheduleKind`/`ScheduleStatus`/`ScheduleEntityType`
  enums (snake_case serde), `RecurrenceSpec { rrule, tz }`, `ScheduledChange` summary,
  `RecurrenceSpec::next_occurrence(after) -> Result<Option<DateTime<Utc>>, RecurrenceError>`.
- rrule 0.14 API: `RRuleSet::from_str(full_rfc5545)` parses a `DTSTART;TZID=...` + `RRULE:` body
  in one go (the DTSTART carries the IANA TZID — that zone, not the redundant `tz` field, is
  authoritative for the math). `rrule::Tz` wraps `chrono_tz::Tz` (`.into()`); `.after(dt)` then
  `.all(1)` gives the next occurrence. Convert results back to UTC with `.with_timezone(&Utc)`.
- GOTCHA: `RRuleSet::all()`'s `after` bound is INCLUSIVE (the 4th arg to internal
  `collect_with_error` is `inclusive=true`). For strict "next AFTER" semantics (so re-querying
  at the exact fire instant returns the *next* window) bump the bound by
  `chrono::Duration::seconds(1)` — RRULE occurrences are second-granular so this never skips.
- DST proof test: weekday 09:00 America/New_York. Pre-spring-forward fire = 14:00 UTC (EST,
  UTC-5); post-spring-forward (after 2026-03-08) = 13:00 UTC (EDT, UTC-4). UTC hour shifts
  14→13 while local stays 09:00 → DST-correct. `chrono::Duration` is NOT std Duration (already
  in inherited learnings) — used `chrono::Duration::seconds` directly on a `DateTime<Utc>`.

## 2026-06-04 — Phase 1 final-verification compile fixes (flag-service)
- The additive proto changes from Task 3 surfaced two REQUIRED downstream edits in
  `stitchd-flag-service` (caught by `cargo build --workspace`, fixed inline per the
  fix-gaps-as-discovered rule):
  1. The production proto-mapping literal `build_feature_flag_proto` (mapping.rs:447)
     constructs `FeatureFlag` exhaustively (no `..Default::default()`), so the new
     `prerequisites` + `fallback_variant_key` fields had to be added explicitly (left empty —
     populated in Phase 4 once FlagRecord carries the gate). Test-side literals already used
     `..Default::default()` and needed no change.
  2. Adding `SetPrerequisites`/`GetPrerequisites` RPCs to the FlagService proto service made
     the generated trait require those methods on `FlagServiceImpl`. Added `unimplemented!`-style
     stubs (return `Status::unimplemented`) so the contract is satisfied in Phase 1; real impl
     is Phase 4. gateway / experimentation-service / SDK FeatureFlag literals already used
     `..Default::default()` and compiled unchanged.

## 2026-06-04 — Phase 2 (prerequisite eval-time gate, stitchd-core)
- SHAs: Red 5349fff (test), Green 99e9bed (feat), Refactor 6e851c4 (refactor).
- **Two evaluation paths exist and were NOT previously connected.** `evaluate_flag`/
  `evaluate_one` (engine.rs) evaluate a SINGLE flag and built an EMPTY `evaluated_flags`
  map internally — so cross-flag `FlagEvaluatedAs` always resolved false on the unified
  preview/SDK path. The multi-flag `orchestrator::evaluate_flags` is a separate
  `(FlagId, Vec<Rule>)` resolver that DOES populate the map + topo-sort. The gate needs
  the resolved prereq variants, so the design threads a pre-resolved
  `&HashMap<FlagId, Option<VariantId>>` into a NEW entry point rather than rewiring 27
  `evaluate_flag(` call sites.
- **Signature-stability trick:** added `evaluate_flag_with_prerequisites(...)` taking the
  resolved map; `evaluate_flag` delegates with an EMPTY map. Empty map ⇒ any configured
  prerequisite is treated as unmet ⇒ gate fails CLOSED → fallback. This keeps every
  existing caller (preview.rs, sdk client.rs, parity tests) compiling + behaving safely;
  Phase 4 (flag-service snapshot) and Phase 7 (SDK snapshot) call the new entry point with
  a populated map. The new map is ALSO fed into `EvaluationInput.evaluated_flags` so
  `FlagEvaluatedAs` finally resolves on the unified path too (latent fix).
- **Unmet = absent OR None OR mismatched variant.** `evaluated_flags.get(id).copied().flatten()`
  collapses "unknown flag" (outer None) and "disabled / no variant" (inner None) into the
  same "no resolved variant" → both ≠ required ⇒ unmet. Disabled flag short-circuits BEFORE
  the gate (FlagDisabled wins over PrerequisiteFailed).
- **Gate slots at engine.rs §1b** — after the `!flag.record.enabled` check, before segment
  resolution + rule iteration. First failing prereq is reported (later satisfied prereqs do
  NOT rescue the gate). Fallback variant = `flag.prerequisites.fallback_variant_id` if it
  names a real variant, else the off/disabled default (`Flag::prerequisite_fallback_variant`).
- **Trace shape:** `EvaluationTrace.prerequisite_failure: Option<PrerequisiteFailureTrace>`
  (skip_serializing_if None). `PrerequisiteFailureTrace { prerequisite_flag_id,
  required_variant_id, resolved_variant_id: Option, fallback_variant_key }` — IDENTIFIERS
  ONLY, no context/parameter values, so the NFR "prerequisite traces must not leak
  privateParameters" holds by construction. New `EvalOutcome::PrerequisiteFailed {
  prerequisite_flag_id }` (snake_case kind tag `prerequisite_failed`).
- **Orchestrator extension:** new `build_dependency_graph(&[(FlagId, Vec<Rule>,
  Vec<FlagPrerequisite>)])` merges `extract_flag_deps` (FlagEvaluatedAs rule edges) with
  prerequisite edges into the same `HashMap<FlagId, HashSet<FlagId>>` that Kahn's
  topological_sort consumes — so prereq flags resolve first AND prereq cycles raise the
  existing `RuleEngineError::CyclicFlagDependency`. `evaluate_flags` delegates to
  `evaluate_flags_with_prerequisites` with empty prereq lists (zero behaviour change).
- **Flag-aggregate field ripple (the field-add blast radius):** adding `Flag.prerequisites:
  PrerequisiteGate` (#[serde(default)]) forced updating EVERY full `Flag { record,
  hashing_config, rules, variants, .. }` literal. Sites fixed (all = empty
  `PrerequisiteGate::default()`):
    - crates/stitchd-flag-service/src/service.rs (preview path — Phase 4 loads the real gate)
    - sdks/rust/src/client.rs `convert_proto_flag_to_core` (Phase 7 wires proto→gate)
    - crates/stitchd-core/src/{flag.rs, evaluation/engine.rs, evaluation/preview.rs} test literals
    - crates/stitchd-flag-service/tests/{evaluate_preview_byte_equivalence,preview_cross_context_bundle}.rs
    - sdks/rust/tests/{parity_with_preview (×2), exclusion_gating, e2e_cross_context_hashing}.rs
  GOTCHA: `cargo build --workspace --all-targets` did NOT surface the FEATURE-GATED sdk test
  targets (parity/exclusion/e2e need `--features test-util`); use
  `cargo test --workspace --all-targets --all-features --no-run` to compile EVERY test target
  before declaring the field-add green.
- **SDK `EvalOutcome` is a separate taxonomy** (client.rs) — adding core
  `PrerequisiteFailed` made the SDK's `match core_res.outcome` non-exhaustive. Added a SDK
  `EvalOutcome::PrerequisiteFailed` (string `prerequisite_failed`) + the match arm. Phase 7
  fills in SDK prereq tests; the variant is in place now.
- **Gateway mock FlagService impls** (tests/flag_admin_metadata.rs, tests/flag_lock_integration.rs
  ×3 impls) needed `set_prerequisites`/`get_prerequisites` stubs (the Phase-1 proto RPCs) to
  satisfy the generated trait; and one gateway `FeatureFlag {…}` test literal (routes/flags.rs)
  that enumerates fields needed the new `prerequisites: vec![]` + `fallback_variant_key:
  String::new()` proto fields. Kept the build green for the Phase-3 worker.
- **Coverage:** `cargo llvm-cov -p stitchd-core --lib` on new paths — engine.rs 99.1% lines /
  98.1% regions, types.rs 100%, orchestrator.rs 100% lines, prerequisite.rs 100%, flag.rs new
  `prerequisite_fallback_variant` fully covered. The purity test (`evaluation/purity.rs`) still
  passes — the gate is pure map-lookups, no I/O.

## 2026-06-04 — Phase 3 (scheduler core + flag scheduling) — NEW crate stitchd-schedule-service
- **NEW crate `crates/stitchd-schedule-service`** (binary `stitchd-schedule-service`), added to
  workspace `[workspace.members]`. Env vars (all `STITCHD_` prefixed):
  `STITCHD_DATABASE_URL` (req), `STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS` (default 60),
  `STITCHD_SCHEDULE_CLAIM_BATCH` (default 100), `STITCHD_SCHEDULE_SERVICE_HTTP_PORT` (9201),
  `STITCHD_SCHEDULE_SERVICE_GRPC_PORT` (50057), `STITCHD_FLAG_SERVICE_GRPC_URL`
  (`http://localhost:50051`). Mirrors stats-service main.rs (tokio interval, Prometheus,
  graceful shutdown ctrl_c+SIGTERM). SHAs: 3.1 db repo `257a655`, 3.2 crate+loop `a818fc5`,
  3.3 flag apply `4e1ee90`.
- **`ScheduledChangeRepository`** (`repository/pg/scheduled_changes.rs`, registered in pg/mod.rs
  + re-exported from db lib.rs): runtime `sqlx::query_as::<_,Row>` (no `query!` macros — offline
  cache empty, per inherited learning). Text-backed `sqlx::Type` enums (ScheduleStatus/
  ScheduleKind/RunOutcome) mirror the migration's `CHECK` values. 13 `#[sqlx::test]` cases.
- **Restart-safe claim** = `claim_due(&mut tx, as_of, limit)` runs
  `... WHERE status IN ('pending','active') AND next_run_at <= $1 ... FOR UPDATE SKIP LOCKED`.
  The scheduler does apply + `append_run_tx` + `advance_recurring`/`finalize_one_shot` ALL
  inside that same claim tx, so the row stays row-locked for the whole apply; a concurrent
  replica SKIP-LOCKs past it; a crash mid-apply rolls back → re-claimed next tick (missed-tick
  catch-up). One-shot → terminal status; recurring `next_run_at` always moves strictly forward
  → neither double-applies. Tested via two overlapping txns on the shared pool (skip-locked) +
  a second-tick idempotency assertion.
- **Testable scheduler core**: `scheduler::process_due_changes` takes an injected `Clock`
  (FixedClock in tests) + an injected `Applier` (StubApplier) — no wall-clock sleeps. Recurring
  next-run recompute calls core `RecurrenceSpec::next_occurrence(now)` (DST-correct); exhausted
  rule (None) → row marked `applied`. **A9**: a skip/fail on a RECURRING change still advances to
  the next window (only one-shot goes terminal-failed on skip).
- **Locked-flag skip (3.3)**: `apply/flag.rs` `classify_status` keys on
  `Status::code()==FailedPrecondition && message().starts_with("flag_locked_by_experiment:")`
  → `ApplyOutcome::Skipped(sentinel)` (run recorded `skipped`, loop never errors). The sentinel
  prefix is duplicated as a `pub const` here (matching `stitchd_flag_service::error::
  FLAG_LOCKED_STATUS_PREFIX` + the gateway decode) — there is no shared const crate for it.
- **prost messages are NOT serde** → the JSONB `mutation_payload` is a hand-written serde mirror
  (`FlagMutationPayload` + `FlagBody`/`VariantBody`/`VariantValueBody`) rebuilt into
  `MutateFlagRequest`. Covers enable/disable (`enabled_override`), kind, version, project/env
  scope, and variant replacement; rules-replacement is a documented future extension on the
  mirror. Apply seam = `apply::Applier` trait + entity-type `Dispatcher` (flag only; segment/
  experiment Phase 5 return `Failed("unsupported entity type")` rather than silently dropping).
- **Actor attribution**: the scheduler is just a flag-service gRPC client carrying no end-user
  identity; flag-service's existing MutateFlag path does the audit-write + version-bump and
  attributes to the system/scheduler actor. No extra plumbing needed in this crate.
- `stitchd-schedule-service` depends on `stitchd-db` with `features = ["tonic"]` so
  `RepositoryError → tonic::Status` (`From` impl) is available in the gRPC service.
- No `cargo sqlx prepare` needed (zero `query!`/`query_as!` macros). NOTE for Phase 9: this crate
  must be added to docker-compose + the CI build/test matrix (it is NOT yet there).

## 2026-06-04 — Phase 5 (scheduler experiments+segments+routes + experiment start-prereqs)
- SHAs: 5.1 experiment apply `c223ba0`, 5.2 start-prereqs `1d7160a`, 5.3 segment apply `fca5f66`,
  5.4 gateway routes `12f6f18`. Beads hp5.5.1–5.5.4 closed; milestone feature-flag-hp5.5 left OPEN.
- **Dispatcher generalized to 3 arms** (`apply/mod.rs`): `Dispatcher<F,E,S>` over flag/experiment/
  segment appliers; unknown entity_type → `Failed`. main.rs now dials experimentation-service
  (`STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL`, default :50055 — NOT :50054 which is analytics) +
  segmentation-service (`STITCHD_SEGMENTATION_SERVICE_GRPC_URL`, default :50053).
- **Experiment apply (5.1)** mirrors flag apply: `ExperimentTransitioner` trait + JSON payload
  `{transition: start|pause|resume|stop|archive, reason?}`. `start`/`resume`→Active, `stop`/
  `archive`→Concluded (the experiment model has only Draft/Running/Paused/Stopped — no separate
  archived state; Concluded IS terminal/archived). **Outcome classification key insight:** an
  invalid transition at fire time surfaces as `FAILED_PRECONDITION` (validate_transition →
  RepositoryError::InvalidState → failed_precondition) OR `ALREADY_EXISTS` (the one-running-per-flag
  uniqueness guard → UniqueViolation → already_exists). BOTH map to `Skipped` (recoverable; recurring
  advances). Unmet start-prereq is also failed_precondition → Skipped. Everything else → Failed.
- **Experiment start-prereqs (5.2)** = migration `20260604000002` + `experiment_start_prerequisites`
  (kind CHECK flag_variant|experiment_done; a `chk_experiment_start_prereq_shape` CHECK keeps each
  kind's columns exclusive: flag_variant sets flag_id+variant_id, experiment_done sets
  prerequisite_experiment_id) + `PgStartPrerequisiteRepository` (runtime queries, in
  experimentation-service NOT stitchd-db — `#[sqlx::test(migrations = "../stitchd-db/migrations")]`
  reaches the shared migration dir). Enforced in `transition_experiment` when `target==Running`
  (covers manual AND scheduled start — both issue the same RPC), BEFORE `apply_transition`, rejecting
  unmet with `FAILED_PRECONDITION` (→409 via gateway GatewayError::Conflict). Wired as an OPTIONAL
  collaborator (`with_start_prerequisites` builder, like `with_dictionary_refresher`) so the ~6 other
  `ExperimentationServiceImpl::new(...)` test sites compile unchanged.
- **Evaluation is behind a `StartPrerequisiteResolver` trait** (2 bool-ish checks) so the gate is
  unit-testable with stubs (no live flag-service). Production `ServiceStartPrerequisiteResolver`:
  `experiment_done` fully resolved via `experiment_repo.find_by_id().status == Stopped`;
  **`flag_variant` FAILS CLOSED** (reports Unmet → refuses start) because flag-service's
  GetFlag/FeatureFlag proto exposes variants by KEY only (no variant UUIDs) and the prereq stores
  `required_variant_id` (UUID per spec) — can't match. Filed a P2 bead (deps
  discovered-from:feature-flag-hp5.5.2) to add variant IDs to the FeatureFlag proto.
- **Segment apply (5.3)** dispatches a definition update via `UpdateAdminSegment` (segment_id +
  condition_expr bytes + version/name/tags/context_type/excluded_keys). condition_expr travels in the
  JSON payload as a string → `.into_bytes()`. stale-version (Aborted/FailedPrecondition)→Skipped,
  NotFound→Failed. **LIMITATION:** list-generation activation is NOT dispatchable —
  segmentation-service exposes only entry-level AddEntries/RemoveEntries/PatchSegmentEntries, no
  "activate prepared generation" RPC; the payload `kind: list_generation` is rejected with that
  explicit reason (recorded as a Failed run).
- **Gateway routes (5.4)** — added `schedule_client` to `GatewayState`. `connect` gained a
  `schedule_addr` param (default :50057; only 1 prod call site, in gateway main). To avoid rippling
  the 8 `from_channels` test call sites, `from_channels` builds the schedule_client from a lazy
  placeholder channel + a `with_schedule_client(c)` builder lets schedule-route tests inject a mock.
  One direct `GatewayState { … }` struct literal (routes/events.rs test) needed the new field added.
- **SHARED SEAM router.rs:** added schedules in TWO clearly-commented `// --- schedules
  (flag_lifecycle) ---` / `// --- end schedules ---` blocks — one in `resource_read` (list+get, before
  its `.with_state`), one in `resource_write` (create+cancel+pause+resume, before its `.with_state` +
  `require_write_permission` layer). Also one line added to the `use crate::routes::{…}` import list
  (inserted `schedules` alphabetically between `saml,` and `sdk_backend,`). Phase-4 worker also edits
  router.rs (prerequisites routes) — these blocks are disjoint from any flag-prereq routes.
- URL shape chosen ENV-scoped (a scheduled change carries env_id; mirrors experiment/segment reads):
  `/v1/environments/{eid}/{flags|segments|experiments}/{entity_id}/schedules` (create/list) +
  `/v1/environments/{eid}/schedules/{sid}[/cancel|/pause|/resume]` (by-id ops). The `{entity_kind}`
  path segment maps flags→Flag etc.; unknown kind → 400.
- Gateway route handlers on the write tree do NOT call `require_permission` — authz is the
  `require_write_permission` middleware layer on `resource_write`. Read handlers likewise rely on
  `auth_middleware`. So schedule handlers just proxy the gRPC call + `GatewayError::from`.
