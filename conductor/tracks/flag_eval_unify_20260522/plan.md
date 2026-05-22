# Implementation Plan: flag_eval_unify_20260522

Refactor track collapsing duplicate variant-evaluation orchestration into a
single `stitchd-core` entry point, unifying the percentage-hash schema end-to-end,
and refreshing the Admin UI rule builder to expose cross-context selectors.

TDD per `conductor/workflow.md`. Each task: failing test → minimal pass → refactor.

Phase dependency annotations enable parallel execution where the dependency
graph allows. Phases without an explicit `<!-- depends: -->` annotation default
to sequential (depends on previous phase).

---

## Phase 1: Foundation Types

Define the canonical types — every later phase consumes them.

- [ ] Task 1: Write failing tests in `crates/stitchd-core/src/evaluation/` for
      `HashSelector` (ContextKey / ContextParameter variants),
      `HashInputSpec { selectors: Vec<HashSelector> }`, `TraceLevel { Off, Full }`,
      `ListMembershipIndex` (per-context segment-id set), `FlagEvaluationResult`
      (variant + outcome + optional trace bundle), `RuleTrace`, `ConditionTrace`,
      `RolloutDebug`. Cover: empty selectors invalid, equality, serde round-trip.
- [ ] Task 2: Implement the type definitions to make tests pass.
- [ ] Task 3: Add `evaluate_flag(...)` signature in
      `stitchd-core::evaluation` with `todo!()` body + rustdoc spelling out
      purity contract and parameter semantics.
- [ ] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

---

## Phase 2: Core orchestration — port preview into evaluate_flag
<!-- depends: phase1 -->

- [ ] Task 1: Write failing tests for `evaluate_flag` happy paths:
      flag disabled → default variant; first rule fires; no rule matches →
      default variant; no rule + `default_rule_distribution` → hashed variant.
- [ ] Task 2: Implement rule iteration (first-match), rule-based segment
      evaluation via existing `SegmentEvaluator`, list-segment membership
      lookup via `ListMembershipIndex`, percentage allocation, default-rule
      fallthrough — `TraceLevel::Off` only.
- [ ] Task 3: Write failing tests for `TraceLevel::Full` output: per-rule
      `RuleTrace` (matched / no-match / skipped), per-condition `ConditionTrace`
      (OR/AND missing-context resolution), per-result `RolloutDebug`
      (hash_input, bucket, variant_ranges).
- [ ] Task 4: Implement trace generation gated by `TraceLevel::Full`.
- [ ] Task 5: Write failing tests for cross-context hashing — selectors
      drawing from multiple context_types, mixing `Key` and `Parameter`.
      Include missing-context and missing-parameter sentinel-empty cases.
- [ ] Task 6: Implement hash-input resolution from `HashInputSpec` +
      context bundle; preserve current empty-string sentinel semantics.
- [ ] Task 7: Reduce `crates/stitchd-core/src/evaluation/preview.rs` to a
      thin wrapper over `evaluate_flag(trace=Full)`, or delete it. No
      duplicate rule-iteration loop survives.
- [ ] Task 8: Add purity-assertion test (`evaluation` module's transitive
      deps must not include `tokio`, `reqwest`, `sqlx`, or
      `tracing::warn!/error!`).
- [ ] Task 9: Add zero-allocation assertion for `TraceLevel::Off` (doc test
      or microbench: returned trace `Vec` capacities == 0).
- [ ] Task 10: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

---

## Phase 3: Proto + PG schema migration
<!-- depends: phase1 -->

- [ ] Task 1: Write failing migration test using a frozen pre-migration
      corpus: rules with `context_hash_specs` maps + a set of context
      bundles. Assert post-migration hash buckets equal pre-migration for
      every rule whose canonical sort matches insertion order; report the
      remainder as operator-review.
