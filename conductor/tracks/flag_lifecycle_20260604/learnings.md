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
