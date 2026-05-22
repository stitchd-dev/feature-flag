# Track Learnings: flag_eval_unify_20260522

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### From flag_eval_preview_20260514 (parent track)

- **Admin vs SDK response shape:** Always define a separate `AdminFooJson` struct in the gateway for admin UI responses. The SDK-facing `FooJson` must stay minimal. (from: flags_crud_20260512)
- **Proto condition payload:** `rule_payload` in `FlagRule` is `serde_json::to_vec(&ConditionExpr)` — a JSON-encoded condition tree stored as bytes. Deserialize with `serde_json::from_slice`. (from: flags_crud_20260512)
- **Domain model change order:** `stitchd-core` structs → DB repo → flag service → proto → mapping.rs → gateway handler. Skipping steps causes compile errors deep in the chain. (from: flags_crud_20260512)
- **`verbatimModuleSyntax`:** Always use `import type { Foo }` for type-only imports in the admin UI. (from: admin_ui_20260427)
- **RBAC UI gating:** Use `disabled` + `style={{ opacity: 0.35 }}` for actions lacking permission. (from: env_sdk_rbac_20260429)
- **Cargo must run from the worktree root:** Always `cd .worktrees/flag_eval_unify_20260522/` before Cargo commands. (from: env_sdk_rbac_20260429)

### Additional patterns directly relevant to this track

- **Formik + Yup is the only form pattern in admin UI:** All admin forms use `<Formik>` + Yup schema. Primitives in `admin/src/components/form/`; schemas in `admin/src/lib/validation/`. API errors surface via `formik.setStatus({ error: message })`, rendered by `<FormErrorBanner />`. (from: boundaries_20260518)
- **`validateOnChange={false}` for async Yup validators:** When using async `.test()` validators, always set `validateOnChange={false}` on `<Formik>` to prevent an API call on every keystroke. Trigger validation on blur or submit only. (from: boundaries_20260518)
- **`cargo sqlx prepare` skips `#[cfg(test)]` by default:** The `-- --tests` flag is REQUIRED on `cargo sqlx prepare --workspace -- --tests`. Without it, test-only queries silently leave the offline cache unpopulated and CI fails with `no cached data for this query`. (from: scheduled_stats_20260423 + workflow.md)
- **`STITCHD_DATABASE_URL` vs `DATABASE_URL`:** sqlx-cli needs plain `DATABASE_URL`. Always `export DATABASE_URL="$STITCHD_DATABASE_URL"` before running sqlx commands. (from: boundaries_20260518)
- **In-process tonic mock servers for integration tests:** Bind `TcpListener::bind("127.0.0.1:0")` for a random free port; wrap with `tokio_stream::wrappers::TcpListenerStream`; pass to `tonic::transport::Server::builder().serve_with_incoming(...)` in a `tokio::spawn`. No external mocking library, no port conflicts. (from: boundaries_20260518)
- **`--features test-util` required for SDK clippy `--all-targets`:** `cargo clippy -p stitchd-sdk-rust --all-targets` fails with unresolved imports unless `--features test-util` is passed — conformance test helpers are behind that feature gate. (from: boundaries_20260518)
- **E2E infra-dependent tests need explicit `#[ignore]`:** Tests that require a running service daemon must be marked `#[ignore = "needs running <service>"]`. Without the annotation, the test attempts a real connection and fails silently in CI. (from: boundaries_20260518)
- **Recursive Types:** Recursive enums or structs (expression trees) must use `Box<T>` for recursive variants. Relevant when extending `ConditionExpr` or related rule-engine types. (from: rule_engine_20260412)
- **Discovered out-of-scope work:** When a worker finds a pre-existing issue clearly outside the current task scope, file a new beads bug with priority 2 and reference it from the report-back. Do not fix inline — it bloats the diff and may conflict with planned work. (from: boundaries_20260518)

### Track-specific context

