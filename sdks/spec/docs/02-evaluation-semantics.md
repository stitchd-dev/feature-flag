# 02 — Evaluation Semantics

This document specifies the **canonical evaluation algorithm** that every
Stitchd SDK must implement. The Rust reference implementation in `sdks/rust/`
delegates to `stitchd-core` internally, but other languages MUST re-implement
the algorithm in their own runtime — the wire-level definition snapshot is
self-contained (see `sdk_service.proto` and JSON schemas), and the algorithm
here is precise enough to implement from this document alone.

## The `Context`

Every evaluation requires a `Context`:

```
Context {
  type:                String,                  // e.g. "user", "device", "org"
  key:                 String,                  // unique identifier within this type
  parameters:          Map<String, ParamValue>, // typed attributes
  private_parameters:  List<String>,            // attribute names to redact from event payloads
}

ParamValue = Int(i64) | Double(f64) | String(s) | Bool(b) | Semver(major.minor.patch)
```

`(type, key)` is the LRU cache key for list-segment membership.

## Evaluation Entry Points

Every SDK MUST expose two evaluation operations:

```
evaluate(requests: [EvalRequest])                  -> [EvalResult]
evaluate_with_reasoning(requests: [EvalRequest])   -> [EvalResultWithReasoning]
```

Both accept a **batch** of `EvalRequest { flag_key, context }`. Each request in
the batch is evaluated independently against the local snapshot — no
cross-request state.

The non-reasoning variant returns minimal output (variant only). The reasoning
variant returns the variant **plus** a `ReasoningTrace` describing which rule
matched, which segment(s) were consulted, and how the rollout bucket was
computed.

## Definition Snapshot Shape

The snapshot returned by `SyncDefinitions` contains:

```
DefinitionsSnapshot {
  flags:           Map<flag_key, FlagDefinition>
  rule_segments:   Map<segment_id, RuleSegmentDefinition>     // full rule condition tree
  list_segments:   Map<segment_id, ListSegmentMetadata>       // segment_id + context_type + name (NO entries)
}

FlagDefinition {
  key, name, value_type ("bool"|"int"|"double"|"string"|"json"),
  enabled: bool,
  variants: [{ key, value }],
  default_rule: Rule,
  rules: [Rule],          // evaluated in order; first match wins
  project_id, environment_id, salt
}

Rule {
  rule_id, rule_name?,
  condition: ConditionTree,
  variant_assignment: SpecificVariant(key) | PercentageRollout([{variant_key, percentage}])
}
```

## Condition Tree Primitives

Every SDK must support these condition nodes (the wire format is JSON inside
`Rule.condition`; see `schemas/condition_tree.schema.json` for the formal
schema). Nodes:

| Node | Shape | Semantics |
|---|---|---|
| `Eq` | `{context_type, param, value}` | `context.parameters[param] == value` (after type coercion) |
| `Neq` | same | logical complement of `Eq` |
| `Lt`, `Lte`, `Gt`, `Gte` | `{context_type, param, value}` | numeric or semver comparison; non-numeric param → `false` |
| `Contains` | `{context_type, param, substr}` | `param` is a string AND contains `substr` |
| `StartsWith`, `EndsWith` | same | string prefix / suffix match |
| `SemverGte`, `SemverLt`, etc. | `{context_type, param, value}` | parse both sides as semver; non-semver → `false` |
| `InList` | `{context_type, param, values: [String]}` | `param` matches any value in `values` |
| `Matches` | `{context_type, param, pattern: String}` | wildcard match (`*` as wildcard); use `strip_suffix("*")` for simple prefix |
| `InSegment` | `{segment_id}` | true iff context is a member of the named segment (see *Segment Resolution* below) |
| `NotInSegment` | `{segment_id}` | logical complement of `InSegment` |
| `And` | `{children: [ConditionTree]}` | all children must be true; short-circuit on first `false`; **MissingContext / MissingParameter sub-result treated as `false`** |
| `Or` | `{children: [ConditionTree]}` | any child true; short-circuit on first `true`; MissingContext/MissingParameter treated as `false` |
| `Not` | `{child: ConditionTree}` | logical complement; MissingContext bubbles up unchanged |

