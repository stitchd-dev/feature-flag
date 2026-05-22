# Spec: Unify Feature-Flag Variant Evaluation Across Service Preview and SDK

**Track ID:** flag_eval_unify_20260522
**Type:** Refactor + targeted UI follow-through
**Date:** 2026-05-22

## Overview

Today, the orchestration of feature-flag variant evaluation — rule iteration with
first-match semantics, percentage-allocation bucketing, default-rule fallthrough,
and trace generation — is implemented twice:

1. **`stitchd-core::evaluation::preview::evaluate_preview()`** (used by the flag
   service's `POST /flags/{key}/evaluate-preview` endpoint that powers the Admin
   UI "Test" panel), and
2. **`SdkClient::evaluate_inner()`** inline in `sdks/rust/src/client.rs` (used
   by the server-side Rust SDK's `evaluate()` / `evaluate_with_reasoning()`).

Both paths re-use shared primitives in `stitchd-core` (`calculate_allocation`,
`evaluate_expr`, `SegmentEvaluator`, `EvaluationInput`), but the top-level
orchestration is duplicated. This has already produced behavioural drift along
two axes:

- **Feature drift:** Preview supports `default_rule_distribution` (Phase 2
  hash-based fallthrough); the SDK does not.
- **Schema drift in percentage hashing:** Preview uses an ordered list of
  `PercentageTarget { context_type, field }` and can hash key/parameter values
  drawn from *any* context in the request bundle. The SDK uses a different
  shape (`ContextHashSpec` keyed by `context_type` in a map) AND evaluates one
  context at a time, making cross-context hashing structurally impossible
  from SDK consumers regardless of how the rule is configured.

Without consolidation, every future change to variant evaluation must be
implemented and tested twice, with subtle skew risk. This track collapses the
duplicate orchestration into a single `stitchd-core` entry point, unifies the
percentage-hash input schema end-to-end (Admin UI rule builder → REST API →
proto → PG → core → SDK), and brings the SDK's input shape in line so that
cross-context hashing works from authoring UI to in-process evaluation.

## Functional Requirements

### FR-1: Single core evaluation entry point

`stitchd-core` exposes a single function (working name: `evaluate_flag`) with
this signature shape:

```rust
pub fn evaluate_flag(
    flag: &Flag,                                        // variants + rules + default_rule_distribution
    contexts: &[Context],                               // bundle: all contexts available for hashing
    rule_based_segments: &[RuleBasedSegmentDefinition], // configs for any rule-based segment referenced by the flag
    list_segment_memberships: &ListMembershipIndex,     // per-context "which list segments contain me"
    environment_id: EnvironmentId,
    project_id: ProjectId,
    trace: TraceLevel,                                  // Off | Full
) -> FlagEvaluationResult;
```

- The function is **pure** — no I/O, no logging, no clock.
- `FlagEvaluationResult` carries: the matched `EvaluatedVariant` + `EvalOutcome`
  plus, when `trace == Full`, a `Vec<RuleTrace>`, `Vec<ConditionTrace>` (OR/AND
  missing-context resolution), and `Option<RolloutDebug>` (hash_input, bucket,
  variant_ranges).
- Rule iteration uses first-match semantics; rule-based segment membership is
  evaluated **inside core** from the provided segment definitions; list-segment
  membership is **looked up** from the caller-supplied index.
- Default-rule distribution (Phase 2 hash-based fallthrough) is supported
  uniformly for every caller.

### FR-2: Unified percentage-hash input schema

The two divergent representations (`PercentageTarget` in core, `ContextHashSpec`
in proto/SDK) collapse into one canonical shape in `stitchd-core`:

```rust
pub struct HashInputSpec {
    /// Ordered list — order is significant for hash stability.
    pub selectors: Vec<HashSelector>,
}

pub enum HashSelector {
    /// Hash the `key` of the named context_type, if present in the bundle.
    ContextKey { context_type: String },
    /// Hash a specific parameter of the named context_type, if present.
    ContextParameter { context_type: String, parameter: String },
}
```

This schema supports the user-facing requirement:

> Percentage hashing supports parameters as well as keys, within or across
> contexts — e.g. a combination of `user.key`, `user.params.name`,
> `device.key`, `device.params.os`, `application.key`.

Selectors that resolve to a missing context or missing parameter contribute a
deterministic empty-string sentinel (current preview behaviour). The proto
`PercentageAllocation` message migrates to carry `repeated HashSelector
hash_inputs` instead of the current context-type-keyed map.

### FR-3: Flag service rewires preview to the unified entry point

`stitchd-flag-service::evaluate_preview` keeps its current I/O behaviour
(fetching flag, variants, rules, rule-based segment defs, and list memberships
from PG + Scylla), then assembles the inputs and calls `evaluate_flag` with
`trace = Full`. The endpoint's response shape and field semantics (rule traces,
rollout debug, OR/AND missing-context details, variant ranges) MUST remain
byte-equivalent to today's output for the same input. Existing
evaluate-preview integration tests pass unchanged.

