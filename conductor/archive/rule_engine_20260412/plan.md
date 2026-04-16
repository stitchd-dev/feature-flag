# Plan: Rule Engine

## Phase 1: Core Types & Error Model
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task: Define `RuleEngineError` enum
  - Variants: `TypeMismatch { param, expected, actual }`, `MissingParameter { param }`,
    `MissingContext { context_type }`, `CyclicFlagDependency { involved: Vec<FlagId> }`,
    `InvalidWeights`, `EmptyPercentageTargets`
  - Implement `thiserror::Error`
  - Unit tests: each variant formats correctly
  <!-- files: crates/stitchd-core/src/rule_engine/error.rs -->

- [x] Task: Define `Condition` leaf enum
  - Variants for equality/inequality, numeric (`Lt`, `Lte`, `Gt`, `Gte`),
    string (`Contains`, `StartsWith`, `EndsWith`), SemVer (`SemverGte`,
    `SemverTilde`, `SemverCaret`), `InSegment`, `NotInSegment`,
    `FlagEvaluatedAs`
  - All variants carry `context_type: String` where applicable
  - Derives: `Debug`, `Clone`, `PartialEq`, `Serialize`, `Deserialize`
  <!-- files: crates/stitchd-core/src/rule_engine/condition.rs -->

- [x] Task: Define `ConditionExpr`, `Rule`, `RuleOutput`, `PercentageTarget`,
    `TargetField`, `EvaluationInput` types
  - `ConditionExpr`: `Leaf(Condition)`, `And(Vec<ConditionExpr>)`,
    `Or(Vec<ConditionExpr>)`, `Not(Box<ConditionExpr>)`
  - `RuleOutput`: `Variant(VariantId)`, `Percentage { targets, weights }`
  - `EvaluationInput<'a>`: borrows `contexts: &'a [Context]`,
    owns `resolved_segments: HashSet<SegmentId>`,
    `evaluated_flags: HashMap<FlagId, VariantId>`
  - All types derive `Debug`, `Clone`, `Serialize`, `Deserialize`
  <!-- files: crates/stitchd-core/src/rule_engine/types.rs -->

- [x] Task: Conductor — User Manual Verification 'Core Types & Error Model'
    (Protocol in workflow.md)

## Phase 2: Leaf Condition Evaluation
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task: Equality, inequality, and numeric comparison evaluators
  - `evaluate_leaf(cond: &Condition, input: &EvaluationInput)
    -> Result<bool, RuleEngineError>`
  - Handle `Eq`, `Ne`, `Lt`, `Lte`, `Gt`, `Gte` against `Int`, `Double`, `SemVer`
  - Return `TypeMismatch` on wrong `ParameterValue` variant
  - Return `MissingParameter` / `MissingContext` when lookup fails
  - Unit tests: all type-match cases, all mismatch cases, missing param/context
  <!-- files: crates/stitchd-core/src/rule_engine/eval_leaf.rs -->

- [x] Task: String operator evaluators
  - `Contains`, `StartsWith`, `EndsWith` against `ParameterValue::Str`
  - `TypeMismatch` on non-Str values
  - Unit tests: match, no-match, type mismatch for each operator
  <!-- files: crates/stitchd-core/src/rule_engine/eval_leaf.rs -->
  <!-- depends: task1 -->

- [x] Task: SemVer comparison evaluators
  - `SemverGte`, `SemverTilde`, `SemverCaret` against `ParameterValue::SemVer`
  - Delegate to `semver::Version` for tilde/caret semantics
  - Unit tests: `1.2.3 >= 1.2.0`, `~1.2.3` matches `1.2.4` not `1.3.0`,
    `^1.2.3` matches `1.3.0` not `2.0.0`, type mismatch
  <!-- files: crates/stitchd-core/src/rule_engine/eval_leaf.rs -->

- [x] Task: Segment membership and cross-flag condition evaluators
  - `InSegment` / `NotInSegment` → `resolved_segments` lookup
  - `FlagEvaluatedAs` → `evaluated_flags` lookup
  - Unit tests: in-set, not-in-set, flag match, flag mismatch, flag absent
  <!-- files: crates/stitchd-core/src/rule_engine/eval_leaf.rs -->

- [x] Task: Conductor — User Manual Verification 'Leaf Condition Evaluation'
    (Protocol in workflow.md)

## Phase 3: Composite Expression & Rule List Evaluation
<!-- execution: sequential -->
<!-- depends: phase2 -->

- [x] Task: Recursive `ConditionExpr` evaluator
  - `evaluate_expr(expr: &ConditionExpr, input: &EvaluationInput)
    -> Result<bool, RuleEngineError>`
  - `And([])` → `true`; `Or([])` → `false`
  - `And` short-circuits on first `false`; `Or` on first `true`
  - Unit tests: `Not(And([...]))`, `Or([And([...]), Not(Leaf(...))])`,
    vacuous And/Or, short-circuit verification
  <!-- files: crates/stitchd-core/src/rule_engine/eval_expr.rs -->

