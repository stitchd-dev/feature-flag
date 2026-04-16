# Track Learnings: rule_engine_20260412

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### Code Conventions
- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml` (not `"2"`).
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment.
- `rustfmt.toml` options like `imports_granularity`, `group_imports` are nightly-only — strip from stable configs.

### Architecture
- Prometheus metrics: use `PrometheusBuilder::new().install_recorder()` for handle-based rendering.
- Graceful shutdown: `tokio::select!` over `ctrl_c()` + `SIGTERM`.

### Gotchas
- OpenTelemetry version alignment: pin all OTel crates to the same minor version to avoid incompatible types.
- `opentelemetry_sdk 0.28`: `Resource::new()` is private — use `Resource::builder()`.

### Testing
- Axum router integration tests: use `tower::ServiceExt::oneshot` without a real TCP server.

---

<!-- Learnings from implementation will be appended below -->

## [2026-04-12 05:56] - Phase 1 Tasks 1-3: Core Types & Error Model
- **Implemented:** RuleEngineError (6 variants), Condition leaf enum (14 variants), ConditionExpr/Rule/RuleOutput/PercentageTarget/TargetField/EvaluationInput types
- **Files changed:** rule_engine/error.rs, condition.rs, types.rs, mod.rs, lib.rs
- **Commit:** 7b5c35c
- **Learnings:**
  - Patterns: `EvaluationInput` borrows `&'a [Context]` — no cloning at eval time, matches spec precisely
  - Gotchas: `ConditionExpr` must be `Box<ConditionExpr>` for `Not` to avoid infinite-size type
  - Context: `find_context()` helper on `EvaluationInput` makes all evaluators cleaner
---

## [2026-04-12 05:57] - Phase 2 Tasks 1-4: Leaf Condition Evaluation
- **Implemented:** evaluate_leaf() dispatching all 14 Condition variants; helpers lookup_param/lookup_str/lookup_semver/numeric_cmp
- **Files changed:** rule_engine/eval_leaf.rs
- **Commit:** e840983
- **Learnings:**
  - Patterns: `VersionReq::parse(&format!(">{version}"))` / `~{version}` / `^{version}` correctly maps to semver crate tilde/caret semantics
  - Patterns: `numeric_cmp` closure over `std::cmp::Ordering` avoids duplicating 4 nearly-identical match arms
  - Gotchas: `param_value_type_name` must return `&'static str` for `TypeMismatch` — the `actual` field is `&'static str`
---

## [2026-04-12 05:58] - Phase 3 Tasks 1-2: Composite Expression & Rule List Evaluation
- **Implemented:** evaluate_expr() with short-circuit And/Or, vacuous empty semantics; evaluate_rules() returning first-match &RuleOutput
- **Files changed:** rule_engine/eval_expr.rs, eval_rules.rs
- **Commit:** ea4975c
- **Learnings:**
  - Patterns: Recursive pattern match on ConditionExpr is idiomatic and stack-safe for practical depths
  - Patterns: `And(children) | Or(children)` in one match arm handles both with the same binding
---

## [2026-04-12 05:59] - Phase 4 Tasks 1-2: Percentage Allocation
- **Implemented:** resolve_targets(), allocate_percentage() with SipHash-1-3; siphasher added to workspace
- **Files changed:** rule_engine/percentage.rs, Cargo.toml, stitchd-core/Cargo.toml
- **Commit:** cccaa1e
- **Learnings:**
  - Patterns: `siphasher = { version = "1" }` — use `siphasher::sip::SipHasher13`; hash with `std::hash::Hash` + `Hasher::finish()`
  - Patterns: Hash input = `resolved_values.join("|") + "|" + flag_key + "|" + project_id + "|" + environment_id`
  - Gotchas: `siphasher` is not in the workspace by default — must add to both `Cargo.toml` workspace deps and crate deps
---

## [2026-04-12 06:00] - Phase 5 Tasks 1-3: Cross-Flag Dependency Resolution
- **Implemented:** extract_flag_deps(), topological_sort() (Kahn's), evaluate_flags() orchestrator
- **Files changed:** rule_engine/dependency.rs, orchestrator.rs
- **Commit:** 01b516a
- **Learnings:**
  - Patterns: Kahn's algorithm: build in_degree + reverse adjacency, seed queue with zero-in-degree nodes
  - Gotchas: Rust 2024 pattern matching — use `**d > 0` not `&d > 0` when iterating map refs; use `.keys()` not `(k, _)` for clippy::for_kv_map
  - Gotchas: `in_degree[node] = deps.len()` must reset (not increment) to avoid double-counting when re-deriving
  - Context: orchestrator skips flags in topo order that are transitive deps but not in the evaluation set
---

## [2026-04-12 06:01] - Phase 6: Integration, Wiring & Quality Gate
- **Implemented:** Public re-exports in mod.rs; 100% coverage via targeted tests for all branches
- **Files changed:** rule_engine/mod.rs, eval_leaf.rs (coverage tests), orchestrator.rs (coverage tests)
- **Commit:** 15bd8ae
- **Learnings:**
  - Patterns: cargo-tarpaulin `--include-files "crates/stitchd-core/src/rule_engine/*"` for scoped rule_engine coverage
  - Gotchas: `parse_semver_req` error path only reachable with invalid version string prefix — worth testing explicitly
  - Gotchas: Orchestrator `Percentage` arm is intentional/documented — returns `None` since `flag_key`/`project_id`/`environment_id` not available
---