### FR-4: Rust SDK rewires evaluation to the unified entry point

`stitchd-sdk-rust`'s `evaluate_inner` is deleted. The SDK becomes a thin shim
that:

1. Reads the cached `DefinitionSnapshot` (unchanged).
2. Resolves list-segment memberships via the existing `MembershipCache` + REST
   batch fetch (unchanged).
3. Assembles inputs and calls `evaluate_flag`.
4. Translates the result back to its public types and emits the
   `FlagEvaluationEvent` per evaluation (unchanged).

### FR-5: SDK public-API consolidation + multi-context input

The two existing public methods (`SdkClient::evaluate` and
`SdkClient::evaluate_with_reasoning`) collapse into a single method, and the
per-request input shape grows from one context to a context bundle:

```rust
pub struct EvalRequest {
    pub flag_key: String,
    pub contexts: Vec<Context>,   // was: pub context: Context
}

pub async fn evaluate(
    &self,
    requests: &[EvalRequest],
    trace: TraceLevel,
) -> Vec<EvalResult>;
```

`EvalResult` carries the variant + outcome plus an `Option<EvaluationTrace>`
populated only when `trace == TraceLevel::Full`. The SDK is pre-1.0; this is
an intentional, documented breaking change. Internal callers (Rust SDK
integration tests, in-tree examples) update accordingly.

### FR-6: Closing the default-rule-distribution gap

Because every caller now goes through `evaluate_flag`, the SDK gains
`default_rule_distribution` support automatically. Parity tests assert that
preview and SDK return identical variants for a flag with
`default_rule_distribution` set, across a representative bucket-sweep of
context keys.

### FR-7: Storage + proto migration of existing percentage rules

Existing `PercentageAllocation` rule payloads in PG (`feature_flag_rules`,
`feature_flags.default_rule_distribution`) are migrated to the new
`hash_inputs: Vec<HashSelector>` shape:

- A forward SQL migration converts each existing `context_hash_specs` map into
  an ordered selector list using an explicit canonical sort (by
  `context_type` ASC, then parameters ASC within each context_type) to
  preserve the existing hash bucket for each in-flight flag where the prior
  map iteration order matched that canonical sort.
- The proto schema gets the new `repeated HashSelector` field; the old
  `context_hash_specs` map is dropped (no backwards-compat shim — pre-1.0
  SDK, internal-only gRPC channel between gateway and flag-service).
- A pre-flight script in `xtask` (or one-shot binary) re-hashes a sample of
  contexts against pre-migration and post-migration configs and reports
  bucket-identical results vs. operator-review-required rules.

### FR-8: REST + gRPC rule-CRUD API refactor

The rule create/update API surface — `POST /v1/flags/{id}/rules`,
`PUT /v1/flags/{id}/rules/{rule_id}`, and the matching gRPC RPCs in
`stitchd-flag-service` — refactors to carry the new schema:

- Request/response DTOs accept `hash_inputs: HashInputSpec` (an ordered list
  of `HashSelector` variants) on any `percentage` rule output and on
  `default_rule_distribution`.
- The old `context_hash_specs` map field is removed from the JSON envelope.
  Pre-1.0; no backwards-compat shim.
- OpenAPI annotations (`#[utoipa::path]` + schema derives) regenerate; the
  contract-check job (`scripts/check_openapi_contract.py`) passes.
- Server-side validation enforces:
  - `selectors` is non-empty;
  - no two selectors are exact duplicates (same context_type + same field);
  - selector order is preserved end-to-end (no implicit re-sort post-validation).

### FR-9: Admin UI rule builder — cross-context selector UX

The Admin UI rule builder (in `admin/src/components/flag/...`) gains a
multi-selector control for percentage-allocation rule outputs and for the
default-rule distribution:

- A draggable / ordered list of selector rows. Each row has:
  - A context-type picker (sourced from the Context Intelligence registry —
    `GET /v1/environments/{env_id}/context-types`),
  - A field picker: `Key` or `Parameter`,
  - If `Parameter`, a parameter-name input with autocomplete from
    `GET /v1/environments/{env_id}/context-types/{ct}/params`.
- The order is user-controlled and reflected in the persisted JSON. Reordering
  is exposed as drag handles + keyboard reorder for accessibility.
- A small helper banner shows a worked example for the configured selector
  list (e.g. "Bucket = hash(user.key || user.params.name || device.params.os)")
  so authors can verify their intent without leaving the form.
- Formik field shape + Yup schema (in `admin/src/lib/validation/`) align with
  FR-8: required non-empty array, unique selectors, parameter required when
  field == `Parameter`.
- The Test panel (evaluate-preview) keeps working unchanged for the new shape
  because the backend response is unchanged (rule trace + rollout debug still
  surface the resolved hash_input string).

## Non-Functional Requirements

### NFR-1: Behavioural parity

