# Experimentation

The Stitchd experimentation surface delivers A/B and multivariate experimentation built on
**first-exposure (intent-to-treat), rule-scoped, server-derived attribution**. SDKs do not need
to know about experiments — assignment is computed from `flag_evaluation_log_v2` and joined to
events server-side.

This section covers the post-`experimentation_full_20260521` design. Earlier docs that describe
SDK-tagged event-context attribution (the `experiment` / `iteration` / `variant` tuples in the
events `contexts` column) are retired — the stats pipeline no longer reads those tuples.

## Topics

- [Attribution Model](./attribution.md) — eval-log → `experiment_assignments` first-exposure
  pipeline, ITT semantics, rule scoping, context-type scoping.
- [Default-Rule Experiments](./default-rule-experiments.md) — how to run an experiment on a
  flag's default-rule (fallthrough) path; the XOR rule-vs-default-rule binding; whole-flag
  lock interaction with the default-rule distribution.
- [Sequential Testing](./sequential-testing.md) — always-valid inference (mSPRT always-valid
  p-values + confidence sequences) so experiments can be peeked safely; opt-in per experiment.
- [Multi-Armed Bandit](./multi-armed-bandit.md) — adaptive traffic allocation (Thompson / ε-greedy
  / UCB / contextual), static vs realtime propagation, autonomous lifecycle, campaigns,
  multi-objective rewards.

## Quick Start

```text
1. Create a flag, set a percentage-distribution rule OR set default_rule_distribution.
2. Create an experiment bound to that rule (or set targets_default_rule = true).
3. Set unit_context_types (e.g. ["user", "account"]) + primary metric_ids + (optional) guardrails.
4. Transition draft -> running. The flag is now locked while the experiment runs.
5. Evals are written to flag_evaluation_log_v2 with matched_rule_id; the
   experiment_assignments_mv routes them to experiment_assignments first-exposure rows.
6. View results in the Admin UI Experiment Detail page (per-context-type tab strip).
7. Transition running -> stopped to unlock the flag.
```

## Implementation Status

| Component                                                     | Status      |
|---------------------------------------------------------------|-------------|
| `flag_evaluation_log_v2.targeting_on` + `matched_rule_id`     | ✅ Complete |
| `experiment_iterations_active` CH dictionary                  | ✅ Complete |
| `experiment_assignments_mv` materialized view                 | ✅ Complete |
| 90-day backfill migration                                     | ✅ Complete |
| `feature_flags.default_rule_distribution`                     | ✅ Complete |
| `experiments.targets_default_rule` (XOR with `flag_rule_id`)  | ✅ Complete |
| `experiments.unit_context_types` (per-context-type analysis)  | ✅ Complete |
| `experiments.guardrail_metric_ids` + `pre_period_days` (CUPED)| ✅ Complete |
| Whole-flag lock (HTTP 409 `FLAG_LOCKED_BY_EXPERIMENT`)        | ✅ Complete |
| Stats query cutover (JOIN on `experiment_assignments`)        | ✅ Complete |
| Per-context-type Frequentist / Bayesian / CUPED / SRM         | ✅ Complete |
| Sequential testing (mSPRT always-valid p + confidence sequences, opt-in) | ✅ Complete |
| Multi-armed bandit (Thompson / ε-greedy / UCB / contextual, static + realtime, autonomous lifecycle, campaigns, multi-objective) | ✅ Complete |
| Guardrail direction-violation detection                       | ✅ Complete |
| Gateway: `/results`, `/exposures`, `/timeseries`, `/recompute`| ✅ Complete |
| Admin UI: Results / Exposures / Time-series / Iterations tabs | ✅ Complete |
| Admin UI: Create / Edit modal with rule + default-rule picker | ✅ Complete |
| Admin UI: Flag editor default-rule distribution section       | ✅ Complete |
| Admin UI: Per-context-type tab strip                          | ✅ Complete |

## Rolled Out Across 11 Phases

The work landed across 11 phases tracked in `conductor/tracks/experimentation_full_20260521/`:

1. Data Model Foundations (PG + CH migrations, domain types)
2. Flag Service Eval Log wiring (`matched_rule_id`)
3. Flag Lock Enforcement (`FLAG_LOCKED_BY_EXPERIMENT` 409)
4. Attribution Pipeline (`experiment_assignments_mv` + 90-day backfill)
5. Stats Query Cutover (aggregation / ratio / funnel / preview)
6. Stats Math (Frequentist + Bayesian + CUPED + SRM + Guardrails)
7. Gateway API Surface (`/results`, `/exposures`, `/timeseries`, `/recompute`)
8. Admin UI Foundation (context-type tab strip + ContextTypeContext)
9. UI Detail Tabs (Results / Exposures / Time-series / Iterations)
10. UI Create + Flag Editor + List (default-rule distribution editor)
11. Docs + E2E + Cleanup (this phase)

## Out of Scope

The following remain explicitly out of scope (separate future tracks):

- Group-sequential / alpha-spending boundaries (O'Brien–Fleming / Pocock) — note: fully-sequential
  always-valid inference (mSPRT + confidence sequences) is now implemented, see
  [Sequential Testing](./sequential-testing.md)
- Holdout group / global experiment holdback
- Warehouse-backed (offline) event ingestion
- Client-side (browser/mobile) SDK experiment helpers
- Email/Slack alerting on SRM red or guardrail violation
- Cross-experiment interaction analysis (k×k interaction tables)
- Cross-context-type interaction analysis
- Retroactive backfill of `matched_rule_id` for pre-migration eval log rows
