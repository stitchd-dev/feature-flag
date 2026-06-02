# Spec: Cross-Experiment Interaction — Exclusion Groups + Interaction Analysis

**Track ID:** `xexp_interaction_20260602`
**Type:** Feature

## Overview

Today every experiment binds 1:1 to a flag, and a unique partial index
(`idx_experiments_one_active_per_flag`) forbids two active experiments on the *same*
flag. But nothing constrains a single context (e.g. a user) from being enrolled in
many experiments across *different* flags simultaneously. When two such experiments'
treatments influence the same downstream metric, their effects can interact —
biasing each experiment's measured lift.

This track adds the two halves of overlapping-experiment infrastructure:

1. **Prevention — Mutual-Exclusion Groups ("layers").** An optional named group per
   environment. Experiments assigned to the same group are guaranteed to never share a
   context: the group owns a deterministic bucket space `[0, 10000)` bp, each member
   experiment is allocated a *disjoint* sub-range sized by its traffic allocation, and a
   context is enrolled in a member experiment only if `hash(context_key, group_salt)`
   falls in that experiment's range. Disjoint ranges ⇒ at most one experiment per group
   enrolls any given context. Contexts outside all member ranges are held out (unenrolled).

2. **Analysis — Interaction Detection + Significance.** For experiments that *do* overlap
   (different groups, or ungrouped), detect shared-context populations from the existing
   `experiment_assignments` ClickHouse table and run a pairwise two-way interaction test
   (main effects + interaction term) per shared metric. Surface statistically significant
   interactions as a warning + report on the experiment detail page.

## Evaluation-Path Invariant (load-bearing)

Verified against `crates/stitchd-core/src/evaluation/engine.rs:70`: `evaluate_flag(...)` is a
**pure, non-async function** — no DB pool, no client, no `.await`, no I/O — and it is today
**completely experiment-unaware**. Experiments are not in the flag snapshot; experiment→rule
routing happens *post-evaluation* on the server via `experiment_assignments_mv` (keyed on
`(env_id, flag_id, matched_rule_id, context_type)`). The SDK holds definitions in a lock-free
`ArcSwap<DefinitionSnapshot>` (`sdks/rust/src/snapshot.rs:259`) and the trace is built in-memory
inside `evaluate_one()` (`engine.rs:175`).

**This track MUST preserve that invariant.** The exclusion gate is therefore modeled as static
snapshot data carried **on the rule's percentage allocation** — exactly analogous to how
`hash_inputs`/`weights` already travel — NOT as an experiment object the core looks up.
Consequently:

- Flag evaluation (preview + SDK) performs **zero DB/network lookups** for exclusion gating; the
  gate (`group_salt` + bucket range) lives in the in-memory `Flag` snapshot.
- `evaluate_flag` stays **experiment-unaware and pure**: it reads the rule-resident gate, computes
  `group_bucket(context_key, group_salt)` in-memory, gates enrollment, and emits the
  "held out by exclusion group" trace reason in-memory — the same construction path as `RolloutDebug`.
- The experimentation-service **stamps** the gate onto the rule's distribution when an experiment is
  assigned to a group (and clears it on unassign/stop); it flows into the snapshot through the
  existing PG → proto → server-streaming sync pipeline.

## Functional Requirements

### A. Mutual-Exclusion Groups (Prevention)

- **FR-A1** New PostgreSQL entity `exclusion_groups` scoped per environment: `id`, `env_id`,
  `name` (unique per env), `description`, immutable random `salt`, `version`, audit/soft-delete
  columns. Standard optimistic-concurrency + audit-log + soft-delete semantics (per product.md).
- **FR-A2** `experiments` gains nullable `exclusion_group_id` (FK) plus an allocated bucket range
  (`group_bucket_lo`, `group_bucket_hi`, both basis points in `[0, 10000]`, null when ungrouped).
  Snapshotted into `experiment_iterations` like other config.
- **FR-A3** Assigning an experiment to a group allocates a disjoint sub-range from the group's free
  space, sized by the experiment's `traffic_allocation`. Allocation **rejects** (HTTP 409 / gRPC
  `FAILED_PRECONDITION`) if the group lacks enough free contiguous space. Stopping/deleting a member
  frees its range.
- **FR-A4** Bucket math lives in `stitchd-core` (reusing the existing Murmur3 → 0–9999 hashing) as a
  pure, unit-tested function: `group_bucket(context_key, salt) -> u16`, plus a range-membership check.
- **FR-A5** The gate is carried as a **snapshot-resident attribute of the rule's percentage
  allocation** (`exclusion_gate: Option<{ group_salt, bucket_lo, bucket_hi }>`), analogous to
  `hash_inputs`/`weights`. The SOLE orchestrator `stitchd-core::evaluation::evaluate_flag` reads this
  gate from the in-memory `Flag` and enrolls the context **only if** its group bucket ∈ `[lo, hi)`;
  otherwise the flag evaluates as if the rule did not enroll it (falls through to non-experiment
  behavior). `evaluate_flag` remains experiment-unaware, pure, and non-async — **no DB/network/await,
  no experiment object passed.** Applies identically to the preview path and the Rust SDK path.