`context_type` selector: every leaf condition is anchored to a specific context
type (e.g. `"user"`, `"device"`). If the caller's `Context.type` doesn't match,
the condition resolves to `MissingContext` — which `And`/`Or` treat as `false`
but `Not` preserves.

## Segment Resolution

When a condition tree contains `InSegment(segment_id)` or `NotInSegment(segment_id)`:

1. Look up the segment in the definition snapshot.
2. **If `rule_segments[segment_id]` exists** (rule-based segment): recursively
   evaluate the segment's condition tree against the same `Context`.
3. **If `list_segments[segment_id]` exists** (list-based segment): consult the
   LRU cache for `(context.type, context.key)`:
   - **Hit:** read `membership[segment_id]` (default `false` if not in map).
   - **Miss:** synchronously fetch via `POST /v1/sdk/segments/list:batch` with a
     single query containing `[{context_type, key, segment_ids: [segment_id]}]`,
     insert the response into the LRU, then read membership.
   - **Miss fetch fails:** treat as `false` for this evaluation. Log a warning.
     Do not insert anything into the LRU. The next evaluation for the same
     `(type, key)` will retry the fetch.
4. **If segment is unknown** (not in snapshot at all): treat as `false`. Log a
   warning. The most likely cause is a stale snapshot — the next definition
   poll should reconcile.

## Evaluation Algorithm

For each `(flag_key, context)` request:

1. Look up `flag_def` in the local definition snapshot.
   - **If missing:** return `EvalResult { variant: default_for_type(flag.value_type), outcome: "flag_not_found" }`. Still emit an event with `matched_rule_id = null` and `outcome = "flag_not_found"`.
2. **If `flag_def.enabled == false`:** return the disabled-state default variant.
   Do not iterate rules. Emit an event with `outcome = "disabled"`.
3. **Iterate `flag_def.rules` in order.** For each rule:
   - Evaluate `rule.condition` against the context (see Condition Tree Primitives + Segment Resolution above).
   - If the result is `true`: assign the variant using `rule.variant_assignment`. Stop iteration.
4. **If no rule matched:** apply `flag_def.default_rule.variant_assignment`.

### Variant Assignment

- `SpecificVariant(variant_id)`: return that variant's value.
- `PercentageRollout({targets: [PercentageTarget], weights: [(variant_id, weight)]})`:
  compute the bucket (see Hash Function below); walk weights in order,
  accumulating; the first weight whose cumulative sum is **strictly greater than**
  the bucket wins. Weights MUST sum to exactly `1000` (100.0% × 10 for 0.1%
  effective granularity). If weights do not cover the bucket: this is a
  misconfiguration; SDKs MUST surface it as an internal error.

### PercentageTarget Shape

A percentage rollout is parameterised by one or more targets that tell the
SDK **which context field(s)** to feed into the hash. The target is what binds
"the same user → the same bucket every time" semantics.

```
PercentageTarget {
  context_type: String              // which context to look up (e.g. "user", "org")
  field:        Key | Parameter(name: String)
}
```

- `Key`: use `Context.key` (the unique identifier within that context type).
- `Parameter(name)`: use the stringified value of `Context.parameters[name]`.

If a target requires a context type not present in the caller's `Context`,
the evaluation returns a `MissingContext` error for that request (treated as
`false` by enclosing `And`/`Or`; see Condition Tree Primitives).

## Hash Function (Percentage Rollout Bucket)

The bucket for a `(context, flag)` pair is computed deterministically. All SDKs
MUST produce the same bucket for the same inputs.

**Inputs (UTF-8, concatenated as plain strings — NO separator):**

```
flag.key                            // e.g. "checkout-flow"
flag.environment_id                 // UUID stringified (lowercase, hyphenated, no braces)
target_value_1                      // first PercentageTarget's resolved value (Context.key or stringified parameter)
target_value_2                      // additional targets, in order
...
```

Note: `project_id`, `salt`, `context.type`, and any per-flag separator are
**NOT** part of the hash. The context's contribution comes solely through the
resolved target values, in the order the rule lists them.

**Algorithm:** MurmurHash3 x64_128, seed `0`. Output is a single 128-bit
unsigned integer (`u128`). On any error producing the hash (which should be
unreachable for finite string input), use `0`.

