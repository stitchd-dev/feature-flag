# Spec: Rule Engine

## Overview

Implement the core rule evaluation engine in `stitchd-core`. The engine evaluates
an ordered list of rules against an evaluation context, resolving to a variant via
direct assignment or hash-based percentage allocation. This is the foundational
evaluation layer that segmentation, flag evaluation, and experimentation all build on.
No I/O, no async — pure deterministic logic.

## Functional Requirements

### Leaf Conditions

A leaf condition tests a single value from a `Context`:

**Equality / Inequality**
- `Eq(context_type, param, value: ParameterValue)` — exact match
- `Ne(context_type, param, value: ParameterValue)` — not equal

**Numeric Comparisons** (Int, Double, SemVer)
- `Lt`, `Lte`, `Gt`, `Gte`

**String Operators** (Str only)
- `Contains(context_type, param, substr)`
- `StartsWith(context_type, param, prefix)`
- `EndsWith(context_type, param, suffix)`

**SemVer Comparisons** (SemVer only)
- `SemverGte` — `>=` compatible
- `SemverTilde` — `~` patch-compatible
- `SemverCaret` — `^` minor-compatible

**Segment Membership**
- `InSegment(segment_id: SegmentId)`
- `NotInSegment(segment_id: SegmentId)`

**Cross-Flag Conditions**
- `FlagEvaluatedAs { flag_id: FlagId, variant_id: VariantId }`

### Composite Condition Expression (Recursive)

Rules use a recursive `ConditionExpr` instead of a flat condition list. This
allows arbitrary nesting of AND, OR, and NOT combinators:

```rust
enum ConditionExpr {
    /// A single leaf test
    Leaf(Condition),
    /// All children must be true
    And(Vec<ConditionExpr>),
    /// At least one child must be true
    Or(Vec<ConditionExpr>),
    /// Inverts the inner expression
    Not(Box<ConditionExpr>),
}
```

**Examples:**

```
// Simple AND
And([Leaf(Eq("user", "plan", "pro")), Leaf(Gt("user", "age", 18))])

// NOT on top of AND
Not(And([Leaf(InSegment(s1)), Leaf(Eq("org", "tier", "free"))]))

// Nested: (A AND B) OR (NOT C)
Or([
  And([Leaf(Eq("user", "country", "US")), Leaf(Gte("device", "version", 5))]),
  Not(Leaf(FlagEvaluatedAs { flag_id: f1, variant_id: v2 }))
])
```

**Evaluation rules:**
- `And([])` → `true` (vacuously true)
- `Or([])` → `false` (vacuously false)
- `Not` wraps exactly one `ConditionExpr`
- Short-circuit: `And` stops at first false; `Or` stops at first true

### Rule Structure

```rust
struct Rule {
    id: RuleId,
    condition: ConditionExpr,   // arbitrary nesting of And/Or/Not/Leaf
    output: RuleOutput,
}

enum RuleOutput {
    Variant(VariantId),
    Percentage {
        targets: Vec<PercentageTarget>, // non-empty; fields to hash
        weights: Vec<(VariantId, u32)>, // tenths-of-a-percent; must sum to 1000
    },
}
```

### Evaluation Input

Multiple contexts may be provided — one per `context_type`. Conditions and
percentage targets resolve against the context whose `context_type` matches.

```rust
struct EvaluationInput<'a> {
    contexts: &'a [Context],               // no two with same context_type
    resolved_segments: HashSet<SegmentId>,
    evaluated_flags: HashMap<FlagId, VariantId>,
}
```

### Rule List Evaluation

- Evaluate `Vec<Rule>` in order; first rule whose `condition` evaluates to `true` → take output
- No match → `None` (caller applies flag default)
- Errors propagate immediately — no partial evaluation

### Percentage Allocation (Hash-Based Bucketing)

For `RuleOutput::Percentage { targets, weights }`:
- For each `PercentageTarget`, resolve value from the matching context
- Concatenate resolved values (UTF-8, `|`-separated) + `flag_key` + `project_id`
  + `environment_id` as stable salt
- Apply SipHash-1-3; bucket = `hash mod 1000`
- Assign to the variant whose cumulative weight range contains the bucket

```rust
struct PercentageTarget {
    context_type: String,
    field: TargetField,
}

enum TargetField {
    Key,                    // use Context::key
    Parameter(String),      // use Context::parameters[name]
}
```

**Examples:**
- Hash on user `key` only → `PercentageTarget { context_type: "user", field: Key }`
- Hash on org `account_tier` param → `PercentageTarget { context_type: "org", field: Parameter("account_tier") }`
- Hash on combination → `[user.key, org.account_tier]`

### Cross-Flag Dependency Resolution

When evaluating a set of flags together:
- Build a directed dependency graph: flag A → B if A's `ConditionExpr` contains
  `FlagEvaluatedAs { flag_id: B }`
- Topologically sort via Kahn's algorithm
- Cycle → `RuleEngineError::CyclicFlagDependency { involved: Vec<FlagId> }`
- Evaluate in topological order; accumulate into `HashMap<FlagId, Option<VariantId>>`

### Error Types

```rust
enum RuleEngineError {
    TypeMismatch { param: String, expected: &'static str, actual: &'static str },
    MissingParameter { param: String },
    MissingContext { context_type: String },
    CyclicFlagDependency { involved: Vec<FlagId> },
    InvalidWeights,
    EmptyPercentageTargets,
}
```

## Non-Functional Requirements

- All evaluation is pure (no I/O, no async)
- `ConditionExpr` recursion is unbounded but stack-safe for practical depths
- SemVer comparisons delegate to `semver::Version` semantics
- `EvaluationInput` borrows `contexts` — no cloning at evaluation time
- Short-circuit evaluation on `And`/`Or` nodes

## Acceptance Criteria

- [ ] `Not(And([...]))` correctly inverts a compound AND expression
- [ ] `Or([And([...]), Not(Leaf(...))])` evaluates with correct precedence
- [ ] `And([])` → true; `Or([])` → false
- [ ] Short-circuit: `And` stops at first false, `Or` stops at first true
- [ ] All leaf condition operators evaluate correctly
- [ ] Type mismatch → `TypeMismatch`, never panics
- [ ] Missing parameter → `MissingParameter`; missing context type → `MissingContext`
- [ ] SemVer `~` and `^` match `semver` crate semantics
- [ ] Percentage bucketing is deterministic for same inputs
- [ ] Multi-target bucketing hashes all specified fields in declaration order
- [ ] Weights ≠ 1000 → `InvalidWeights`; empty targets → `EmptyPercentageTargets`
- [ ] Topological sort evaluates flags in correct dependency order
- [ ] Cycles → `CyclicFlagDependency`
- [ ] `cargo test -p stitchd-core` ≥90% coverage on rule engine modules
- [ ] `cargo clippy -p stitchd-core -- -D warnings` passes clean

## Out of Scope

- Segment rule/list evaluation logic (segmentation track)
- REST or gRPC endpoints
- Persistence of rule definitions (DB schema already has `RuleId` from domain track)
- Event ingestion or experiment assignment
- Admin UI