- **FR-A6** The gate travels through the **existing** PG → proto → flag-service server-streaming
  definition-sync pipeline (the same path `hash_inputs` uses) and is held in the SDK's in-memory
  `ArcSwap` snapshot — gating is enforced in-process with **no extra round-trip per evaluation**.
- **FR-A7** The whole-flag experiment lock continues to apply unchanged; group membership is itself a
  locked attribute while the experiment is running/paused.

### B. Interaction Detection + Significance (Analysis)

- **FR-B1** A detection query (new builder in `stitchd-stats-service::queries`) self-joins
  `experiment_assignments` on `(env_id, context_type, context_key)` across distinct
  `experiment_id`s with overlapping active windows, producing, per experiment pair + context type,
  the shared-context count and the per-variant-combination (Aᵥ × Bᵥ) cell counts.
- **FR-B2** For each pair sharing a metric (intersection of their `metric_ids`) above a configurable
  minimum shared-sample threshold, compute a **two-way interaction test**: main effects of A and B
  plus the A×B interaction term, with a p-value / confidence interval on the interaction. Binary
  (conversion) metrics and continuous (revenue/duration/numeric) metrics are both supported;
  funnel/ratio metrics are out of scope for v1 interaction (documented).
- **FR-B3** Results persist to a new ClickHouse `experiment_interactions` table keyed on
  `(env_id, experiment_id_a, experiment_id_b, context_type, metric_key)`, holding cell stats, the
  interaction estimate, p-value, and a significance flag.
- **FR-B4** Interaction computation runs inside the existing 60-minute `stitchd-stats-service`
  schedule (and on-demand recompute), reusing the established pure-query-builder → execute → write
  pattern. It only runs for experiments **not** mutually excluded (grouped experiments cannot overlap,
  so the detector skips same-group pairs).
- **FR-B5** New gRPC RPC(s) on the experimentation-service to read interaction results for an
  experiment; exposed through the gateway as REST.

### C. Admin UI

- **FR-C1** Exclusion-group management: list/create/edit groups in an environment; a capacity view
  showing each group's allocated vs. free bucket space and its member experiments.
- **FR-C2** The CreateExperiment form gains an optional "Mutual-exclusion group" picker; selecting one
  shows remaining capacity and validates the experiment's traffic allocation fits.
- **FR-C3** ExperimentDetail gains an **"Interactions" tab**: lists detected overlapping experiments,
  shared-context counts, the per-cell metric breakdown, and the interaction test result. A
  warning banner appears on the Results tab when a significant interaction is detected, advising
  cautious interpretation.

## Non-Functional Requirements

- **NFR-1** Backward compatible: ungrouped experiments behave exactly as today; all new proto fields
  are additive (proto3 `optional` / new messages), no breaking contract changes.
- **NFR-2** Evaluation gating adds **no network round-trip and no DB lookup** and negligible CPU (one
  extra hash + range compare) on the hot path; trace reasoning is built in-memory.
- **NFR-3** Detection queries must be parameterized (no `format!()` SQL) per the established
  ClickHouse injection-safety convention.
- **NFR-4** ≥90% per-crate coverage; `cargo clippy --all-targets -- -D warnings` clean; admin
  vitest green; `cargo xtask docs` idempotent.
- **NFR-5** Lean-gateway boundary preserved: all new domain logic lives in owning services; the
  gateway only does REST↔gRPC translation + cross-cutting concerns.

## Acceptance Criteria

- **AC-1** Two running experiments on different flags assigned to the same exclusion group never
  share a context: an integration test enrolls a population across both flags and asserts the
  `experiment_assignments` intersection on `(context_type, context_key)` is empty.
- **AC-2** A context whose group bucket falls outside a grouped experiment's range evaluates the
  bound flag to its non-experiment outcome — verified identically via evaluate-preview and the Rust
  SDK — with **no DB/network call** made during the evaluation (the gate is read from the snapshot).
- **AC-3** Assigning experiments whose combined traffic exceeds 100% of a group's space is rejected
  with a clear capacity error; stopping a member frees its range for reuse.
- **AC-4** Given a seeded overlapping pair with a planted interaction effect, the interaction test
  flags it significant; given independent effects, it does not (no false positive) — both asserted.
- **AC-5** The ExperimentDetail "Interactions" tab renders detected overlaps, per-cell breakdown, and
  the significance verdict; the Results-tab warning banner appears only when a significant interaction
  exists.
- **AC-6** Full quality gate green: workspace tests, clippy, admin vitest, sqlx offline cache, docs
  idempotency.

## Out of Scope

- N-way (3+) interaction analysis — pairwise only in v1.
- Interaction testing for funnel and ratio metrics — aggregation/conversion/numeric only in v1.
- Bayesian interaction modeling — Frequentist two-way test only in v1.
- Automatic remediation (auto-pausing an experiment on detected interaction) — report only.
- Cross-environment groups — groups are environment-scoped.
- Retroactive re-bucketing of already-enrolled contexts when a group's membership changes
  (first-exposure assignment remains sticky; range changes affect future enrollments only).