- [x] Task: Rule list evaluator
  - `evaluate_rules(rules: &[Rule], input: &EvaluationInput)
    -> Result<Option<&RuleOutput>, RuleEngineError>`
  - First matching rule wins; `None` if no match
  - Unit tests: first-match-wins, no-match, error stops evaluation
  <!-- files: crates/stitchd-core/src/rule_engine/eval_rules.rs -->

- [x] Task: Conductor — User Manual Verification 'Composite Expression & Rule
    List Evaluation' (Protocol in workflow.md)

## Phase 4: Percentage Allocation
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [x] Task: `PercentageTarget` value resolution
  - Resolve each target to `String` from `EvaluationInput`
  - `TargetField::Key` → `context.key`; `TargetField::Parameter(name)` →
    `ParameterValue` stringified
  - Return `MissingContext` / `MissingParameter` on lookup failure
  - Unit tests: key resolution, param resolution, missing context/param
  <!-- files: crates/stitchd-core/src/rule_engine/percentage.rs -->

- [x] Task: SipHash-1-3 bucketing and weight range assignment
  - Concatenate resolved values with `|`, append `flag_key`, `project_id`,
    `environment_id` as salt; hash with `siphasher::sip::SipHasher13`
  - bucket = `hash mod 1000`; walk cumulative weights to select variant
  - `InvalidWeights` if sum ≠ 1000; `EmptyPercentageTargets` if no targets
  - Unit tests: determinism, multi-target ordering, invalid weights, empty targets
  <!-- files: crates/stitchd-core/src/rule_engine/percentage.rs -->
  <!-- depends: task1 -->

- [x] Task: Conductor — User Manual Verification 'Percentage Allocation'
    (Protocol in workflow.md)

## Phase 5: Cross-Flag Dependency Resolution
<!-- execution: sequential -->
<!-- depends: phase3, phase4 -->

- [x] Task: Dependency graph extraction
  - `extract_flag_deps(rules: &[Rule]) -> HashSet<FlagId>`
  - Recursively walk `ConditionExpr` tree; collect all `FlagId`s from
    `FlagEvaluatedAs` leaves
  - Unit tests: single dep, multiple deps in nested expr, no deps
  <!-- files: crates/stitchd-core/src/rule_engine/dependency.rs -->

- [x] Task: Kahn's topological sort + cycle detection
  - `topological_sort(graph: &HashMap<FlagId, HashSet<FlagId>>)
    -> Result<Vec<FlagId>, RuleEngineError>`
  - Remaining nodes after BFS → `CyclicFlagDependency { involved }`
  - Unit tests: linear chain, diamond, disconnected, direct cycle, indirect cycle
  <!-- files: crates/stitchd-core/src/rule_engine/dependency.rs -->
  <!-- depends: task1 -->

- [x] Task: Multi-flag evaluation orchestrator
  - `evaluate_flags(flags: &[(FlagId, Vec<Rule>)], base_input: &EvaluationInput)
    -> Result<HashMap<FlagId, Option<VariantId>>, RuleEngineError>`
  - Build graph → topological sort → evaluate in order, accumulating results
  - Unit tests: cross-flag resolution, unresolved flag (None), cycle error
  <!-- files: crates/stitchd-core/src/rule_engine/orchestrator.rs -->
  <!-- depends: task1, task2 -->

- [x] Task: Conductor — User Manual Verification 'Cross-Flag Dependency
    Resolution' (Protocol in workflow.md)

## Phase 6: Integration & Wiring
<!-- execution: sequential -->
<!-- depends: phase5 -->

- [x] Task: Wire `rule_engine` module into `stitchd-core` `lib.rs`
  - `pub mod rule_engine;` with clean public re-exports:
    `Condition`, `ConditionExpr`, `Rule`, `RuleOutput`, `PercentageTarget`,
    `TargetField`, `EvaluationInput`, `RuleEngineError`,
    `evaluate_rules`, `evaluate_flags`
  <!-- files: crates/stitchd-core/src/lib.rs,
    crates/stitchd-core/src/rule_engine/mod.rs -->

- [x] Task: Full coverage and quality gate
  - `cargo test -p stitchd-core` — all tests pass
  - `cargo tarpaulin -p stitchd-core` — ≥90% coverage on `rule_engine` modules
  - `cargo clippy -p stitchd-core -- -D warnings` — clean
  - `cargo fmt --check` — clean

- [x] Task: Conductor — User Manual Verification 'Integration & Wiring'
    (Protocol in workflow.md)
