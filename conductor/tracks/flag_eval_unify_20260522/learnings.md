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

## [2026-05-22] Phase 4 — REST + gRPC rule-CRUD API refactor (worker P4)

- **Implemented:** Gateway REST DTOs and flag-service gRPC handlers
  carry the new `HashInputSpec` schema end-to-end. A new
  `HashSelectorJson` enum on the gateway mirrors core's `HashSelector`
  serde representation (`#[serde(tag = "kind", rename_all =
  "snake_case")]`); a free `validate_hash_inputs` function enforces FR-8
  rules (non-empty, no duplicates by `(context_type, field)` identity,
  non-empty `parameter` for `ContextParameter`). The validator runs in
  the gateway BEFORE the upstream gRPC call so bad payloads return 400
  even when the flag-service is unreachable. mapping.rs in flag-service
  dual-writes proto (`hash_inputs` authoritative + `context_hash_specs`
  synthesised for the backwards-compat window) and prefers `hash_inputs`
  on read, falling back to canonical `(context_type ASC, parameter
  ASC)` sort of the legacy map. A symmetrical
  `validate_proto_hash_inputs` in mapping.rs runs at the gRPC boundary so
  non-gateway clients hit the same rules. `set_default_rule_distribution`
  gained a variant-key referential check that reinstates the diagnostic
  Phase 2 dropped from core's purity-bound `evaluate_flag`.

- **Files changed:**
  - `crates/stitchd-gateway/src/routes/flags.rs` — `HashSelectorJson`,
    `validate_hash_inputs`, `extract_hash_inputs_from_allocation`,
    `synthesise_context_hash_specs`, new field on
    `DefaultRuleDistributionBody`, validation hooked into `update_rules`
    + `set_default_rule_distribution`, dual response shape
    (`hash_inputs` + `hash_targets`) in `flag_rule_to_json`.
  - `crates/stitchd-flag-service/src/mapping.rs` — dual-write proto,
    prefer-hash_inputs read, canonical-sort fallback, public
    `validate_proto_hash_inputs`.
  - `crates/stitchd-flag-service/src/error.rs` — `InvalidHashInputs`,
    `UnknownDefaultRuleVariant` variants; mapped to
    `Status::invalid_argument` with the sentinel prefixes
    `invalid_hash_inputs:` and `invalid_distribution:`.
  - `crates/stitchd-flag-service/src/service.rs` — `mutate_flag` Update
    rule-validation loop; `set_default_rule_distribution` variant-key
    check via `variant_repo.find_by_flag`.
  - `crates/stitchd-gateway/src/openapi.rs` — register
    `HashSelectorJson`, `RuleBody`, `ReplaceRulesBody`, `RuleJson`,
    `VariantBody`, `VariantJson`, `AdminFlagJson` in
    `components.schemas`.
  - `crates/xtask/README.md` — cargo-rdme refresh for Phase 3's
    `verify-hash-cutover` subcommand (Phase 3 left this stale).

- **Commits (chronological):**
  - `20490e6` test(gateway): failing tests for hash_inputs rule CRUD +
    default-rule dist
  - `53ca54a` feat(gateway): accept hash_inputs in rule CRUD DTOs
  - `1c6cb01` feat(flag-service): dual-write hash_inputs + server-side
    validation
  - `0c04f09` feat(gateway): register Phase 4 rule-CRUD DTOs in OpenAPI
  - `e63ad81` docs(xtask): refresh cargo-rdme block
  - `a7910b6` refactor(gateway,flag-service): collapse nested if-lets
    for clippy

- **Tests:** 17 new tests (11 gateway integration, 3 mapping, 3
  service); workspace lib total 1715 passed, 0 failed.

