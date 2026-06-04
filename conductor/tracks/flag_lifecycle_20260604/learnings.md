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