- [ ] Task 2: Author PG migration
      `crates/stitchd-db/migrations/2026XXXXNNNNNN_hash_input_spec_cutover.sql`
      converting `context_hash_specs` (jsonb map) on `feature_flag_rules` and
      `feature_flags.default_rule_distribution` to `hash_inputs: jsonb` (ordered
      array of `{ kind: "ContextKey" | "ContextParameter", ... }`).
      Canonical sort: context_type ASC, parameter ASC within type.
- [ ] Task 3: Update `stitchd-proto/proto/.../flag.proto`: add
      `repeated HashSelector hash_inputs` to `PercentageAllocation`; drop
      `map<string, ContextHashSpec> context_hash_specs`. Regenerate stubs.
- [ ] Task 4: Update `stitchd-flag-service` repo layer + DTO conversions to
      read/write the new shape end-to-end. Regenerate sqlx offline cache via
      `SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --tests`.
- [ ] Task 5: Author `xtask` (or one-shot binary) `verify-hash-cutover`: scans
      live (or test-fixture) PG payloads, re-hashes a sample bundle against
      both schemas, prints bucket-identical vs operator-review report.
- [ ] Task 6: Update repo-layer unit tests for new shape.
- [ ] Task 7: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

---

## Phase 4: REST + gRPC rule-CRUD API refactor
<!-- depends: phase3 -->

- [ ] Task 1: Write failing integration tests for `POST /v1/flags/{id}/rules`
      with `hash_inputs` payload; cover happy path, empty-selectors rejection
      (400), duplicate-selector rejection, missing parameter on `Parameter`
      field (400).
- [ ] Task 2: Write failing tests for `PUT /v1/flags/{id}/rules/{rule_id}`
      and for default-rule-distribution update endpoint.
- [ ] Task 3: Update gateway REST DTOs (request + response); remove the old
      `context_hash_specs` field; map to/from `HashInputSpec`.
- [ ] Task 4: Update gRPC handlers in `stitchd-flag-service` (CreateRule,
      UpdateRule, UpdateDefaultRuleDistribution).
- [ ] Task 5: Server-side validation: non-empty selectors, no duplicates
      (exact context_type+field equality), parameter required & non-empty
      when field == `Parameter`.
- [ ] Task 6: Update `#[utoipa::path]` annotations + schema derives.
- [ ] Task 7: Run `cargo xtask docs` + `scripts/check_openapi_contract.py`;
      resolve drift.
- [ ] Task 8: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

---

## Phase 5: Flag-service preview rewire
<!-- depends: phase2, phase3 -->

- [ ] Task 1: Write failing test asserting `evaluate_preview` response is
      byte-equivalent to a frozen baseline for a corpus of representative
      flags (single rule, multi-rule, default-rule-distribution,
      cross-context hashing).
- [ ] Task 2: Rewire `FlagServiceImpl::evaluate_preview` in
      `crates/stitchd-flag-service/src/service.rs:871` to assemble
      `(flag, contexts, rule_based_segments, list_segment_memberships)` and
      call `evaluate_flag(trace=Full)`. Convert the result back to the
      existing `EvaluatePreviewResponse` proto.
- [ ] Task 3: Remove any leftover orchestration in
      `stitchd-core::evaluation::preview` now superseded by `evaluate_flag`.
- [ ] Task 4: Run full preview integration test suite; confirm zero
      regression.
- [ ] Task 5: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)

---

## Phase 6: SDK rewire + multi-context EvalRequest + API consolidation
<!-- depends: phase2, phase3 -->

- [ ] Task 1: Write failing test for `EvalRequest { flag_key, contexts: Vec<Context> }`
      accepting a multi-context bundle.
- [ ] Task 2: Write failing test for unified
      `SdkClient::evaluate(&[EvalRequest], TraceLevel)`:
      thin result when `Off`, rich `EvaluationTrace` when `Full`.