- **Learnings:**
  - **JSON wire shape — dual fields on responses, prefer-new on
    requests:** REST responses populate BOTH `hash_inputs` (new,
    authoritative) and `hash_targets` (legacy, kept for the existing
    admin UI). Requests accept either; `hash_inputs` wins when both are
    present. Spec FR-8 reads "removed" but the worker prompt's "alongside"
    interpretation matches the safest migration path — sibling worker P7
    will sweep the legacy field after the Admin UI migrates.
  - **Gateway validation runs BEFORE upstream RPC:** stubbed gRPC
    channels in unit tests connect lazily and fail at the first RPC
    attempt with `BAD_GATEWAY` (502). Putting validation in front of the
    RPC means 400 cleanly distinguishes "request rejected by validator"
    from "upstream unreachable". `assert_ne!(BAD_REQUEST)` is the
    happy-path assertion against the same stub.
  - **`HashSelectorJson::identity` uses `"__key__"` sentinel:**
    `(context_type, field)` duplicate-detection collapses
    `ContextKey { context_type }` and `ContextParameter { context_type,
    parameter }` into a single tuple by stuffing the reserved sentinel
    `"__key__"` for the key variant. A real parameter named `"__key__"`
    can never collide because `parameter` is validated non-empty
    separately and the sentinel is never serialised on the wire.
  - **Canonical sort of legacy `context_hash_specs`:** Both directions
    (proto→core for old data, and synthesis core→proto map for legacy
    consumers) use `context_type ASC, parameter ASC within type`. Hash
    stability across the cutover is preserved only when producers used
    that order — `cargo xtask verify-hash-cutover` (P3) prints the
    operator-review report.
  - **Proto `SetDefaultRuleDistributionRequest` doesn't carry
    `hash_inputs`:** P3 added a `default_rule_hash_inputs` column to
    `feature_flags` but did not extend the proto RPC. Phase 4 validates
    the new field in the gateway DTO but discards it at the wire
    boundary; full plumb-through lands in P5/P6.
  - **Variant-key referential validation lives in the gRPC handler, not
    core:** Phase 2 dropped `tracing::warn!` from `evaluate_flag` per
    NFR-3 (no logging side effects in core). Phase 4 reinstates the
    diagnostic by checking referential integrity at the
    `set_default_rule_distribution` boundary via
    `variant_repo.find_by_flag`. Returns a typed `FlagServiceError`
    variant that maps to `Status::invalid_argument` with the
    `invalid_distribution:` sentinel the gateway already converts to
    HTTP 400.
  - **Rust 1.95 `clippy::collapsible_if` covers `if let` too:** nested
    `if let Some(x) = … { if … { … } }` patterns must use the
    `let-chain` form `if let Some(x) = … && … { … }`. Three of our new
    diffs tripped this — fixed in `a7910b6`.
  - **`docs/src/api/openapi.json` is gitignored:** `cargo xtask docs`
    regenerates it on every run but the file never lands in git. The
    docs idempotency check looks for drift in TRACKED files only —
    crates/xtask/README.md is the canonical signal that Phase 3's
    xtask command wasn't `cargo-rdme`-refreshed. Worth catching in CI.
---

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

## [2026-05-22] Phase 6 — SDK rewire + multi-context EvalRequest + API consolidation (Worker P6)

- **Implemented:** Rewired `sdks/rust/src/client.rs::SdkClient::evaluate` to
  delegate orchestration entirely to
  `stitchd_core::evaluation::evaluate_flag`. Deleted the duplicate
  `evaluate_inner` (~120 lines) + `resolve_segments` (~80 lines) inline
  orchestration. Added proto -> core flag conversion in the SDK
  (`convert_proto_flag_to_core`, `convert_proto_flag_rule_to_core`,
  `proto_allocation_to_core`, `proto_to_core_hash_input_spec`,
  `proto_variant_value_to_core`, `core_variant_value_to_json`,
  `proto_value_type_to_core`, `parse_env_id`). The conversion prefers the
  Phase 3 `PercentageAllocation.hash_inputs` selector list and falls
  back to canonical-sorted legacy `context_hash_specs` for back-compat.
- **Public-API breaking changes** (SDK is pre-1.0):
  - `EvalRequest::context: Context` → `EvalRequest::contexts: Vec<Context>`
    + new `EvalRequest::single(flag_key, ctx)` ergonomic constructor for
    the common single-context case.
  - `evaluate(&[EvalRequest]) -> Vec<EvalResult>` + `evaluate_with_reasoning(...)`
    collapsed into ONE method:
    `evaluate(&[EvalRequest], TraceLevel) -> Vec<EvalResult>`. The thin
    vs rich split is gated by `TraceLevel`.
  - `EvalResult` gains `Option<EvaluationTrace>` (populated only at
    `TraceLevel::Full`) + `context_index: usize` so callers can correlate
    results back to the bundle position.
  - `EvalResult` and `EvalOutcome` gain `DefaultRuleDistribution`
    variant — the SDK now surfaces the core engine's distributed-variant
    outcome (inert until the proto SDK wire carries the field; gated by
    a future tracking ticket).
  - `EvalResultWithReasoning` + `ReasoningTrace` types DELETED — callers
    use `EvalResult.trace` directly.
  - Re-exports: `stitchd_sdk_rust::{EvaluationTrace, TraceLevel}` from
    `stitchd_core::evaluation` so callers don't depend on core for the
    one-method API.
- **Files changed:** `sdks/rust/src/client.rs`,
  `sdks/rust/src/lib.rs`, `sdks/rust/examples/live_verify.rs`,
  `sdks/rust/tests/conformance.rs`, `sdks/rust/tests/parity_with_preview.rs` (NEW),
  `sdks/rust/README.md`.
- **Commits:** c8068a3 (main rewire + struct change + cross-context test),
  e46c732 (parity test), 2002276 (README + Task 8 dedicated test).
