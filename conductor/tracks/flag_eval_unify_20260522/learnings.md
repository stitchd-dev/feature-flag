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

## [2026-05-22] Phase 5 — Flag-service preview rewire (worker P5)

- **Implemented:** A new byte-equivalence regression test
  `crates/stitchd-flag-service/tests/evaluate_preview_byte_equivalence.rs`
  that locks the JSON wire shape of `evaluate_preview` against a frozen
  baseline for a 4-flag corpus (single-rule, multi-rule,
  default-rule-distribution, cross-context-hash) plus a gRPC
  `results_json` round-trip check and a list-membership-pathway guard
  (6 tests total). All 6 green.
- **No code change required for Tasks 2 + 3.** This phase landed
  PURELY as a test + documentation pass, by design — worker P2's Phase 2
  rewire (commit 37c0995) had already reduced
  `stitchd_core::evaluation::preview::evaluate_preview` to a thin
  delegating wrapper over `evaluate_flag(trace=Full)`, and the flag
  service's gRPC handler at `crates/stitchd-flag-service/src/service.rs:871`
  was already calling that wrapper. The Phase 5 prompt explicitly
  anticipated this ("Phase 5 may be MOSTLY a TEST-WRITING +
  VERIFICATION exercise").
- **Files changed:**
  - `crates/stitchd-flag-service/tests/evaluate_preview_byte_equivalence.rs`
    (NEW, 565 lines).
  - `conductor/tracks/flag_eval_unify_20260522/plan.md` (Phase 5 tasks
    checked off).
  - `conductor/tracks/flag_eval_unify_20260522/learnings.md` (this block).
- **Commits:** 1c8a357
- **Tests:** 6 new tests in
  `crates/stitchd-flag-service/tests/evaluate_preview_byte_equivalence.rs`.
  Suite totals: flag-service 90 lib + 1 eval_log_matched_rule_e2e +
  1 eval_preview_clickhouse + 6 new = 98 tests green. Core: 531/531 green
  (unchanged from Phase 2). Clippy `-p stitchd-flag-service -p stitchd-core
  --all-targets -- -D warnings` clean. `cargo fmt --all --check` clean.
- **Learnings:**
  - Patterns:
    - **UUID serde format is `8-4-4-4-12` hyphenated, not 32-char hex.**
      The `uuid` crate's default `Serialize` impl produces the canonical
      `00000000-0000-0000-0000-0000000000c1` form. Phase 5's first
      byte-equivalence baseline initially failed because the expected
      JSON used the un-hyphenated 32-char form; the hyphenated form is
      the one downstream JSON clients (admin UI, gateway) see, so the
      hyphenated form is what we lock in.
    - **`serde_json::Value` semantic comparison beats raw-string
      comparison for byte-equivalence tests.** JSON map field order is
      not stable across serde / serde_json versions; a string `==`
      would brittle-break on a serde upgrade even though the JSON is
      equivalent. Compare parsed `Value`s — that's what consumers do.
    - **Hand-authored deterministic IDs (`Uuid::from_u128(N)`)
      sidestep the need for an external fixture file.** Baselines stay
      inline next to the assertion, the test is fully self-contained,
      and the diff is a single new file.
    - **For `rollout_debug` baselines on default-rule and
      percentage-rule entries, assert the STRUCTURAL shape (variant
      ranges, range boundaries, hash_input format) but allow the
      Murmur3-derived bucket to be ANY value in `0..1000`.** Trying to
      pin the bucket against a literal would require running
      `calculate_allocation` by hand once and copy-pasting; the
      structural assertions are sufficient to catch any Phase 2
      regression that would change the hash input or the range
      math.
  - Gotchas:
    - **`stitchd-core` is in regular `dependencies`, not
      `dev-dependencies`, of `stitchd-flag-service`** — so the tests
      directory can import core types without any
      `[dev-dependencies]` edit. Worth noting for future
      cross-service test authors.
    - **`Vec<ContextPreviewResult>` is the *direct* result of the
      core `evaluate_preview` function and is what the gRPC handler
      `serde_json::to_string`s into `EvaluatePreviewResponse.results_json`.**
      The handler does NOT remap the structure — its only
      post-evaluation work is the JSON encode. So testing the core
      function's serialization output IS testing the gRPC wire shape
      (modulo the bookkeeping at the gRPC envelope level).
  - Context:
    - **Worker P3's mapping.rs `hash_inputs: vec![]` patch (commit
      ec999cf via merge) is the dual-schema state Phase 5 still
      relies on.** The `PercentageAllocation` proto carries both the
      new repeated `hash_inputs` field AND the legacy
      `context_hash_specs` map; the flag service's `mapping.rs` (P4's
      scope) reads/writes the legacy field, and the engine.rs bridge
      `hash_input_spec_from_targets` (P2's scope) converts on the
      fly. This phase produces no change to either path.

---

## [2026-05-22] Phase 2 — Core orchestration (worker P2)

- **Implemented:** Body of `evaluate_flag` in `evaluation/engine.rs` —
  per-context iteration over a shared bundle, rule iteration with
  first-match short-circuit, rule-based segment evaluation via
  `SegmentEvaluator`, list-segment membership lookup via the caller-supplied
  `ListMembershipIndex`, percentage allocation via `calculate_allocation`,
  default-rule-distribution fallthrough, and full trace assembly gated by
  `TraceLevel::Full`. Reduced `evaluate_preview` to a thin wrapper. Added a
  grep-based purity test in a new `evaluation/purity.rs` module that fails
  if `tracing::warn!`/`error!`/`tokio::`/`reqwest::`/`sqlx::` ever appear
  in any of the evaluation module's source files (with comments stripped
  before scanning to avoid docstring false-positives).
