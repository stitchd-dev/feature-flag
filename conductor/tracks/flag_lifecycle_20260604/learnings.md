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
