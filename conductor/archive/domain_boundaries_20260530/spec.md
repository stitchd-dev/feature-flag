# Spec: Domain-Boundary Refactor — Lean Gateway, Dedup, Dead-Code Audit

Track type: Refactor (audit-first)
Status: new

## Overview

The seven-service stitchd backend has accreted boundary erosion over many tracks:
the REST gateway (15.5K LOC; `events.rs` 2.4K, `flags.rs` 2.2K, `experiments.rs`
1.7K) carries domain logic that belongs in services, several concepts are
implemented more than once across crates, conventions drift between modules, and
retired features have left dead code behind.

This track runs **audit-first**: Phase 1 produces an evidence-backed findings
report with a per-item disposition; subsequent phases execute the approved
changes in small, individually-verifiable batches. The end state is a **lean
gateway** that only translates REST↔gRPC, enforces cross-cutting concerns
(auth, rate-limit, quota), and orchestrates multi-service calls — with **zero
domain logic** — while every domain rule lives in its owning service behind a
gRPC API.

## Goals

1. **Lean gateway**: gateway routes contain no validation beyond shape, no
   domain mapping, no business rules, no direct evaluation/hashing/query
   building — only translation + orchestration + cross-cutting middleware.
2. **Proper domain boundaries**: each domain (auth, flag, segmentation,
   analytics, experimentation, stats) owns its rules; cross-domain access is
   via gRPC only. `stitchd-core`/`stitchd-db` remain shared libraries with
   clear ownership.
3. **De-duplication**: a single canonical implementation for each repeated
   concept (DTO↔proto mappers, error mapping, validation, ID/UUID conversions,
   ClickHouse row structs, hash/query helpers).
4. **Consistency**: uniform conventions for error handling, pagination,
   validation, naming, and RPC request/response shapes across services.
5. **Dead-code removal**: remove code that is unreferenced, gated behind retired
   features, or unreachable — judged against `conductor/product.md`'s active
   module list as the product-need baseline.

## Functional Requirements

### FR1 — Audit & Findings Report (Phase 1 deliverable)
- Produce `conductor/tracks/domain_boundaries_20260530/findings.md` cataloging,
  with file:line evidence:
  - **Boundary violations**: domain logic in the gateway, per route group.
  - **Gateway leanness gaps**: each gateway route classified as
    translation / orchestration / *leaks domain logic* (the line is
    "pure translation + orchestration, zero domain logic").
  - **Duplicates**: repeated mappers/validators/error-mapping/row-structs/
    helpers, with the proposed canonical home.
  - **Inconsistencies**: divergent error handling, pagination, validation,
    naming, RPC shapes.
  - **Dead code**: unreferenced items, retired-feature remnants, unreachable
    branches — each tagged DELETE-NOW (unambiguous) or PROPOSE (judgment call).
- Each finding carries: evidence, owning crate, disposition, risk, and a
  contract-impact flag (does fixing it change the REST/gRPC surface?).

### FR2 — Disposition gate
- Unambiguous dead code (no references, behind retired features, unreachable)
  is auto-removed in the cleanup phase.
- Judgment-call removals, boundary moves, and any contract-affecting change are
  listed for explicit approval before execution.

### FR3 — Lean-gateway refactor
- Move domain logic out of gateway routes into the owning service's gRPC
  handler (extend the proto/RPC where required).
- Gateway routes reduced to: parse/validate-shape → call service(s) →
  map response → translate errors.

### FR4 — De-duplication & consistency
- Consolidate each duplicated concept to one implementation in its canonical
  crate; update call sites.
- Apply consistent error/pagination/validation/naming patterns; record the
  chosen patterns in `conductor/patterns.md`.

### FR5 — Dead-code removal
- Execute approved deletions; ensure no dangling references, docs, or proto
  fields remain.

## Non-Functional Requirements

- **NFR1 — Behavior-preserving with allowed contract evolution**: internal
  behavior (flag evaluation results, experiment math, auth decisions) is
  unchanged. The REST/gRPC surface MAY change where it improves boundaries,
  but only with the OpenAPI snapshot, `.proto`, generated stubs, contract-check,
  and docs updated **in the same batch**.
- **NFR2 — Green gate per batch**: every batch passes the full workspace test
  suite (`--no-fail-fast`), `clippy --all-targets -D warnings`, `cargo fmt
  --check`, `check_openapi_contract.py`, and sqlx offline-cache check before
  commit.
- **NFR3 — Small reviewable batches**: one boundary/dedup/dead-code concern per
  commit; no mega-commits.
- **NFR4 — No new behavior**: this track adds no features; product surface is
  unchanged unless a contract change is explicitly approved.

## Acceptance Criteria

- [ ] `findings.md` exists with evidence-backed, dispositioned findings across
      all five categories.
- [ ] Gateway routes contain no domain logic (validation beyond shape, mapping,
      rules, evaluation/hashing/query-building) — verified by re-audit.
- [ ] Each previously-duplicated concept has exactly one canonical
      implementation; call sites updated.
- [ ] Approved dead code removed; `cargo build`, clippy, and the dead-code lint
      pass clean; no dangling references/docs/proto fields.
- [ ] Chosen conventions recorded in `conductor/patterns.md`.
- [ ] Full CI green on the track branch (all jobs, including Coverage and Docs).
- [ ] No change to flag-evaluation, experiment, or auth behavior (proven by the
      existing conformance/parity/e2e suites still passing unchanged).

## Out of Scope

- New product features or modules.
- Admin UI (`admin/`) refactor beyond what a gateway contract change forces.
- Performance optimization (except incidental wins from dedup).
- Database schema/migration changes (no new migrations) unless an approved
  contract change strictly requires one.
- Cross-SDK spec changes beyond keeping `sdks/spec` consistent with approved
  contract evolution.
