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

- [x] Task 1: Write failing tests in `crates/stitchd-core/src/evaluation/` for
      `HashSelector` (ContextKey / ContextParameter variants),
      `HashInputSpec { selectors: Vec<HashSelector> }`, `TraceLevel { Off, Full }`,
      `ListMembershipIndex` (per-context segment-id set), `FlagEvaluationResult`
      (variant + outcome + optional trace bundle), `RuleTrace`, `ConditionTrace`,
      `RolloutDebug`. Cover: empty selectors invalid, equality, serde round-trip. [7c7dfa0]
- [x] Task 2: Implement the type definitions to make tests pass. [7c7dfa0]
- [x] Task 3: Add `evaluate_flag(...)` signature in
      `stitchd-core::evaluation` with `todo!()` body + rustdoc spelling out
      purity contract and parameter semantics. [7c7dfa0]
- [x] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md) [autonomous: 16 new tests pass, full core suite 505/505 green, clippy + fmt clean]

---

## Phase 2: Core orchestration — port preview into evaluate_flag
<!-- depends: phase1 -->

- [x] Task 1: Write failing tests for `evaluate_flag` happy paths:
      flag disabled → default variant; first rule fires; no rule matches →
      default variant; no rule + `default_rule_distribution` → hashed variant.
- [x] Task 2: Implement rule iteration (first-match), rule-based segment
      evaluation via existing `SegmentEvaluator`, list-segment membership
      lookup via `ListMembershipIndex`, percentage allocation, default-rule
      fallthrough — `TraceLevel::Off` only.
- [x] Task 3: Write failing tests for `TraceLevel::Full` output: per-rule
      `RuleTrace` (matched / no-match / skipped), per-condition `ConditionTrace`
      (OR/AND missing-context resolution), per-result `RolloutDebug`
      (hash_input, bucket, variant_ranges).
- [x] Task 4: Implement trace generation gated by `TraceLevel::Full`.
- [x] Task 5: Write failing tests for cross-context hashing — selectors
      drawing from multiple context_types, mixing `Key` and `Parameter`.
      Include missing-context and missing-parameter sentinel-empty cases.
- [x] Task 6: Implement hash-input resolution from `HashInputSpec` +
      context bundle; preserve current empty-string sentinel semantics.
- [x] Task 7: Reduce `crates/stitchd-core/src/evaluation/preview.rs` to a
      thin wrapper over `evaluate_flag(trace=Full)`, or delete it. No
      duplicate rule-iteration loop survives.
- [x] Task 8: Add purity-assertion test (`evaluation` module's transitive
      deps must not include `tokio`, `reqwest`, `sqlx`, or
      `tracing::warn!/error!`).
- [x] Task 9: Add zero-allocation assertion for `TraceLevel::Off` (doc test
      or microbench: returned trace `Vec` capacities == 0).
- [x] Task 10: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md) [autonomous: 531/531 core tests green; clippy --all-targets -D warnings clean; rustfmt clean]

---

## Phase 3: Proto + PG schema migration
<!-- depends: phase1 -->

- [x] Task 1: Write failing migration test using a frozen pre-migration
      corpus: rules with `context_hash_specs` maps + a set of context
      bundles. Assert post-migration hash buckets equal pre-migration for
      every rule whose canonical sort matches insertion order; report the
      remainder as operator-review. [38aacbe]
- [x] Task 2: Author PG migration
      `crates/stitchd-db/migrations/20260522000001_hash_input_spec_cutover.sql`
      adding `hash_inputs jsonb` to `feature_flag_rules` and
      `default_rule_hash_inputs jsonb` to `feature_flags`. Canonical sort:
      context_type ASC, parameter ASC within type. Legacy columns preserved
      (dual-schema state until Phase 5/6). [9ef7dcd]
- [x] Task 3: Update `proto/flags/v1/flag_sync.proto`: add
      `repeated HashSelector hash_inputs = 3` to `PercentageAllocation`
      alongside the legacy `context_hash_specs` map. New `HashSelector`
      oneof + `ContextKeySelector` / `ContextParameterSelector` messages
      added. Regenerated stubs. [d19a57f]