- **Two paths to unify (current state):**
  - Preview path: `crates/stitchd-core/src/evaluation/preview.rs::evaluate_preview()` and `evaluate_single()`, called from `crates/stitchd-flag-service/src/service.rs:871 (FlagServiceImpl::evaluate_preview)`. Pre-fetches list memberships from Scylla via `resolve_list_memberships()` at service.rs:197–262.
  - SDK path: `sdks/rust/src/client.rs::SdkClient::evaluate_inner()` (line 762), plus `resolve_segments()` (line 887). LFU-cached list memberships via the in-SDK `MembershipCache`.
- **Shared primitives already in core (KEEP as-is):**
  - `stitchd-core::hashing::calculate_allocation` (Murmur3 → 0–100.0)
  - `stitchd-core::rule_engine::eval_expr` + `eval_leaf` (segment membership check reads from `EvaluationInput.resolved_segments`)
  - `stitchd-core::segment::{SegmentEvaluator, RuleBasedSegment, ListBasedSegment}`
- **Bucket-mapping invariant (preserve byte-equivalence):** `((percentage * 10.0).floor() as u32).min(999)` — same in both paths today. Do NOT change this expression.
- **Empty-string sentinel for missing context/parameter (preserve):** Preview's current behaviour when a `PercentageTarget` selector resolves to a missing context/parameter is to push an empty string into the hash input list. This must be preserved by the unified `HashInputSpec` resolver for hash-stability.

---

<!-- Learnings from implementation will be appended below -->

## [2026-05-22] Phase 3 — Proto + PG schema migration (Worker P3)

- **Implemented:**
  - `crates/stitchd-db/migrations/20260522000001_hash_input_spec_cutover.sql` —
    new JSONB columns `feature_flag_rules.hash_inputs` and
    `feature_flags.default_rule_hash_inputs`. Backfill of `hash_inputs` from
    existing `rule_def->'output'->'Percentage'->'targets'` using canonical sort
    (`context_type ASC, parameter ASC within type`). Legacy `rule_def` and
    `default_rule_distribution` columns preserved — dual-schema state for Phase
    5/6 to clean up.
  - `crates/stitchd-db/tests/hash_input_spec_cutover.rs` — frozen-corpus +
    canonical-sort + bucket-parity tests (pure-Rust portion) plus `#[sqlx::test]`
    schema assertions + a backfill round-trip test.
  - `proto/flags/v1/flag_sync.proto` — added new `HashSelector` /
    `ContextKeySelector` / `ContextParameterSelector` messages + new
    `PercentageAllocation.hash_inputs` repeated field at tag 3. Legacy
    `context_hash_specs` map at tag 1 retained.
  - `crates/xtask/src/main.rs` — added new `verify-hash-cutover` subcommand;
    `crates/xtask/fixtures/hash_cutover_corpus.json` fixture file.
- **Deliberate scope split with sibling worker P2 (Phase 2 / Phase 5/6):**
  Repo-layer cutover (reading/writing the new column from `PgFlagRepository`)
  is deferred to Phase 5/6. This phase only lands the schema + proto wire
  expansion. Both columns exist NULL-able; existing code paths continue to
  use `rule_def`.
- **Caveat — struct-literal compile break in `mapping.rs`:** Adding the new
  `hash_inputs` repeated field to `PercentageAllocation` broke three
  struct-literal init sites that name every field explicitly. Two are in
  `crates/stitchd-gateway/src/` (my scope) — patched to add
  `hash_inputs: vec![]`. The third is `crates/stitchd-flag-service/src/mapping.rs`
  — explicitly out-of-scope per the worker prompt, BUT the workspace will not
  compile without a minimum touch. I added the same `hash_inputs: vec![]`
  one-line field to the literal. This change is functionally inert (empty
  vec = legacy field is the only source of percentage-rule data, exactly the
  dual-schema contract). No merge conflict risk against P2: P2 owns
  `stitchd-core/**` and never touches mapping.rs.
