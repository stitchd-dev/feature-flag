# Plan: Domain-Boundary Refactor — Lean Gateway, Dedup, Dead-Code Audit

Methodology: audit-first; behavior-preserving (contract evolution allowed with
snapshot+docs in same batch); green gate per batch; small reviewable commits.
Safety net: the existing conformance/parity/e2e + unit suites are the
characterization tests — they MUST stay green unchanged except where a contract
change is explicitly approved.

## Phase 1: Audit & Findings [checkpoint: 5a3a5b0]
<!-- execution: parallel -->

- [x] Task 1.1: Gateway leanness audit — classify every route in
      `crates/stitchd-gateway/src/routes/*` as translation / orchestration /
      domain-logic-leak, with file:line evidence and the target service that
      should own each leaked rule.
  <!-- files: conductor/tracks/domain_boundaries_20260530/findings.md -->
- [x] Task 1.2: Domain-boundary audit — map cross-domain access; flag any
      service reaching into another domain's data/logic outside gRPC; confirm
      `stitchd-core`/`stitchd-db` ownership lines.
  <!-- files: conductor/tracks/domain_boundaries_20260530/findings.md -->
- [x] Task 1.3: Duplication audit — find repeated DTO↔proto mappers, error
      mapping, validation, ID/UUID conversions, ClickHouse row structs, hash/
      query helpers; propose a canonical home for each.
  <!-- files: conductor/tracks/domain_boundaries_20260530/findings.md -->
- [x] Task 1.4: Consistency audit — catalog divergent error handling,
      pagination, validation, naming, and RPC request/response shapes.
  <!-- files: conductor/tracks/domain_boundaries_20260530/findings.md -->
- [x] Task 1.5: Dead-code audit — unreferenced items, retired-feature remnants,
      unreachable branches; tag each DELETE-NOW vs PROPOSE against product.md's
      active-module baseline (use `cargo +nightly udeps` / `-W dead_code` / grep
      evidence).
  <!-- files: conductor/tracks/domain_boundaries_20260530/findings.md -->
- [x] Task 1.6: Synthesize `findings.md` — merge 1.1–1.5 into one dispositioned
      report (evidence, owning crate, disposition, risk, contract-impact flag);
      present the PROPOSE/contract-affecting items for approval; record agreed
      conventions seed for `patterns.md`. <!-- 880277a -->
  <!-- files: conductor/tracks/domain_boundaries_20260530/findings.md -->
  <!-- depends: task1, task2, task3, task4, task5 -->
- [ ] Task: Conductor - User Manual Verification 'Audit & Findings' (Protocol in workflow.md)

## Phase 2: Lean-Gateway Refactor
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [x] Task 2.1: Establish characterization coverage — for each gateway route
      carrying domain logic, confirm a test pins current REST behavior; add
      characterization tests (Red where gaps exist) before moving logic. <!-- b5bdeae -->
- [x] Task 2.2: Flags + evaluation routes (`flags.rs`) — move validation/
      mapping/evaluation-adjacent logic into flag-service gRPC handlers; extend
      proto where needed (snapshot+docs same batch). Reduce routes to
      translate→call→map→error. <!-- 74bc8d3,bdaef7a — GL-02,03,04,05,11 done; GL-06 kept (Admin UI compat); GL-07 deferred (proto restructure) -->
- [x] Task 2.3: Experiments + metrics routes (`experiments.rs`, `metrics.rs`) —
      same treatment; domain logic → experimentation/analytics services. <!-- 2f23e7a,61dbcc0,e93d76b -->
- [x] Task 2.4: Events + event-admin routes (`events.rs`, `event_admin.rs`) —
      move ingestion/quota/validation domain rules into analytics service; keep
      quota middleware as cross-cutting in gateway. <!-- 273bedd,14c382d -->
- [x] Task 2.5: Segments + management + stats + auth-provider routes — same
      treatment for the remaining route groups. <!-- 0a305e4 — GL-11 done in 2.2 -->
- [x] Task 2.6: Re-audit gateway — verify zero domain logic remains; update
      OpenAPI snapshot + contract-check for any approved contract changes. <!-- bcedef0 -->
- [ ] Task: Conductor - User Manual Verification 'Lean-Gateway Refactor' (Protocol in workflow.md)

## Phase 3: De-duplication & Consistency
<!-- execution: sequential -->
<!-- depends: phase2 -->

- [ ] Task 3.1: Consolidate mappers/error-mapping/validation/ID conversions to
      their canonical crate; update all call sites.
- [ ] Task 3.2: Unify ClickHouse row structs + hash/query helpers behind single
      canonical definitions (continuation of the `EventRow`/`EventV2Row`
      consolidation pattern).
- [ ] Task 3.3: Apply consistent error/pagination/validation/naming/RPC-shape
      patterns across services; record decisions in `conductor/patterns.md`.
- [ ] Task: Conductor - User Manual Verification 'De-duplication & Consistency' (Protocol in workflow.md)

## Phase 4: Dead-Code Removal
<!-- execution: sequential -->
<!-- depends: phase3 -->

- [ ] Task 4.1: Auto-remove DELETE-NOW items (unreferenced / retired-feature /
      unreachable); ensure no dangling refs, docs, or proto fields.
- [ ] Task 4.2: Execute approved PROPOSE-tier removals from the audit.
- [ ] Task 4.3: Verify with `-W dead_code` / `udeps` / grep that no newly-dead
      code was introduced by Phases 2–3.
- [ ] Task: Conductor - User Manual Verification 'Dead-Code Removal' (Protocol in workflow.md)

## Phase 5: Final Verification & Sync
<!-- execution: sequential -->
<!-- depends: phase4 -->

- [ ] Task 5.1: Full green gate on the branch — workspace tests (--no-fail-fast),
      clippy -D warnings, fmt, contract-check, sqlx-check, docs idempotency.
- [ ] Task 5.2: Refresh `conductor/tech-stack.md` / `product.md` notes for any
      approved contract/boundary changes; finalize `patterns.md`.
- [ ] Task 5.3: Confirm flag-evaluation/experiment/auth behavior unchanged
      (conformance/parity/e2e pass unmodified).
- [ ] Task: Conductor - User Manual Verification 'Final Verification & Sync' (Protocol in workflow.md)