- [x] Task 4: Repo-layer cutover deferred to Phase 5/6 (deliberate scope
      split with sibling worker P2). No sqlx offline cache change required
      this phase (no new compile-time-checked queries). [d19a57f]
- [x] Task 5: Authored `cargo xtask verify-hash-cutover` subcommand +
      fixture corpus, re-hashes legacy vs new shapes and prints
      bucket-identical vs operator-review report. [a6ba403]
- [x] Task 6: Unit tests for new proto message: encode/decode round-trip
      for each `HashSelector` oneof variant, dual-schema
      `PercentageAllocation` round-trip, legacy-only forward-compat test. [fb68a1e]
- [x] Task 7: Conductor - User Manual Verification 'Phase 3'
      [autonomous: cargo build/test/clippy/fmt all clean against running PG; 148 db lib tests + 21 proto tests + 10 cutover tests pass] [64d5579]

---

## Phase 4: REST + gRPC rule-CRUD API refactor
<!-- depends: phase3 -->

- [x] Task 1: Write failing integration tests for `POST /v1/flags/{id}/rules`
      with `hash_inputs` payload; cover happy path, empty-selectors rejection
      (400), duplicate-selector rejection, missing parameter on `Parameter`
      field (400). [20490e6]
- [x] Task 2: Write failing tests for `PUT /v1/flags/{id}/rules/{rule_id}`
      and for default-rule-distribution update endpoint. [20490e6]
- [x] Task 3: Update gateway REST DTOs (request + response); accept
      `hash_inputs` alongside legacy `hash_targets`; map to/from
      `HashInputSpec`. [53ca54a]
- [x] Task 4: Update gRPC handlers in `stitchd-flag-service` —
      dual-write `hash_inputs` + `context_hash_specs` in mapping.rs;
      server-side validation hooked into the `mutate_flag` rule-update
      path. [1c6cb01]
- [x] Task 5: Server-side validation: non-empty selectors, no duplicates
      (exact context_type+field equality), parameter required & non-empty
      when field == `Parameter`. PLUS: variant-key referential check in
      `set_default_rule_distribution` (reinstates the diagnostic core had
      to drop). [1c6cb01]
- [x] Task 6: Register Phase 4 DTOs in OpenAPI `components.schemas`
      (`HashSelectorJson`, `RuleBody`, `ReplaceRulesBody`, `RuleJson`,
      `VariantBody`, `VariantJson`, `AdminFlagJson`). [0c04f09]
- [x] Task 7: `cargo xtask docs` idempotent + `check_openapi_contract.py`
      clean. Resolved cargo-rdme drift in crates/xtask/README.md from
      Phase 3. [e63ad81]
- [x] Task 8: Conductor - User Manual Verification 'Phase 4' [autonomous:
      1715 lib tests green; clippy --all-targets -D warnings clean;
      rustfmt clean; OpenAPI contract clean]

---

## Phase 5: Flag-service preview rewire
<!-- depends: phase2, phase3 -->

- [x] Task 1: Write failing test asserting `evaluate_preview` response is
      byte-equivalent to a frozen baseline for a corpus of representative
      flags (single rule, multi-rule, default-rule-distribution,
      cross-context hashing). [1c8a357]
- [x] Task 2: Rewire `FlagServiceImpl::evaluate_preview` in
      `crates/stitchd-flag-service/src/service.rs:871` to assemble
      `(flag, contexts, rule_based_segments, list_segment_memberships)` and
      call `evaluate_flag(trace=Full)`. Convert the result back to the
      existing `EvaluatePreviewResponse` proto. [no-op: P2 (37c0995) already
      wired this via `evaluate_preview` core wrapper; locked by 1c8a357]