- **Tests added:**
  - `client::tests::eval_request_accepts_multi_context_bundle` (Task 1)
  - `client::tests::evaluate_full_trace_includes_evaluation_trace` (Task 2)
  - `client::tests::evaluate_full_trace_for_flag_not_found_has_no_trace` (Task 2)
  - `client::tests::evaluate_cross_context_hash_selectors_match_core` (Task 4)
  - `parity_with_preview::parity_cross_context_percentage_hash_matches_core` (Task 3)
  - `parity_with_preview::parity_default_rule_distribution_via_core_engine` (Task 3 + 8)
  - `parity_with_preview::default_rule_distribution_assigns_listed_variant_not_fallback` (Task 8)
- **Test totals:** 139 lib + 8 conformance + 3 parity + 1 doc = 151 SDK
  tests, all green. Clippy --all-targets --features test-util -D warnings
  clean. cargo fmt --all --check clean. Workspace build clean.
- **Learnings:**
  - Patterns:
    - **Proto -> core conversion ON the evaluation hot path:** Acceptable
      cost — `FeatureFlag` is small (variants + rules), and the
      `rule_payload` JSON deserialisation was happening on the old path
      anyway. A future SDK refresh can pre-convert at snapshot-load
      time; for Phase 6 the per-evaluation conversion keeps the
      `DefinitionSnapshot` proto shape unchanged.
    - **`evaluate_flag` returns one result per context** in the input
      bundle. The SDK propagates this by emitting one `EvalResult` per
      `(request, context_index)` pair. Single-context bundles produce a
      single result — the common case stays terse via
      `EvalRequest::single`.
    - **`ListMembershipIndex` assembly per request:** for each context
      in `req.contexts`, look up `(type, key)` in the SDK's existing
      `MembershipCache` (LRU); on miss, batch-fetch via the existing
      `MembershipBatchFetcher` and write back to the LRU. The aggregated
      index is then fed into `evaluate_flag` which folds it into its
      per-context segment-resolution loop.
    - **`hash_inputs` (Phase 3 new field) preferred over the legacy
      `context_hash_specs` map** when both are present in proto. Legacy-
      only path uses canonical sort (`context_type ASC, parameter ASC
      within type`) matching the PG migration backfill — preserves
      bucket-identical behaviour for legacy rules during dual-schema
      state. New code authoring `hash_inputs` preserves selector order.
  - Gotchas:
    - **The proto SDK service does NOT yet carry
      `default_rule_distribution`** on `FeatureFlag` (proto is sealed
      for Phase 6 scope per the orchestrator prompt). The SDK proto ->
      core conversion sets `record.default_rule_distribution = None`,
      so the SDK gains "automatic" support via core ONCE the proto wire
      ships the field (separate track). Task 8 verification works
      around this by constructing a core `Flag` with the distribution
      directly and asserting `evaluate_flag` returns the distributed
      variant + `EvalOutcome::DefaultRuleDistribution`. The contract
      the SDK inherits is verified; the SDK's own conversion picks up
      the field automatically when proto is updated.
    - **`RolloutDistribution` has no `::new()` constructor** — use the
      struct literal `RolloutDistribution { allocations: vec![...] }`
      directly. The `validate()` method exists for runtime validation
      but isn't a constructor.
    - **`RuleOutput::Percentage` still carries `Vec<PercentageTarget>`**
      (the legacy shape) — the core engine has a bridge
      (`hash_input_spec_from_targets`) that converts to `HashInputSpec`
      internally during evaluation. The SDK uses the reverse bridge
      (`hash_input_spec_to_targets`) when converting proto allocations.
      Both bridges go away when Phase 5/6 of the broader cutover (separate
      flag-service work, not SDK scope) rewires `RuleOutput::Percentage`
      to carry `HashInputSpec` directly.
    - **`FlagNotFound` is an SDK-only outcome.** The core engine never
      sees a missing flag — the SDK short-circuits before calling
      `evaluate_flag`. Consequently, an `EvalResult` with
      `outcome == FlagNotFound` always has `trace == None` even at
      `TraceLevel::Full`. Documented inline.
    - **Clippy `useless_vec` fires on `let bundles = vec![v1, v2]`**
      where the binding is iterated by ref — use an array literal
      `let bundles: [Vec<_>; 2] = [v1, v2]` instead (the inner Vec is
      still a Vec, but the outer collection can be an array).
    - **rustfmt re-orders `pub use` blocks alphabetically** when the
      first re-export changes. Don't fight it; the `pub use stitchd_core::...`
      line ended up at the bottom after fmt because of casing rules.
  - Context:
    - The `parity_with_preview.rs` integration test lives in
      `sdks/rust/tests/` (not `crates/stitchd-flag-service/tests/`) because
      Worker P6's scope is the SDK crate and the parity check needs both
      the SDK client + the core engine. Adding it to flag-service tests
      would create a cross-worker dependency P6 doesn't own.
    - The `--features test-util` flag is REQUIRED for both the lib + the
      integration tests (the `parity_with_preview.rs` uses
      `client::testing::sdk_client_with_snapshot_and_lru` to inject
      stubs). Run via
      `cargo test -p stitchd-sdk-rust --features test-util` to exercise
      every test.

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