- **Gotchas:**
  - `RuleOutput` in `stitchd-core::rule_engine::types` uses default external
    serde tagging — `Percentage` rules serialize as
    `{"Percentage": {"targets": [...], "weights": [...]}}`. The backfill SQL
    relies on this exact shape (`rule_def->'output'->'Percentage'->'targets'`).
  - PG `jsonb_array_elements` returns objects-with-`value`-column, NOT plain
    values — use `t.value->>'context_type'` not `t->>'context_type'`.
  - sqlx test runner needs `r#"..."#` (NOT `r"..."` + `\"` escape) for SQL
    strings containing literal double-quoted JSONB literals like
    `'"Key"'::jsonb`.
- **Migration test design:** The "frozen corpus" is a Rust unit-test corpus,
  not a PG-side fixture. This makes the canonical-sort + bucket-parity logic
  testable without a running DB — only the schema-assertion subset needs PG.
  Operator-review reporting (rows where canonical sort ≠ legacy insertion
  order) is via `eprintln!` rather than assertion-failure — the test surfaces
  the deltas without breaking the build.
- **Commits:** see report-back.

## [2026-05-22] Phase 1 — Foundation Types

- **Implemented:** New `crates/stitchd-core/src/evaluation/types.rs` module with `HashSelector`, `HashInputSpec`, `TraceLevel`, `ListMembershipIndex`, `EvalOutcome`, `EvaluationTrace`, `FlagEvaluationResult`. Added `evaluate_flag` signature with `todo!()` body in `evaluation/engine.rs`. Phase 1 implementation requires no behavioural change — Phase 2 ports the body.
- **Files changed:** `crates/stitchd-core/src/evaluation/types.rs` (NEW, 480 lines including tests), `crates/stitchd-core/src/evaluation/mod.rs`, `crates/stitchd-core/src/evaluation/engine.rs`
- **Commit:** 7c7dfa0
- **Learnings:**
  - Patterns:
    - `#[serde(tag = "kind", rename_all = "snake_case")]` on the `HashSelector` enum produces clean JSON like `{"kind":"context_key","context_type":"user"}` without `#[serde(other)]` gotchas. Same pattern fits `EvalOutcome`.
    - `#[serde(default, skip_serializing_if = "Option::is_none")]` on `FlagEvaluationResult::trace` keeps the SDK hot-path JSON lean (no `"trace":null` field when trace is `Off`) — tested with `assert!(!json.contains("\"trace\""))`.
  - Gotchas:
    - `SegmentId` uses `::new()` constructor (returns a fresh UUID) — NOT `SegmentId::from(Uuid)` which has no `impl From<Uuid>`. Same for `FlagId`, `VariantId`, `RuleId`, `EnvironmentId`, `ProjectId`. Generated by the macro in `crates/stitchd-core/src/id.rs`.
    - `VariantValue` variants are `StrValue` / `BoolValue` / `IntValue` / `DoubleValue` / `JsonValue` (NOT `Str` / `Bool` / `Int` / `Double`). Easy slip when reaching for "the obvious shape."
    - Existing `FlagEvaluator::evaluate` in `engine.rs` ALREADY supports `default_rule_distribution` (lines 105–136). The SDK gap is in `sdks/rust/src/client.rs::evaluate_inner` — NOT in core. The unification still consolidates orchestration, but don't claim "default_rule_distribution is missing from core" — it isn't.
  - Context:
    - `cargo fmt` will reorder `use engine::{evaluate_flag, FlagEvaluator}` to `use engine::{FlagEvaluator, evaluate_flag}` (alphabetical case-insensitive with lowercase preferred after uppercase). Don't fight it.
    - Existing `preview.rs` already exposes `ConditionTrace`, `RuleOutcome`, `RuleTrace`, `VariantRange`, `RolloutDebug`, `ContextPreviewResult`. Phase 2 should re-use them rather than redefining — `types.rs` imports `RuleTrace` and `RolloutDebug` from `super::preview`.
---