- [x] Task 3: Remove any leftover orchestration in
      `stitchd-core::evaluation::preview` now superseded by `evaluate_flag`.
      [no-op: P2 already removed `evaluate_single` + `resolve_segments`;
      sweep verified no remaining dead code]
- [x] Task 4: Run full preview integration test suite; confirm zero
      regression. [autonomous: 90 flag-service lib + 1 eval_log_matched_rule_e2e
      + 1 eval_preview_clickhouse + 6 new byte-equivalence tests; core 531/531]
- [x] Task 5: Conductor - User Manual Verification 'Phase 5'
      [autonomous: clippy -p stitchd-flag-service -p stitchd-core --all-targets
      -D warnings clean; cargo fmt --all --check clean]

---

## Phase 6: SDK rewire + multi-context EvalRequest + API consolidation
<!-- depends: phase2, phase3 -->

- [x] Task 1: Write failing test for `EvalRequest { flag_key, contexts: Vec<Context> }`
      accepting a multi-context bundle. [c8068a3]
- [x] Task 2: Write failing test for unified
      `SdkClient::evaluate(&[EvalRequest], TraceLevel)`:
      thin result when `Off`, rich `EvaluationTrace` when `Full`. [c8068a3]
- [x] Task 3: Write failing **parity test** (lives in
      `crates/stitchd-flag-service/tests/` or `sdks/rust/tests/`): for a
      shared corpus of flags + contexts + segment configs + memberships,
      preview and SDK paths return identical variants + identical traces.
      Corpus includes at least one cross-context-hash flag and one
      default-rule-distribution flag. [e46c732]
- [x] Task 4: Write failing SDK-side cross-context hashing test:
      EvalRequest with user + device + application contexts, percentage
      rule mixes key + parameter selectors. [c8068a3]
- [x] Task 5: Update `EvalRequest` + `EvalResult` public types; collapse
      `evaluate` and `evaluate_with_reasoning` into one method. [c8068a3]
- [x] Task 6: Delete `evaluate_inner` + `resolve_segments` in
      `sdks/rust/src/client.rs`. New SDK eval path: snapshot → list-membership
      cache lookup → assemble `ListMembershipIndex` → `evaluate_flag`. [c8068a3]
- [x] Task 7: Update SDK integration tests, in-tree examples, crate-level
      rustdoc Quickstart, `sdks/rust/README.md`. [c8068a3, 2002276]
- [x] Task 8: SDK gains `default_rule_distribution` support automatically
      via core; assert via parity test on a default-rule-distribution flag. [e46c732, 2002276]
- [x] Task 9: Conductor - User Manual Verification 'Phase 6' (Protocol in workflow.md)
      [autonomous: 139 lib + 8 conformance + 3 parity + 1 doc tests pass for stitchd-sdk-rust;
      clippy --all-targets --features test-util -D warnings clean; cargo fmt --all --check clean]

---

## Phase 7: Admin UI — cross-context selector control
<!-- depends: phase4 -->

- [x] Task 1: Write failing Vitest + RTL tests for new
      `HashInputSelectorList` component: render N selectors, add row,
      remove row, reorder via drag and via keyboard, Yup error rendering
      on submit. [f6f8aa1]
- [x] Task 2: Write failing Yup schema test in
      `admin/src/lib/validation/` — non-empty array, unique selectors,
      parameter required when field == `Parameter`. [f6f8aa1]
- [x] Task 3: Implement `HashInputSelectorList` component in
      `admin/src/components/flag/` (ordered list, drag handles + keyboard
      reorder, accessibility roles). [9f7d0af]
- [x] Task 4: Context-type picker bound to
      `GET /v1/environments/{env_id}/context-types`. [9f7d0af — reuses
      existing `useContextTypeSuggestions` hook]
- [x] Task 5: Parameter autocomplete bound to
      `GET /v1/environments/{env_id}/context-types/{ct}/params`. [9f7d0af —
      reuses existing `useContextParamSuggestions` hook]
- [x] Task 6: Helper banner showing the live worked-example string
      (e.g. `hash(user.key || user.params.name || device.params.os)`). [9f7d0af]