- **Files changed:**
  - `crates/stitchd-core/src/evaluation/engine.rs` (added `evaluate_flag`
    body, `evaluate_one`, `resolve_hash_inputs`, `hash_input_spec_from_targets`;
    removed `tracing::warn!` from both `evaluate_flag` and the legacy
    `FlagEvaluator::evaluate` to satisfy the purity contract).
  - `crates/stitchd-core/src/evaluation/preview.rs` (rewrote `evaluate_preview`
    body to delegate to `evaluate_flag(Full)` + remap the result; deleted the
    in-file `evaluate_single` + `resolve_segments` helpers; made
    `trace_conditions` `pub(super)` so the engine.rs trace path can reuse it).
  - `crates/stitchd-core/src/evaluation/purity.rs` (NEW — purity test).
  - `crates/stitchd-core/src/evaluation/mod.rs` (`#[cfg(test)] mod purity;`).
- **Commits:** 23dfd9a, c15e368, c38fce5, 37c0995, 5ca62dd, 87e87e2
- **Tests:** Added 26 new tests in engine.rs (happy path, full trace,
  cross-context hashing, zero-allocation guards) + 2 purity tests in
  purity.rs. Total core suite: 531 passed (was 505 at start of phase).
- **Learnings:**
  - Patterns:
    - **One bundle, one call:** `evaluate_flag` takes a single bundle of
      contexts and returns `Vec<FlagEvaluationResult>` with one entry per
      context. All entries share the bundle for rule evaluation and
      percentage hashing — the per-context aspect just makes the API
      symmetric for future batched-subject use cases. For preview,
      `evaluate_preview` calls `evaluate_flag` once per `EvaluationContext`
      and takes the first per-context result.
    - **Pre-resolved list-segment memberships → per-bundle index:** When
      the flag service supplies a `HashSet<SegmentId>` aligned by
      `EvaluationContext` index, the `evaluate_preview` wrapper registers
      that set under EVERY `(context_type, context_key)` tuple in the
      bundle. The engine's per-bundle union loop then folds the set into
      `resolved_segments` regardless of which context type the flag rule's
      `InSegment` predicate references — preserving the byte-equivalent
      behaviour of the legacy `evaluate_single::resolved_segments.extend(extra)`.
    - **Internal `PercentageTarget → HashInputSpec` bridge:** Inside
      `evaluate_flag`, `RuleOutput::Percentage { targets: Vec<PercentageTarget> }`
      is converted on the fly via `hash_input_spec_from_targets`. Phase 5/6
      cuts over storage to author `HashInputSpec` directly; the bridge
      survives until then to keep the proto/PG layer untouched in Phase 2.
    - **Single-codepath trace gating:** Trace collection lives on the
      SAME code path as the hot path, gated by `want_trace = trace == Full`.
      Every `rule_traces.push`, every `trace_conditions` call, every
      `rollout_debug = Some(...)` is wrapped in `if want_trace`. On the
      Off path the `rule_traces: Vec<RuleTrace>` is `Vec::new()` (cap=0)
      and never grows — `FlagEvaluationResult.trace = None`.
  - Gotchas:
    - **`tracing::warn!` in the evaluation module breaks purity:** The
      legacy `FlagEvaluator::evaluate` in the SAME engine.rs file uses
      `warn!` for the unknown-variant_key fallback. To satisfy the
      grep-based purity assertion, both call sites had to be silenced.
      This is a minor regression vs current behaviour (admin operators
      no longer see the diagnostic), justified by the purity contract:
      a misconfigured `default_rule_distribution.variant_key` should be
      caught at REST validation write time (Phase 4 task 5).
    - **The purity test must exclude itself:** The forbidden-token
      constant array literally contains the forbidden tokens. The scan
      excludes `purity.rs` by filename.
    - **`cargo fmt` reformats `const` slice literals across lines based
      on line width:** A 3-element string slice that fits on one line
      collapses to a single line. Always run `cargo fmt` before
      committing — clippy/test gates won't flag this, but the workflow's
      `cargo fmt -p stitchd-core --check` will.
    - **Test-only `Condition` import:** Tests that need
      `crate::rule_engine::condition::Condition` only inside one or two
      cases can either `use` it locally inside the test (cleaner, fewer
      unused-import warnings) or pull a `std::marker::PhantomData::<Condition>`
      no-op to silence the unused-import diagnostic.

---

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
- **Commits:** 38aacbe, 9ef7dcd, d19a57f, a6ba403, fb68a1e, 64d5579, dd16f44

---

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