A property-based / table-driven parity-test suite asserts that, for a corpus
of flags + contexts + rule-based segments + list memberships, the service-
preview path and the SDK path return **identical** variants and outcomes.
When trace is `Full`, the rule-trace, condition-trace, and rollout-debug
payloads are field-by-field equal. The corpus includes at least one flag
whose percentage allocation hashes across multiple context types (e.g.
user + device + application, mixing key and parameter selectors).

### NFR-2: Hot-path zero-overhead trace

When `trace == TraceLevel::Off`, `evaluate_flag` MUST NOT allocate trace,
condition-trace, or rollout-debug structures.

### NFR-3: Core stays pure

`evaluate_flag` and everything it calls in core has no `async`, no
`std::time`, no logging side-effects, and no external dependency calls.

### NFR-4: Hash stability across the cutover

For every existing rule whose canonical sort matches its previous insertion
order, the post-migration bucket assignment for any given context bundle
MUST equal the pre-migration assignment. Enforced by a dedicated migration
test that diffs hashes on a frozen corpus.

### NFR-5: UI test coverage

The new rule-builder selector control has Vitest + React Testing Library
coverage for: add/remove/reorder selectors, parameter autocomplete, Yup
validation messages, round-tripping a saved rule through the form.

### NFR-6: Coverage gates

≥90% per-crate coverage maintained (tarpaulin). Admin UI continues to ship
without a coverage gate but new component code carries unit tests.

## Acceptance Criteria

- [ ] Single `stitchd-core::evaluation::evaluate_flag` function exists and is
      the sole orchestrator of rule iteration + percentage allocation +
      default-rule fallthrough.
- [ ] `stitchd-core::evaluation::preview` module either becomes a thin wrapper
      over `evaluate_flag(trace=Full)` or is deleted entirely; no duplicate
      rule-iteration loop survives.
- [ ] `HashInputSpec` is the single canonical schema for percentage hashing
      inputs in core, proto, PG, REST, and SDK. `PercentageTarget` and
      `ContextHashSpec` are removed.
- [ ] `stitchd-flag-service::evaluate_preview` endpoint produces byte-
      equivalent responses to its pre-track baseline for existing tests.
- [ ] `SdkClient::evaluate_inner` is deleted; SDK runs through `evaluate_flag`.
- [ ] SDK exposes one public `evaluate(&[EvalRequest], TraceLevel)` method;
      `evaluate_with_reasoning` is removed; `EvalRequest.contexts: Vec<Context>`
      replaces single-context; SDK README + crate docs updated.
- [ ] SDK supports `default_rule_distribution`; parity test passes.
- [ ] Cross-context percentage hashing (user.key + user.params.name +
      device.key + device.params.os + application.key) is exercised end-to-end
      from UI authoring → REST → PG → preview AND from snapshot → SDK; both
      produce identical buckets.
- [ ] PG migration converts existing rules to the new schema; hash-stability
      migration test passes.
- [ ] REST + gRPC rule-CRUD APIs accept and emit the new `hash_inputs` shape;
      OpenAPI contract-check job passes.
- [ ] Admin UI rule builder supports ordered multi-context selectors with the
      UX described in FR-9; new Vitest coverage in place.
- [ ] Existing flag-rule integration + e2e tests pass with new shape.
- [ ] All workspace tests + clippy + fmt pass; tarpaulin ≥90% per crate.
- [ ] Docs idempotency check (`cargo xtask docs && git diff --exit-code`) clean.

## Out of Scope

- **SDK fetching layer** — definition snapshot polling, gRPC sync,
  `MembershipCache`, REST batch list-segment fetch. All stay as today.
- **Eval-log emission** — the SDK continues to emit `FlagEvaluationEvent` per
  evaluation; preview continues to skip eval-log writes.
- **Experiment-assignment routing** — `experiment_assignments_mv`,
  `experiment_iterations_active` dictionary, and the ITT first-exposure
  pipeline are completely untouched.
- **Service-side `resolve_list_memberships()` Scylla batch lookup** — the
  prefetch from Scylla stays as-is.
- **New flag, rule, or segment features** — no new rule operators, no new
  variant types, no new bucketing strategies. Pure refactor + close two known
  gaps (default-rule distribution in SDK, cross-context hashing end-to-end).
- **Hash algorithm change** — `calculate_allocation` (Murmur3 → 0–100.0 →
  bucket 0–999) stays exactly as today. Only the *input shape*, the
  *orchestration*, and the *authoring UX* change.
- **Non-Rust SDKs** — no other-language SDK exists yet.
- **Context Intelligence registry changes** — the autocomplete sources
  (`/v1/environments/{env_id}/context-types/...`) are consumed as-is; no new
  endpoints or column additions to the registry tables.
- **Backwards-compatible API shim** — the rule-CRUD JSON contract changes
  cleanly with no deprecation period. SDK and Admin UI are in-tree and
  migrate together.
