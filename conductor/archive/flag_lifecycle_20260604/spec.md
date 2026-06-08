# Spec: Flag Lifecycle Automation — Scheduled Changes, Prerequisites & Dependency Integrity

<!-- Last Revised: 2026-06-05 (Revision #1 — see Revision History at end + Phase 10 in plan.md) -->

## Overview

Lifecycle-automation capabilities spanning the platform's mutable entities
(flags, segments, experiments), rounding out the flag-operations side:

1. **Scheduled changes** (entity-aware) — apply a mutation to a flag, segment, or
   experiment at a future time, one-shot or as a recurring window (timezone- and
   DST-aware).
2. **Flag prerequisites** — a flag-level dependency gate in the sole
   `evaluate_flag` orchestrator that returns an author-configured **fallback
   variant** when a prerequisite flag does not resolve to a required variant.
3. **Cross-entity dependency graph + referential integrity** — formalized
   dependency tracking with cycle detection, and **deletion/archival blocked while
   an entity is still referenced** as a prerequisite/dependency.
4. **First-class SDK support** for prerequisites (Rust SDK), with dedicated tests.

All surface with full Admin UI parity.

## Functional Requirements

### A. Scheduled Changes  ⟨generalized to flags + segments + experiments⟩
- **A1.** A scheduled change targets `(entity_type, entity_id, env_id)` + a
  mutation payload + a schedule spec. The scheduler is **entity-agnostic**;
  application dispatches to the owning service's existing mutation/lifecycle RPC.
- **A2. Flags:** any `MutateFlag` mutation — enable/disable (`enabled_override`),
  replace variants, change rollout %, replace rules, default-rule distribution.
- **A3. Experiments:** lifecycle transitions — **start, pause, resume, stop,
  archive** (e.g. start an experiment Monday 09:00, auto-stop in two weeks).
- **A4. Segments:** scheduled definition update (rule-expression swap; for
  list-based segments, activate a prepared generation).
- **A5. One-shot** schedule: apply once at a specific instant; author picks an
  IANA timezone, stored canonically in UTC.
- **A6. Recurring window** schedule: repeating paired transitions (e.g. enable
  Mon–Fri 09:00, disable 17:00) on a weekly/cron-like spec with an IANA timezone,
  evaluated **DST-aware**.
- **A7.** A background scheduler (tokio interval loop, mirroring
  `stitchd-stats-service`) applies due changes — **idempotent** and
  **restart-safe** (all state in PostgreSQL; a missed tick catches up).
- **A8.** Application flows through each entity's canonical mutation path: bumps
  the entity **version** (optimistic concurrency) and writes an **audit-log**
  entry attributed to a system/scheduler actor.
- **A9. Lock/guard aware:** if a flag is `FLAG_LOCKED_BY_EXPERIMENT` (or an
  experiment transition is invalid) at fire time, the apply is skipped, the run
  recorded `failed`/`deferred` with reason, surfaced in UI; recurring schedules
  continue to the next window.
- **A10.** Lifecycle: one-shot = `pending → applied | failed | cancelled`;
  recurring = `active ⇄ paused` with per-fire run history. Cancel pending; pause/
  resume recurring.
- **A11.** List/inspect scheduled changes per entity and per environment: next-run,
  last-run status, human-readable diff of the pending mutation.

### B. Flag Prerequisites — eval-time fallback gate
- **B1.** Define prerequisites on a flag: a set of `(prerequisite_flag,
  required_variant)` deps + a per-flag **fallback variant** (default = the flag's
  off/disabled variant).
- **B2.** `evaluate_flag` checks prerequisites **before** rule iteration: if any
  prerequisite flag does not resolve to its required variant (incl. when disabled),
  the dependent flag returns the configured fallback variant and skips its rules.
- **B3.** Resolution is **transitive**, shares the context bundle, and reuses the
  cross-flag resolution `FlagEvaluatedAs` already relies on. The decision is
  recorded in the `EvaluationTrace` (failing prerequisite + fallback taken).
- **B4.** Prerequisites + fallback ride the **flag-definition snapshot**
  (definition-sync proto) so the SDK and preview gate **identically** through
  shared `evaluate_flag`.

### C. Cross-Entity Dependency Graph & Referential Integrity  ⟨was out-of-scope⟩
- **C1.** Dependency edges are tracked for: flag→flag (prerequisites),
  flag→segment (rule/segment refs), segment→segment (`InSegment` nesting),
  experiment→flag (binding) and experiment→experiment (where applicable).
- **C2. Cycle detection at write time:** any edge creation/update that would form
  a cycle within an entity type's prerequisite graph is rejected (`409`/`422`)
  with the offending path; prerequisite graphs are DAGs by construction.
- **C3. Deletion/archival is blocked while referenced.**  ⟨replaces cascade⟩
  Attempting to delete/archive an entity that is still referenced as a
  prerequisite/dependency by another entity returns **`409 DEPENDENCY_EXISTS`**
  with the list of blocking dependents. The reference must be removed first.
- **C4. Experiment start-time prerequisites:** an experiment may declare
  prerequisites (a flag in a given variant, or another experiment stopped); a
  manual or scheduled **start** is rejected with a clear reason if unmet.

### D. SDK Support (Rust)  ⟨was out-of-scope⟩
- **D1.** The Rust SDK's local snapshot carries each flag's prerequisites +
  fallback variant; the SDK supplies all prerequisite flags' definitions for
  transitive local resolution.
- **D2.** `SdkClient::evaluate(...)` returns the fallback variant when a
  prerequisite is unmet, **identically** to the preview path (shared
  `evaluate_flag`), including transitive chains and a missing/unknown prerequisite
  flag (treated as unmet → fallback).
- **D3.** Dedicated SDK tests cover: single + transitive prerequisites, the
  fallback variant, a disabled prerequisite flag, and a prerequisite flag absent
  from the snapshot.

### E. Admin UI (full parity)
- **E1. Schedule builder** on flag, segment, and experiment pages: one-shot +
  recurring; mutation/transition editor; IANA timezone picker; pending/active list
  with next-run, last-run status, diff preview; cancel / pause / resume.
- **E2. Prerequisites editor** on the flag page: add/remove `(flag, required
  variant)` deps; pick the fallback variant; live cycle warning before save.
- **E3. Dependency-graph visualization:** upstream prerequisites + downstream
  dependents for a flag (and the cross-entity edges from C1 where natural).
- **E4. Delete-blocked UX:** deleting a referenced entity shows the blocking
  dependents and the “remove references first” guidance (the `409` from C3).
- **E5. Surfacing:** flag/segment/experiment badges (“has schedule” / “has
  prerequisites” / “is a prerequisite”); preview-trace shows prerequisite gating.

## Non-Functional Requirements
- **TDD**, ≥90% per-crate coverage, `clippy -D warnings`, `cargo fmt`, sqlx offline
  cache regenerated, `cargo xtask docs` idempotent, OpenAPI contract preserved,
  admin vitest + tsc clean.
- **Backward-compatible proto additions only** (new messages/fields/RPCs; no
  breaking renumber).
- Timezone via `chrono-tz`; recurrence via a maintained cron/recurrence crate
  (document both in `tech-stack.md` before use).
- Schedules + dependency edges stored in **PostgreSQL** (canonical UTC + IANA tz).
- `privateParameters` never logged (prerequisite traces must not leak them).
- All scheduler-applied mutations audited + version-checked exactly like human
  mutations.

## Acceptance Criteria
- One-shot "disable flag at T" applies at T (live), bumps version, audited as the
  scheduler actor; experiment "start at T / stop at T+N" transitions fire.
- A recurring weekday window toggles correctly across a DST boundary in a non-UTC
  timezone.
- A scheduled apply against an experiment-locked flag (or invalid experiment
  transition) is skipped with a surfaced reason; recurring proceeds.
- A flag with an unmet prerequisite returns its **fallback variant** via both the
  preview endpoint **and** the Rust SDK; trace names the failing prerequisite;
  transitive + missing-prereq cases covered by SDK tests.
- Creating a prerequisite cycle is rejected with the cycle path; a valid DAG saves.
- Deleting/archiving a still-referenced entity returns `409 DEPENDENCY_EXISTS`
  listing dependents; succeeds after the reference is removed.
- An experiment with an unmet start-time prerequisite refuses to start (manual +
  scheduled).
- Full Admin UI flows for schedules (flag/segment/experiment), prerequisites,
  graph visualization, and delete-blocked UX.
- CI green: workspace tests, clippy, sqlx-check, vitest, tsc, docs idempotent,
  contract.

## Out of Scope
- **Approval workflows / change requests** and **webhooks / outbound
  notifications** (deferred to a future governance track).
- Client/browser/mobile SDKs (only the server-side Rust SDK is in scope).
- ClickHouse changes (schedules + dependencies are PostgreSQL-only).
- Auto-cascade *deletion* of dependents — explicitly replaced by the
  block-until-references-removed guard (C3).

## Revision History

### Revision #1 (2026-06-05) — Follow-Up Completions (now Phase 10)
The initial implementation (Phases 1–9) shipped three behaviours in a deferred/partial form;
this revision pulls them into scope for completion (no change to existing behaviour, additive):
- **A4 / C4 — `flag_variant` experiment start-prerequisites now verify exactly.** Implementation
  fell back to fail-closed because flag-service's proto exposed variant *keys* but not UUIDs.
  Phase 10.1 exposes variant UUIDs and compares the served variant exactly. (was `feature-flag-bun`)
- **C4 — experiment start-prerequisites become readable.** They were write-and-enforce only; the
  dependency-graph API's experiment branch returned a `note`. Phase 10.2 adds a read RPC + gateway
  wiring + Admin UI surfacing. (was `feature-flag-coe`)
- **A4 — segment list-generation activation.** Scheduled segment changes covered definition updates
  only (no activation RPC existed; `list_generation` payloads were rejected). Phase 10.3 adds a
  segmentation-service activation RPC and wires the scheduler to it.