**Reduction to bucket:**

```
percent_f64 = (u128_hash mod 100_000) as f64 / 1000.0    // range [0.0, 99.999]
bucket_u32  = min(999, floor(percent_f64 * 10.0))        // range [0, 999]
```

Effective granularity: 0.1% per bucket (1000 buckets across 100% range).

**Cross-language notes:**

- Languages without native `u128` (JavaScript, older Python) MUST use BigInt /
  arbitrary-precision integer for the modulo step. The result of `u128 mod
  100_000` always fits in a 32-bit integer; only the modulo itself needs
  big-integer math.
- The MurmurHash3 x64_128 implementation used by the Rust reference is the
  [`murmur3`](https://crates.io/crates/murmur3) crate v0.5 — its byte ordering
  convention for the 128-bit output is the standard "h2 high, h1 low" packing.
  Cross-language ports MUST match this convention to produce identical buckets.

**Reference vectors** (generated from `stitchd-core/src/hashing.rs::calculate_allocation`,
murmur3 crate v0.5.2):

| flag.key | env_id | targets (concatenated) | u128 hash | percent | bucket |
|---|---|---|---|---|---|
| `checkout-flow` | `00000000-0000-0000-0000-000000000001` | `alice` | `222181947253813895803534738316378551102` | 51.102 | 511 |
| `checkout-flow` | `00000000-0000-0000-0000-000000000001` | `bob` | `18952123904222406138070807736280323342` | 23.342 | 233 |
| `new-pricing`   | `11111111-1111-1111-1111-111111111111` | `user-42` | `168110605657145194934791374285431545175` | 45.175 | 451 |
| `homepage-redesign` | `00000000-0000-0000-0000-000000000001` | *(empty — no targets)* | `228078977241418722764046033579419205404` | 5.404 | 54 |
| `flag-X` | `env-Y` | `ctx-Z` | `34508178141733093739110042051873359827` | 59.827 | 598 |

These vectors are also encoded in `sdks/spec/fixtures/hashing/` (added in Phase 1
Task 6) as machine-readable JSON for any SDK's conformance suite to consume
directly.

## Reasoning Trace Shape

```
ReasoningTrace {
  outcome:               "matched" | "default_rule" | "disabled" | "flag_not_found"
  matched_rule:          { rule_id, rule_name, condition_summary } | null
  segment_evaluations:   [
    { segment_id, segment_type: "rule" | "list", matched: bool,
      source: "snapshot" | "lru_hit" | "lru_miss_fetched" | "lru_miss_failed" }
  ]
  rollout:               { bucket: int, allocation: [{variant_key, percentage}] } | null
  evaluated_at:          RFC3339_timestamp
}
```

The reasoning variant is **not free** — it adds bookkeeping on the hot path. Use
it for debugging, admin "test this flag" preview UIs, or selective sampling, not
for every production evaluation.

## Determinism Guarantees

- Same `(flag_def, context)` → same `variant`. **Always**, on every SDK instance
  in the world, until either the flag definition changes or the percentage
  rollout's salt changes.
- List-segment membership is the only piece of state that can drift: the LRU
  may serve stale membership between background refreshes (default staleness
  budget: 60s). This is intentional — see `03-caching.md`.

## Batch Semantics

A batch passed to `evaluate()` is evaluated in submission order. There is no
short-circuit on error: every request gets a result, and failures (unknown flag,
list-segment fetch error) are recorded in the result rather than thrown.

## Error Handling Per-Request

| Condition | Result | Event emitted? |
|---|---|---|
| Flag not found in snapshot | Default variant for `value_type` | Yes (`outcome = "flag_not_found"`) |
| List-segment fetch on miss fails | Treat referenced list-segments as `false`; continue rule iteration | Yes (`outcome = "matched"` or `"default_rule"`; reasoning's `segment_evaluations[].source = "lru_miss_failed"`) |
| Definition snapshot stale (gateway unreachable for > N intervals) | Continue serving last-known snapshot | Warn-logged; no eval-time error |
| Snapshot empty (only possible before first sync — but `init` blocks on that, so this should never happen) | Implementation-defined; SHOULD panic / throw a programming error | No event |