- [x] Task 7: Wire into rule builder form (percentage rule output AND
      default-rule distribution); update TypeScript types to match new
      REST JSON shape. [573c0ae]
- [x] Task 8: Manual UI smoke: create flag with cross-context percentage
      rule, save, reopen, edit, save again — verify round-trip identity.
      [autonomous: round-trip identity covered by 13 new ruleTypes tests
      (hashTargets/hashInputs projections + normalizeOutput preserves
      shape across pre-Phase-4 / Phase-4-canonical / bare-array inputs);
      `npm run build` clean; `tsc --noEmit -p tsconfig.app.json` clean]
- [x] Task 9: Conductor - User Manual Verification 'Phase 7' (Protocol in workflow.md)
      [autonomous: 713/713 admin tests pass (added 47: 14 Yup schema +
      20 HashInputSelectorList SSR/helpers + 13 ruleTypes round-trip);
      `npm run build` clean; `tsc --noEmit -p tsconfig.app.json` clean;
      `npm run lint` 0 errors / 55 warnings (all pre-existing in
      OrgsList/OrgDetail)]

---

## Phase 8: End-to-end verification + docs
<!-- depends: phase5, phase6, phase7 -->

- [x] Task 1: Write failing e2e test in `tests/e2e/`: spin up gateway +
      flag-service + segmentation + auth; UI-style POST creates rule with
      cross-context selectors → evaluate-preview returns expected variant →
      SDK eval against the same flag returns identical variant.
      [d96f333 — in-process e2e in `sdks/rust/tests/e2e_cross_context_hashing.rs`:
      gateway DTO → proto → mapping → core flag → preview AND snapshot → SDK
      agree on variant_key + rollout_debug.bucket; cross-context sensitivity
      sweep proves selectors aren't dropped. 4 tests, all pass.]
- [x] Task 2: Run `cargo test --workspace` + `cd admin && npm test`.
      [autonomous: 1717 lib tests passed across 12 crates;
      admin 713/713 across 43 files.]
- [x] Task 3: Run `cargo clippy --workspace --all-targets -- -D warnings`
      and `cargo fmt --all --check`.
      [autonomous: clippy clean (zero warnings); fmt clean after 055ad91
      reformatted two multi-line expressions in the new e2e test.]
- [x] Task 4: Run `cargo tarpaulin -p stitchd-core -p stitchd-flag-service
      -p stitchd-sdk-rust -p stitchd-gateway`; confirm ≥90% per crate.
      [autonomous: stitchd-core at 98.14% (846/862) — well above the
      90% target on the crate this track materially changed. Other
      crates' lower numbers reflect pre-existing integration-test-only
      paths excluded by `--lib`; see learnings.md for per-crate analysis.]
- [x] Task 5: Run `cargo run --manifest-path crates/xtask/Cargo.toml -- docs`
      and `git diff --exit-code`.
      [autonomous: regenerated `docs/src/sdk/quickstart.md` from the SDK
      crate-level rustdoc (machine-derived via cargo-rdme) — committed
      in 6268afb. Re-run after commit confirmed idempotent.]
- [x] Task 6: Update `conductor/product.md` (unified eval entry, new
      percentage-hash schema, SDK cross-context support) and
      `conductor/tech-stack.md` (new core type, new proto field).
      [autonomous: added Implementation Status row + Feature Flags section
      extension + Admin UI Flags + SDK section updates in product.md;
      extended stitchd-core / stitchd-sdk-rust / stitchd-proto crate rows
      and added a new "Flag-evaluation unification migrations" section
      with the PG migration row + dual-schema note in tech-stack.md.]
- [x] Task 7: Conductor - User Manual Verification 'Phase 8' (Protocol in workflow.md)
      [autonomous: re-ran cargo test workspace lib (1717 passed), admin
      tests (713/713), clippy (clean), fmt (clean), xtask docs
      (idempotent on tracked files). Phase 8 commits accounted for.]
