# stitchd-core

<!-- cargo-rdme start -->

`stitchd-core` — Pure domain library for the Stitchd feature-flag and experimentation
platform.

No I/O, no database, no network. All other workspace crates depend on this. Holds the
canonical types for feature flags, segments, contexts, evaluation, events, experiments
(including multi-armed bandit algorithms, posteriors, and convergence detection),
and the rule engine that combines them.

See the [Architecture] chapter of the mdBook for how this crate sits relative to the
`*-service` microservices and the gateway.

[Architecture]: ../architecture/README.html

<!-- cargo-rdme end -->

## Modules

| Module | Purpose |
|--------|---------|
| `flag` | `FeatureFlag`, `FlagKey`, `FlagType` — core flag domain types |
| `variants` | `Variant`, `VariantKey`, `VariantValue` (string, bool, int, float, json) |
| `segment` | `Segment`, `SegmentType` (rule-based vs list-based) |
| `rule_engine` | `ConditionExpr` tree: `And`, `Or`, `Not`, `Leaf`; evaluation against `EvaluationContext` |
| `context` | `EvaluationContext`, `Context`, `ParameterValue` — typed attribute map keyed by `(context_type, attribute_name)` |
| `evaluation` | Top-level `evaluate(flag, context)` — walks rules, resolves segments, returns matched variant |
| `tenant` | `Tenant`, `Organisation`, `Project`, `Environment`, `SdkKey` domain types |
| `user` | `User`, `Role`, `OrgMembership` |
| `auth` | JWT claims, token types |
| `experimentation` | `Experiment`, `ExperimentMetric`, `ExperimentResult` |
| `event` | `MetricEvent`, `EvaluationEvent` |
| `hashing` | Percentage-rollout hashing (consistent, deterministic) |
| `id` | Typed newtype IDs wrapping `Uuid` for all domain entities |

## Feature Flags

- `openapi` — derives `utoipa::ToSchema` on domain types for OpenAPI generation (used by the gateway)

## No Runtime Dependencies

`stitchd-core` depends only on `serde`, `uuid`, `chrono`, `thiserror`, `semver`, `smallvec`, and `async-trait`. It is safe to use in any context including tests and CLI tools.
