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

## 2026-06-04 — Phase 4 (prerequisites: persistence, service, gateway, snapshot, delete-block)
- SHAs: 4.1 db repo `58ea071`; 4.2+4.3-service `b115625`; 4.3-gateway-decode+4.4-routes `b8fefa2`.
- **`dependency_exists:` sentinel convention** (mirrors `flag_locked_by_experiment:<uuid>`):
  - PRODUCED by flag-service in `crate::prerequisites::DEPENDENCY_EXISTS_STATUS_PREFIX`
    (`"dependency_exists:"`) on a `tonic::Status::failed_precondition`, payload =
    comma-separated dependent flag UUIDs. Helper `dependency_exists_message(&[FlagId])`.
  - DECODED in `stitchd-gateway/src/error.rs` (its own `const DEPENDENCY_EXISTS_STATUS_PREFIX`
    copy — there is NO shared const crate, same as the flag-lock + invalid_distribution
    sentinels) into `GatewayError::DependencyExists { dependents }` → HTTP 409 body
    `{ "error": "dependency_exists", "dependents": [...], "message": ... }`. Decoded BEFORE the
    generic FailedPrecondition→Conflict mapping (order matters, like the flag-lock decode).
  - The delete-block guard (`FlagServiceImpl::ensure_no_dependents`) runs in the MutateFlag
    Delete/Archive branch AFTER the experiment-lock check, BEFORE the version check.
- **Write-time cycle detection reuses the eval-time orchestrator.** `crate::prerequisites::
  detect_prerequisite_cycle(&[(FlagId, Vec<FlagId>)])` builds a `HashMap<FlagId,HashSet<FlagId>>`
  and runs core `rule_engine::dependency::topological_sort` (the same Kahn pass the gate uses) —
  `CyclicFlagDependency { involved }` → reject with `Status::invalid_argument` carrying the cycle
  path joined by ` -> ` (HTTP 400 at the gateway, NOT 409/422). The graph fed in is existing edges
  (from `PrerequisiteRepository::edges_for_flags(project_flag_uuids)`, minus the edited flag's row
  which is fully replaced) + the proposed edges. Self-prereq is rejected up front.
- **Preview now resolves real prerequisites** (the key wiring): `evaluate_preview` RPC loads the
  persisted gate via `load_prerequisite_gate` into `Flag.prerequisites`, then PER evaluation
  context BFS-walks the transitive prerequisite-flag closure (`resolve_prerequisite_variant_map`),
  loads each closure flag's rules + its own prereqs, resolves their segments, and runs core
  `orchestrator::evaluate_flags_with_prerequisites` to get the `evaluated_flags` map. That map is
  threaded into a NEW core `evaluation::preview::evaluate_preview_with_prerequisites` (old
  `evaluate_preview` delegates with an empty map = fail-closed). Result: preview returns the
  configured fallback variant when a prerequisite is unmet. Added
  `ContextPreviewResult.prerequisite_failure: Option<PrerequisiteFailureTrace>` (serde-default,
  skip-if-none) so the preview trace NAMES the failing prerequisite (spec B3) — populated from the
  engine's `EvaluationTrace.prerequisite_failure`.
