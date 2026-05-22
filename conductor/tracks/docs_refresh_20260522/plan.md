# Implementation Plan: Docs Refresh & Autogeneration Pipeline

**Track:** docs_refresh_20260522
**Spec:** [./spec.md](./spec.md)
**Source-of-truth:** Phase verification gate = `cargo xtask docs` runs idempotent (no diff on
re-run) AND `mdbook build docs/` produces zero warnings AND every code reference in every
rewritten doc resolves to a live symbol (grep audit).

## Phase 1: Discovery, Orphan Cleanup, Baseline
<!-- execution: sequential -->

- [x] Task 1.1: Audit doc inventory — produce table of every `.md` under `docs/src/`,
      every `crates/*/README.md`, every reference doc (README.md, CONTRIBUTING if present),
      tagged `auto-generated | narrative | orphan | shared-readme`.
- [ ] Task 1.2: Delete orphaned `docs/src/internal/*` subtree
      (auth/, events-experimentation/, flag-segmentation/ — confirmed NOT in `SUMMARY.md`).
      Verify with `grep -r "docs/src/internal" docs/` → no references remain.
- [ ] Task 1.3: Decide fate of `docs/openapi-pre-decomposition.json` and
      `docs/src/api/rest.md` (latter overlaps `gateway/openapi.md`). Archive or delete.
- [ ] Task 1.4: Capture golden baseline — run `cargo xtask docs`, snapshot
      `docs/src/grpc/`, `docs/src/api/openapi.json`, `docs/src/sdk/quickstart.md` to a
      tmp dir; commit the snapshot path note so later phases can confirm regression-free.
- [ ] Task 1.5: Conductor - User Manual Verification 'Discovery, Orphan Cleanup, Baseline'
      (Protocol in workflow.md)

## Phase 2: Extend xtask Generators
<!-- execution: sequential -->

- [ ] Task 2.1: Add `env-vars` generator to `crates/xtask/src/main.rs` — scrape every
      `std::env::var("STITCHD_*")` invocation across `crates/*/src/` + `sdks/*/src/`,
      group by service, emit `docs/src/deployment/env-vars.md` with var name + crate + default
      (if `unwrap_or` / `unwrap_or_else` immediately follows). Idempotent.
- [ ] Task 2.2: Add `cargo-rdme` integration — standardize the `//!` preamble across all
      11 workspace crates + `sdks/rust` (one-paragraph crate purpose, link to mdBook section).
      Add `crate-readmes` step to xtask that runs `cargo rdme --workspace --no-fail-on-warnings`.
      Verify each `crates/*/README.md` regenerates cleanly with zero diff on second run.
- [ ] Task 2.3: Add `mdbook-linkcheck` (or equivalent — could be a custom in-xtask
      walker) integration. Wire into xtask's `mdbook_build` step so broken internal links
      fail the build.
- [ ] Task 2.4: Update `cargo xtask docs` orchestration — add the three new steps in order
      (env-vars → crate-rdme → link-check after mdbook build). Update the usage banner.
- [ ] Task 2.5: Add xtask self-test — `cargo xtask docs` runs twice; second run must produce
      `git status --porcelain` = empty. Document this assertion in the xtask README and
      workflow.md "Daily Development" section.
- [ ] Task 2.6: Conductor - User Manual Verification 'Extend xtask Generators'
      (Protocol in workflow.md)

## Phase 3: Narrative-Prose Rewrites (Parallel by Topic)
<!-- execution: parallel -->

- [ ] Task 3.1: Topic A — Introduction + Architecture
      <!-- files: docs/src/introduction.md, docs/src/architecture/README.md, docs/src/architecture/multi-tenancy.md, docs/src/architecture/evaluation-flow.md, docs/src/architecture/data-stores.md, docs/src/architecture/events.md, docs/src/architecture/metrics.md, docs/src/architecture/service-flows.md -->
      Rewrite each against current code (microservice decomposition, ScyllaDB segments,
      events+metrics composable primitives, experimentation default-rule attribution).
      Acceptance: grep every code reference (struct/handler/proto/CLI flag) and confirm it
      resolves; mermaid diagrams reflect actual service topology.
- [ ] Task 3.2: Topic B — Gateway pages
      <!-- files: docs/src/gateway/overview.md, docs/src/gateway/sdk-api.md, docs/src/gateway/admin-api.md, docs/src/gateway/grpc.md, docs/src/gateway/openapi.md -->
      Rewrite to reflect current axum routes + JWT/SDK-key auth model + lean architecture
      (analytics-service split, event-service retirement). `openapi.md` stays a stub that
      points at the generator output.
- [ ] Task 3.3: Topic C — Deployment
      <!-- files: docs/src/deployment/README.md, docs/src/deployment/postgres.md, docs/src/deployment/clickhouse.md, docs/src/deployment/scylladb.md, docs/src/deployment/sdk-keys.md -->
      Rewrite each against current `docker-compose.yml`, `crates/stitchd-db/migrations/`,
      current ClickHouse schema (`experiment_assignments`, `flag_evaluation_log_v2` with new
      columns), and current ScyllaDB CQL migrations. NOTE: `env-vars.md` is now generated
      (Task 2.1) — do NOT touch it here.
- [ ] Task 3.4: Topic D — SDK
      <!-- files: docs/src/sdk/README.md -->
      Rewrite SDK landing page against `sdks/rust/src/lib.rs` current API. NOTE:
      `quickstart.md` is already auto-extracted from `sdks/rust/src/lib.rs` `//!` docs
      — confirm the Quickstart section in `lib.rs` is up-to-date instead.
- [ ] Task 3.5: Topic E — Experimentation spot-check
      <!-- files: docs/src/experimentation/index.md, docs/src/experimentation/attribution.md, docs/src/experimentation/default-rule-experiments.md -->
      Already written May 22 by Phase 11 of the experimentation_full track. Spot-check
      against current code; only edit if drift found.
- [ ] Task 3.6: Conductor - User Manual Verification 'Narrative-Prose Rewrites'
      (Protocol in workflow.md)

## Phase 4: CI Integration + Final Verification
<!-- execution: sequential -->

- [ ] Task 4.1: Add CI job (`.github/workflows/<existing>.yml` or new `docs.yml`) that
      runs `cargo xtask docs` then `git diff --exit-code` — fails if any generator output
      drifted. Use the same Rust toolchain matrix as the rest of CI.
- [ ] Task 4.2: Add link-check to the CI docs job (separate from cargo xtask docs in case
      the broken-link tool is heavier).
- [ ] Task 4.3: Update `conductor/workflow.md` "Daily Development" + "Before Committing"
      sections to mention `cargo xtask docs` produces no diff as a pre-commit check.
- [ ] Task 4.4: Update `conductor/patterns.md` with a new "Docs Autogeneration" section
      capturing the env-vars + cargo-rdme + idempotency patterns.
- [ ] Task 4.5: Update root `README.md` if any badges / quickstart commands change as a
      result of the rewrites (e.g. point new contributors at `cargo xtask docs`).
- [ ] Task 4.6: Conductor - User Manual Verification 'CI Integration + Final Verification'
      (Protocol in workflow.md)