- [ ] Task 3: Write failing **parity test** (lives in
      `crates/stitchd-flag-service/tests/` or `sdks/rust/tests/`): for a
      shared corpus of flags + contexts + segment configs + memberships,
      preview and SDK paths return identical variants + identical traces.
      Corpus includes at least one cross-context-hash flag and one
      default-rule-distribution flag.
- [ ] Task 4: Write failing SDK-side cross-context hashing test:
      EvalRequest with user + device + application contexts, percentage
      rule mixes key + parameter selectors.
- [ ] Task 5: Update `EvalRequest` + `EvalResult` public types; collapse
      `evaluate` and `evaluate_with_reasoning` into one method.
- [ ] Task 6: Delete `evaluate_inner` + `resolve_segments` in
      `sdks/rust/src/client.rs`. New SDK eval path: snapshot → list-membership
      cache lookup → assemble `ListMembershipIndex` → `evaluate_flag`.
- [ ] Task 7: Update SDK integration tests, in-tree examples, crate-level
      rustdoc Quickstart, `sdks/rust/README.md`.
- [ ] Task 8: SDK gains `default_rule_distribution` support automatically
      via core; assert via parity test on a default-rule-distribution flag.
- [ ] Task 9: Conductor - User Manual Verification 'Phase 6' (Protocol in workflow.md)

---

## Phase 7: Admin UI — cross-context selector control
<!-- depends: phase4 -->

- [ ] Task 1: Write failing Vitest + RTL tests for new
      `HashInputSelectorList` component: render N selectors, add row,
      remove row, reorder via drag and via keyboard, Yup error rendering
      on submit.
- [ ] Task 2: Write failing Yup schema test in
      `admin/src/lib/validation/` — non-empty array, unique selectors,
      parameter required when field == `Parameter`.
- [ ] Task 3: Implement `HashInputSelectorList` component in
      `admin/src/components/flag/` (ordered list, drag handles + keyboard
      reorder, accessibility roles).
- [ ] Task 4: Context-type picker bound to
      `GET /v1/environments/{env_id}/context-types`.
- [ ] Task 5: Parameter autocomplete bound to
      `GET /v1/environments/{env_id}/context-types/{ct}/params`.
- [ ] Task 6: Helper banner showing the live worked-example string
      (e.g. `hash(user.key || user.params.name || device.params.os)`).
- [ ] Task 7: Wire into rule builder form (percentage rule output AND
      default-rule distribution); update TypeScript types to match new
      REST JSON shape.
- [ ] Task 8: Manual UI smoke: create flag with cross-context percentage
      rule, save, reopen, edit, save again — verify round-trip identity.
- [ ] Task 9: Conductor - User Manual Verification 'Phase 7' (Protocol in workflow.md)

---

## Phase 8: End-to-end verification + docs
<!-- depends: phase5, phase6, phase7 -->

- [ ] Task 1: Write failing e2e test in `tests/e2e/`: spin up gateway +
      flag-service + segmentation + auth; UI-style POST creates rule with
      cross-context selectors → evaluate-preview returns expected variant →
      SDK eval against the same flag returns identical variant.
- [ ] Task 2: Run `cargo test --workspace` + `cd admin && npm test`.
- [ ] Task 3: Run `cargo clippy --workspace --all-targets -- -D warnings`
      and `cargo fmt --all --check`.
- [ ] Task 4: Run `cargo tarpaulin -p stitchd-core -p stitchd-flag-service
      -p stitchd-sdk-rust -p stitchd-gateway`; confirm ≥90% per crate.
- [ ] Task 5: Run `cargo run --manifest-path crates/xtask/Cargo.toml -- docs`
      and `git diff --exit-code`.
- [ ] Task 6: Update `conductor/product.md` (unified eval entry, new
      percentage-hash schema, SDK cross-context support) and
      `conductor/tech-stack.md` (new core type, new proto field).
- [ ] Task 7: Conductor - User Manual Verification 'Phase 8' (Protocol in workflow.md)