- **Snapshot population in 3 places**: `get_flag` + `list_flags` (admin, `populate_prerequisites_proto`)
  and the `get_flag_definitions` SDK-sync STREAM (uses a free fn `build_prerequisite_protos_with`
  because the spawned task moves cloned `Arc` repos — can't borrow `&self`). All carry BOTH UUIDs
  (admin) + resolved keys (SDK local resolution) per the FlagPrerequisite proto's dual-id/key shape.
- **`PrerequisiteRepository` is a concrete `PgPool` struct, NOT a trait.** Wired into
  `FlagServiceImpl` as `Option<PrerequisiteRepository>` via `with_prerequisite_repo` (None ⇒ RPCs
  return UNIMPLEMENTED, delete-block is a no-op — for targeted unit tests). flag-service tests are
  otherwise mock-based; the prereq RPCs need a real pool, so they live in a NEW
  `tests/prerequisites_integration.rs` using `#[sqlx::test(migrations = "../stitchd-db/migrations")]`
  with `PgFlagRepository`/`PgVariantRepository`/`PgSegmentRepository` (PgSegmentRepository impls
  SegmentRepository directly — no Scylla composite needed since prereq RPCs never touch segments).
  Added `sqlx`+`uuid`+`async-trait`+`chrono` to flag-service `[dev-dependencies]`.
- **`feature_flags` has NO `environment_id` column** (project-scoped); `projects`/`environments`
  have no `key` column. Test fixtures insert org→project→flag→variant directly.
- **router.rs SHARED SEAM** — added EXACTLY these lines (after the `/hashing` route, before
  `// Segments — write`), so Phase 5 can anticipate the merge:
  ```
          // --- prerequisites (flag_lifecycle) ---
          .route(
              "/v1/projects/{project_id}/flags/{flag_id}/prerequisites",
              get(flags::get_prerequisites).put(flags::set_prerequisites),
          )
          // --- end prerequisites (flag_lifecycle) ---
  ```
- No `cargo sqlx prepare` needed (zero compile-time macros; all runtime `sqlx::query`/`query_as`).

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

## 2026-06-05 — Phase 6 (segment/experiment delete-block + dependency read API)
- SHAs: 6.1 delete-block `dea4283`, 6.2 dependency read API `d751524`. Beads hp5.6.1 closed,
  hp5.6.2 in_progress→closed; milestone feature-flag-hp5.6 left OPEN.
- **The `dependency_exists:` sentinel is now produced by THREE services**, each with its own
  PRIVATE `pub const DEPENDENCY_EXISTS_STATUS_PREFIX = "dependency_exists:"` copy (no shared const
  crate, same as `flag_locked_by_experiment:`): flag-service (`prerequisites.rs`, Phase 4),
  segmentation-service (`dependency_scan.rs`), experimentation-service (`start_prerequisites.rs`).
  The gateway `error.rs` decode is source-agnostic — it strips the prefix off any
  FailedPrecondition status → `GatewayError::DependencyExists { dependents }` → HTTP 409. So NO
  gateway change was needed for the two new producers; the Phase-4 decode already covers them.
- **Segment dependents computed authoritatively at delete time** (`dependency_scan.rs`), NOT from
  `entity_dependencies` (only flag→flag is populated there). flag→segment: candidate
  `feature_flag_rules` rows via `WHERE rule_def::text LIKE '%<seg-uuid>%'`, then deserialize each
  `rule_def` as a core `ConditionExpr` and confirm with `ConditionExpr::collect_segment_ids`
  (already existed in core/types.rs) — DISTINCT `flag_id`. segment→segment: same text-prefilter +
  collect over OTHER live `segments.condition_expr` (`deleted_at IS NULL AND id <> $1`). The
  text-LIKE prefilter is just a cheap candidate narrower — the Rust walk is the source of truth
  (a UUID could appear in an unrelated JSON position). **`Condition::InSegment`/`NotInSegment`
  serialize externally-tagged as `{"InSegment":"<uuid>"}`** inside the `{"Leaf":{…}}` ConditionExpr
  node. NOTE: the segment write path FORBIDS InSegment/NotInSegment inside a segment's own
  condition_expr (`SEGMENT_FORBIDDEN_OPS` in grpc/service.rs), so segment→segment is empty for
  service-written data — we still scan it so a ref inserted by any other path blocks the delete.
- **The segmentation "service.rs" is actually `crates/stitchd-segmentation-service/src/grpc/service.rs`**
  (the prompt's path was nominal). It holds `AppState { segment_repo }` only — added an OPTIONAL
  `dependency_pool: Option<sqlx::PgPool>` (flag rules + segments share ONE Postgres DB, so the
  service's own pool can scan flag_rules) + `AppState::new(repo)`/`with_dependency_pool(pool)`
  builders so the ~4 mock-repo unit-test sites switch to `AppState::new(...)` (pool None ⇒ guard
  is a no-op, existing delete tests unchanged). main.rs passes `Some(pool.clone())`. Guard wired
  into BOTH delete paths: `MutateSegment(Delete)` (resolve seg by key first) AND
  `DeleteAdminSegment` (has the id directly). There is NO segment "archive" — only soft-delete.
- **Experiment delete-block** = new `StartPrerequisiteRepository::dependents_experiment_done(exp)`
  (`SELECT DISTINCT experiment_id FROM experiment_start_prerequisites WHERE kind='experiment_done'
  AND prerequisite_experiment_id=$1`), enforced in `delete_experiment` AFTER find_by_id, BEFORE
  soft_delete, via the EXISTING optional `start_prereq` collaborator (no new field). The
  `StubPrereqRepo` tuple-struct in service.rs tests became `{prereqs, dependents}` with
  `::new(prereqs)` / `::with_dependents(ids)` ctors. "Archive" for experiments = transition to
  Concluded (normal lifecycle) — NOT blocked; only delete is guarded (blocking a stop would be wrong).
- **Dependency read API is gateway-only, NO proto/RPC additions** (those are Phase-1-owned + would
  ripple the parallel wave). Computed entirely from EXISTING RPCs:
  - flag: `GetFlag` returns BOTH `prerequisites` (upstream flags) AND `rules` (whose
    `rule_payload` = serialized ConditionExpr → segment_ref upstream); downstream = `ListFlags`
    (project-scoped, also populates prerequisites) filtered to flags whose `prerequisites` name
    the subject by key OR id.
  - segment: downstream = `ListFlags` rule-scan for the segment id; segment→segment nesting needs
    env-scoped `ListSegments` (the URL is project-scoped) AND is forbidden anyway → reported via a
    `note` field, empty set.
  - experiment: start-prereq edges have NO read RPC → empty graph + `note`. (Integrity is still
    enforced at delete time by 6.1.) Filed as a known gap — a future `GetStartPrerequisites` RPC
    would let the gateway populate the experiment branch.
  Response shape: `DependencyGraphJson { entity_kind, entity_id, upstream: [DependencyEdge],
  downstream: [DependencyEdge], note? }`; `DependencyEdge { entity_kind, id, key, kind }` where
  `kind ∈ {prerequisite_flag, segment_ref, dependent_flag}`.
- **router.rs lines added (resource_read, after the `// --- end schedules ---` block, before
  `.with_state`):**
  ```
          // --- dependency graph (flag_lifecycle Phase 6) ---
          .route(
              "/v1/projects/{project_id}/{entity_kind}/{entity_id}/dependencies",
              get(dependencies::get_dependencies),
          )
          // --- end dependency graph ---
  ```
  Plus `dependencies` added to the `use crate::routes::{…}` import list (alphabetical, between
  `context_intel,` and `eval_stats,`) and `pub mod dependencies;` in routes/mod.rs; openapi.rs
  registers `get_dependencies` in `paths(...)` and `DependencyGraphJson`+`DependencyEdge` in
  `components(schemas(...))`. The route is env-agnostic READ-only so it lives in `resource_read`
  (JWT auth via the tree's `auth_middleware`, no `require_write_permission`).
- No `cargo sqlx prepare` needed anywhere in Phase 6 (all runtime `sqlx::query`, zero macros).

## 2026-06-05 — Phase 7 (SDK prerequisite support, Rust) — crate `stitchd-sdk-rust`
- SHAs: 7.1 Red `af91d9d` (`sdks/rust/tests/prerequisites.rs`), 7.2 Green `c320a18`
  (`sdks/rust/src/client.rs`). Beads hp5.7.1/hp5.7.2 closed; milestone feature-flag-hp5.7 OPEN.
- **The key↔id problem & its fix.** SDK definition-sync carries prerequisites by KEY only
  (`prerequisite_flag_key` + `required_variant_key`; the `*_id` fields are empty on the wire), but
  core `PrerequisiteGate` is keyed by `FlagId`/`VariantId` UUIDs, and the old SDK conversion minted
  `VariantId::new()` per variant + parsed `proto.flag_id` for the flag. A dependent flag's
  `required_variant_id` would NEVER match the prerequisite flag's freshly-minted variant id. FIX:
  derive IDs DETERMINISTICALLY from keys — `deterministic_uuid(tag, part_a, part_b)` (length-prefixed
  FNV-1a fold into a v4-shaped Uuid; the `uuid` crate's `v5` feature is NOT enabled so we don't use
  real UUIDv5), wrapped by `deterministic_variant_id(flag_key, variant_key)` and
  `resolve_flag_id(wire_uuid, flag_key)` (prefers a populated wire UUID, else deterministic-from-key).
  Now `(flag_key, variant_key)` → the same `VariantId` across separate conversions of DIFFERENT flags,
  so the gate matches. `build_prerequisite_gate(proto, own_variant_map)` resolves each prereq's
  required-variant against the PREREQUISITE flag's key (not the dependent's) and the fallback against
  the dependent flag's OWN variant map.
- **Closure flags must use deterministic-from-key FlagId.** In `resolve_prerequisite_map` the closure
  flags are keyed by `resolve_flag_id("", key)` (forced deterministic), NOT `core.record.id` (which
  would use the wire UUID) — because the dependent's gate references prereqs deterministic-from-key
  (their wire ids are empty in the gate). Both sides must agree for the map lookup to hit. The eval
  TARGET flag keeps its own wire-UUID FlagId (irrelevant to gating; it references prereqs, not self).
- **CRITICAL transitive gap — the orchestrator does NOT apply the gate.**
  `orchestrator::evaluate_flags_with_prerequisites` resolves each flag's variant from its RULES ONLY
  (its own rustdoc says so) — it does NOT fold a flag's prerequisite fallback into the returned map.
  So in A→B→C with C unmet, B still appears as its rule output (`on`), and A would wrongly proceed.
  The core engine's gate test (`engine.rs::prereq_transitive_chain_falls_back_when_root_off`) proves
  the CALLER must pre-fold each prerequisite's gate-applied variant into the map. SOLUTION (in SDK
  scope): `fold_prerequisite_fallbacks` re-walks the closure in topological order
  (`build_dependency_graph` + `dependency::topological_sort`, both public) and overrides any flag whose
  gate is unmet-given-the-folded-map with its fallback variant — so fallback propagates transitively.
- **The flag-service PREVIEW path has the SAME gap** (it feeds the orchestrator's rules-only map
  straight into `evaluate_preview_with_prerequisites` with no fold — service.rs:1804). Filed as a
  cross-scope bead (deps discovered-from:feature-flag-hp5.7.2): preview/orchestrator need the same
  transitive fold, or `evaluate_flags_with_prerequisites` should fold gates itself, so SDK + preview
  truly match on transitive-unmet chains (spec D2 "identically to preview").
- **Per-context resolution.** Like preview, prerequisites resolve PER context (a prereq flag may
  resolve differently per subject), so the SDK evaluates each context with its own pre-resolved
  `evaluated_flags` map via `evaluate_flag_with_prerequisites` (the no-prereq path keeps the cheap
  batch `evaluate_flag` over the whole bundle). Cycles/self-references degrade to fail-closed fallback:
  the closure BFS excludes the target flag, breaking direct cycles; the fold then makes the dependent
  unmet → fallback. Missing/disabled/archived prereq flag ⇒ absent from the closure/map ⇒ unmet.
- **No snapshot.rs change needed** — the snapshot already stores the proto `FeatureFlag` verbatim
  (prerequisites + fallback_variant_key ride it from Phase 4's `get_flag_definitions`); all wiring is
  in `client.rs`. The SDK's `EvalOutcome::PrerequisiteFailed` (Phase 2) already maps from
  `CoreEvalOutcome::PrerequisiteFailed`.
- Coverage: integration tests (9 cases a–h: unmet/met/transitive×2/disabled/missing/empty-fallback/
  cycle/self) + 7 helper unit tests (deterministic_uuid stability+collision, build_prerequisite_gate
  key↔wire-id branches, fold dependency-order propagation, resolve_segments_for_bundle). New paths
  >90%; remaining gaps are defensive branches (conversion-fail continue, orchestrator-cycle warn —
  hard to reach since closures exclude the target).

## Bugfix: transitive prerequisite fold unified in core (bead feature-flag-df7)

- **Bug**: `rule_engine::orchestrator::evaluate_flags_with_prerequisites` builds the cross-flag
  resolved map from RULES ONLY — it never applies each flag's own prerequisite gate. In a transitive
  chain A→B→C with C unmet, B was recorded as its *rule* variant, so A's gate saw B as satisfying its
  requirement and wrongly proceeded. The Phase 7 SDK worked around this locally; the flag-service
  PREVIEW path had the identical, unfixed bug.
- **Fix (DRY — option b, core helper)**: promoted the SDK's private `fold_prerequisite_fallbacks`
  into `stitchd-core` as `pub fn rule_engine::orchestrator::fold_prerequisite_fallbacks(map,
  flag_entries, gates)` (re-exported from `rule_engine::mod`). It rebuilds the dep graph, walks the
  closure in topological order, and overrides any flag whose gate is unmet (given the already-folded
  map) with `gate.fallback_variant_id` — so fallback propagates transitively. Chose the helper over
  changing the orchestrator's input tuple to keep `evaluate_flags`'s empty-prereq delegation byte-for-
  byte unchanged (no gates ⇒ no override ⇒ identical map).
- **Wiring**: flag-service `resolve_prerequisite_variant_map` now retains each closure flag's full
  `PrerequisiteGate` (it already loads them via `load_prerequisite_gate`) and folds the rules-only map
  before returning it to `evaluate_preview_with_prerequisites`. The Rust SDK now imports and calls the
  core helper; its private duplicate was removed (the SDK lib test + 9 integration prereq cases still
  pass, behaviour identical since the logic was lifted verbatim).
- **Regression tests**: (1) core unit `fold_records_fallback_for_transitive_unmet_chain` — A→B→C, C
  serves a non-required variant ⇒ B folds to its fallback ⇒ A folds to its fallback. (2) flag-service
  integration `preview_transitive_unmet_chain_falls_back` — seeds rules directly into
  `feature_flag_rules` (always-match `And([])` → `Variant`), verified RED without the fold (A returned
  `a_main`) then GREEN with it (`a_fb`, trace names B as the failing prerequisite).
- **Gotcha**: the existing `preview_returns_fallback_when_prerequisite_unmet` test used a *disabled*
  prereq, which is absent from the closure map regardless — it never exercised the transitive-fold
  bug. The new test keeps every flag *enabled* and uses a rule that resolves the intermediate to a
  non-required variant, which is the only shape that reproduces it.

## 2026-06-05 — Phase 8 (Admin UI full parity)
- SHAs: 8.1 api/types/Yup `102c455`; 8.2 schedule builder `c4231b8` + experiment/segment mount `6bee41a`;
  8.3 prerequisites editor `47e5aa9`; 8.4 dependency graph + delete-block UX + badges + preview `fd1ae2a`.
- **Badges need a backend field, not a new query:** the proto `FeatureFlag` already carries
  `prerequisites` + `fallback_variant_key` (Phase 1/4), and gateway `flag_to_admin_json` already had
  them in scope — surfacing them on `AdminFlagJson` (list/get DTO) was a ~6-line mapping, no extra
  round-trip. This lets the UI render "has prerequisites"/"is a prerequisite" badges and resolve
  reverse-dep cycles client-side without the dependencies endpoint.
- Delete-blocked UX keys on the gateway 409 `{error:"dependency_exists", dependents:[...]}` body
  (surfaced in `DeleteSegmentModal` etc.); cycle warning on the prereq editor keys on the 400 cycle
  path. Dependency graph component under `admin/src/components/dependency/` (recharts present, ^3.8).
- Verified: tsc clean, lint 0 errors (70 pre-existing react-hooks warnings unrelated), vitest
  **924/924** across 58 files, `npm run build` ✓ (chunk-size warning pre-existing/informational).
- Resume note: the Phase 8 worker was interrupted right after implementing 8.4 (uncommitted) and
  before bookkeeping; orchestrator verified the frontend+gateway gate and committed 8.4 as fd1ae2a.

## 2026-06-05 — Phase 9 (docs, CI, final integration)
- SHAs: 9.1 docs+compose `5ee323d`; 9.2 generated-docs `e855658`, coverage-tests `d0ae7dc`,
  fmt `1bf240c`. Beads hp5.9.1/hp5.9.2 closed; milestone feature-flag-hp5.9 left OPEN for orchestrator.
- **CI needed ZERO edits.** Every Rust CI job (`fmt`, `sqlx-check`, `clippy`, `coverage`) is
  workspace-wide (`--workspace`), so `stitchd-schedule-service` was already covered the moment it
  joined `[workspace.members]`. There is NO per-crate build/test matrix to extend. The coverage job
  excludes `main.rs` via `--ignore-filename-regex 'main\.rs'` (so the binary entrypoint isn't a
  coverage liability) and excludes `stitchd-proto`/`xtask`. The only per-crate `--test` list in
  ci.yml is the stats-service live-ClickHouse step — this track adds no self-seeding live-CH tests,
  so that list is correctly untouched. E2E (`tests/e2e/*.yaml`) just runs `docker compose up -d
  --wait` then exercises REST, so the new compose service is picked up automatically.
- **docker-compose flag-service port gotcha:** the prompt said
  `STITCHD_FLAG_SERVICE_GRPC_URL=http://flag-service:50051`, but in compose flag-service actually
  listens on **50052** (50051 is auth-service). Used `http://flag-service:50052` (+ exp :50055 /
  seg :50053) so the wired URL matches the real container port. (The crate's config default of
  :50051 is a localhost dev default, irrelevant in compose where the env var is set explicitly.)
- **cargo-rdme does NOT create a README — it fills between markers in an EXISTING file.** Adding
  the crate to xtask's `CRATE_README_TARGETS` alone fails with "crate's README file not found";
  you must first hand-create `README.md` with `# <crate>\n\n<!-- cargo-rdme start -->\n\n<!--
  cargo-rdme end -->\n`, THEN the generator fills the body from `lib.rs`'s `//!` preamble. The
  README is generated from the **lib target** (`src/lib.rs`), not `main.rs` — so the lib.rs
  preamble is the source of truth for the published README. Polished it to drop phase-internal
  language ("Phase 3/5") and name the per-entity dispatch RPCs + the `apply::Applier` seam.
- **Docs idempotency: tracked vs ephemeral.** `cargo xtask docs` then `git diff --exit-code` is the
  CI gate, but generator-owned files under `docs/src/grpc/*` (except README.md) + `docs/src/api/
  openapi.json` are **gitignored** (`git check-ignore` confirms). So the new
  `schedule_v1_schedule_service.md` page + the flag-prereq RPC additions don't show in `git status`
  — verify them by grepping the generated page directly; the TRACKED idempotency surface is
  crate READMEs + `docs/src/deployment/env-vars.md` + `docs/src/SUMMARY.md` + `docs/src/grpc/
  README.md` + quickstart. Confirmed a second run produces byte-identical tracked output.
- **env-vars page auto-discovers `STITCHD_*` by source-scraping** `env::var("STITCHD_…")` /
  `env_or(...)` across `crates/` + `sdks/` — no manual edit. The schedule-service's `config.rs`
  uses `std::env::var`, so all `STITCHD_SCHEDULE_*` (+ the gateway's `STITCHD_SCHEDULE_SERVICE_ADDR`)
  appeared automatically grouped under their crate.
- **sqlx: zero drift** — the whole track used runtime `sqlx::query`/`query_as` (no `query!`
  macros), so `cargo sqlx prepare --workspace -- --all-targets --features
  stitchd-sdk-rust/test-util` produced no `.sqlx/` change and `--check` passes. (`#[sqlx::test]`
  in the new `tests/grpc_service.rs` is a macro, not a cached compile-time query.)
- **schedule-service coverage gap was the gRPC + dispatch + mapping layer (82%→92.59%).** The
  scheduler core, apply paths, and config had tests, but `grpc.rs` (0%), `apply/mod.rs` Dispatcher
  (0%), and `mapping.rs::change_to_proto`/`run_to_proto` (61%) had none. Fixed with: a
  `tests/grpc_service.rs` `#[sqlx::test]` suite over the full ScheduleService surface (the service
  is a thin wrapper over `ScheduledChangeRepository`, which needs a real PgPool — mocking it would
  test nothing), Dispatcher unit tests (incl. the unknown-entity-type → `Failed` not-dropped
  branch), and mapping unit tests (row→proto + exhaustive enum arms). GOTCHA: recurring RRULEs need
  a `DTSTART` (rrule 0.14 errors "Missing start date" on a bare `RRULE:FREQ=DAILY`); and
  pause/resume require an **active** row — only RECURRING changes are created `active` (one-shot →
  `pending`, which only `cancel` accepts), so pause/resume must be exercised on a recurring change.
- **Full CI-mirror gate (all green):** clippy `--workspace --all-targets --features
  stitchd-sdk-rust/test-util -D warnings` ✓; `cargo fmt --all --check` ✓ (rustfmt rewrapped the new
  test's long chained `.await.unwrap_err()` lines — committed); `cargo test --workspace` ✓ 2625
  passed; `check_openapi_contract.py` ✓ (23 baseline routes all covered by 116 gateway routes —
  the new prereq/schedule/dependency routes are additive); admin tsc ✓, lint ✓ (0 errors / 70
  pre-existing react-hooks warnings), vitest ✓ 924/924, build ✓ (pre-existing chunk-size warning).
- **Known environmental failure (NOT a regression):** `stitchd-flag-service`'s
  `tests/eval_preview_clickhouse::evaluate_preview_writes_rows_to_clickhouse` connects to a live
  flag-service daemon on `STITCHD_FLAG_SERVICE_ADDR` (default :50052). It self-skips when the var is
  UNSET, but `.env.local` sets it → it attempts a connect and fails `ConnectionRefused` with no
  daemon running. Not in CI's auto path (CI starts only postgres+clickhouse; this is an E2E
  concern). Unrelated to this track. With the var unset the entire workspace suite is green.

## 2026-06-05 — REVISION #1
- **Type:** Both (plan-heavy: new Phase 10; light spec)
- **Trigger:** Three deferred/partial behaviours from Phases 1–9 (filed as follow-up beads) were
  folded into the plan for completion.
- **Learning:**
  - Gotcha: a feature can pass its phase gate while a sub-behaviour is silently **fail-closed** —
    `flag_variant` experiment start-prereqs "worked" (refused start) only because the verifying
    data (variant UUID) wasn't on the proto, so they could never be satisfied. A green test suite
    didn't catch it because no test asserted the *positive* (prereq-met ⇒ start allowed) path.
  - Pattern: when a worker reports "X fails closed because Y isn't available," treat it as an
    in-scope gap to schedule, not just a note — and ensure both the negative AND positive paths of
    a gate are tested. Cross-service data needs (e.g. an ID the consumer must compare) belong on the
    producer's proto from the start.
