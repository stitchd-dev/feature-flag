# Default-Rule Experiments

A flag's **default rule** is the fallthrough path: the evaluation outcome when targeting is on
but no custom rule matches. Historically the default rule was a single fixed variant. With
`experimentation_full_20260521`, a flag can opt into a **percentage-distribution default rule**
and an experiment can bind directly to that fallthrough — letting you experiment on the
"untargeted" population without writing a custom rule purely to host the experiment.

## When to Use

Use a default-rule experiment when you want to:

- A/B test the **baseline experience** of a flag (no segmentation).
- Run a holdback / always-on experiment without polluting the rule list with a dummy
  100%-allocation custom rule.
- Experiment on rollouts where every untargeted user gets randomized.

Use a **rule-bound** experiment instead when you want to scope the experiment to a specific
segment, percentage rollout to a cohort, or any other custom targeting condition.

## Data Model

Two columns are new with this track:

### `feature_flags.default_rule_distribution Jsonb`

JSON shape:

```json
{
  "allocations": [
    { "variant_key": "control",   "weight_basis_points": 5000 },
    { "variant_key": "treatment", "weight_basis_points": 5000 }
  ],
  "rollout_hash_attribute_keys": []
}
```

- `weight_basis_points` is in basis points (`10000` = 100%). Allocations must sum to `10000`.
- 0.1% granularity is enforced (each weight must be a multiple of `10`).
- `rollout_hash_attribute_keys` is normally empty — the rollout hash is computed over
  every present-context's `key` in iteration order (the same convention as
  percentage-distribution rules).
- `NULL` (the default) preserves today's single-variant fallthrough behaviour.

For **boolean flags** (`flag_type = bool`), the distribution must contain exactly two
allocations for `true` and `false` — same invariant as for boolean variants generally.

### `experiments.targets_default_rule Boolean NOT NULL DEFAULT false`

Companion to the existing `flag_rule_id`. The pair is **XOR**-constrained:

```sql
CHECK ((flag_rule_id IS NOT NULL AND targets_default_rule = false)
    OR (flag_rule_id IS NULL     AND targets_default_rule = true))
```

When `targets_default_rule = true`, the experiment's exposure source is every eval where
`flag_evaluation_log_v2.matched_rule_id IS NULL` (i.e. the flag fell through to the
default rule) AND `context_type ∈ unit_context_types`.

## Lifecycle

### 1. Configure the Flag's Default-Rule Distribution

Endpoint:

```http
POST /v1/environments/{env_id}/flags/{flag_key}/default-rule-distribution
Content-Type: application/json

{
  "allocations": [
    { "variant_key": "control",   "weight_basis_points": 5000 },
    { "variant_key": "treatment", "weight_basis_points": 5000 }
  ]
}
```

Validation rules:

- The flag must NOT currently be locked by a running / paused experiment
  (returns `409 FLAG_LOCKED_BY_EXPERIMENT`).
- Each `variant_key` must reference a variant that exists on the flag.
- Weights must sum to `10000` exactly.
- Each weight must be a multiple of `10` (0.1% granularity).

In the Admin UI: open the Flag editor → **Default Rule** section → choose
**Percentage distribution** → set per-variant weights → save.

### 2. Create the Experiment

```http
POST /v1/environments/{env_id}/experiments
Content-Type: application/json

{
  "key": "homepage-color-baseline",
  "name": "Homepage color (default-rule)",
  "flag_id": "<flag uuid>",
  "flag_rule_id": null,
  "targets_default_rule": true,
  "unit_context_types": ["user", "account"],
  "metric_ids":           ["<metric uuid 1>", "<metric uuid 2>"],
  "guardrail_metric_ids": ["<guardrail uuid>"],
  "pre_period_days": 14,
  "traffic_allocation": 10000
}
```

Validation:

- `targets_default_rule = true` requires `flag.default_rule_distribution IS NOT NULL` —
  returns `422 INVALID_DEFAULT_RULE_KIND` otherwise.
- `flag_rule_id` must be `null` (XOR constraint).
- `unit_context_types` is non-empty and every entry is a known context type
  (`422 EMPTY_UNIT_CONTEXT_TYPES` / `422 UNKNOWN_CONTEXT_TYPE`).
- Exactly one experiment can be `running` or `paused` per flag (whole-flag uniqueness,
  enforced by partial index `idx_experiments_one_active_per_flag`).

In the Admin UI: **New experiment** → in the rule picker, choose the
**"Default rule (fallthrough)"** entry. The entry is only present when the flag has a
`default_rule_distribution`; otherwise the picker surfaces a "Configure default-rule
distribution on the flag first" CTA linking to the flag editor.

### 3. Start, Run, Analyse

`POST /v1/.../experiments/{id}/transition` with `new_status = running` starts the experiment.
At transition:

- A new row is added to `experiment_iterations` with snapshot fields including
  `default_rule_distribution`, `unit_context_types`, `metric_ids`, and `guardrail_metric_ids`.
- `experiment_iterations_active` is reloaded via `SYSTEM RELOAD DICTIONARY`.
- `experiment_assignments_mv` immediately starts routing default-rule evals
  (`matched_rule_id IS NULL`) into `experiment_assignments` for the configured context types.
- The flag is now locked — all mutation endpoints (PATCH flag, variant CRUD, rule CRUD,
  default-rule-distribution update, archive) return `409 FLAG_LOCKED_BY_EXPERIMENT`.

Results are visible via `GET /v1/.../experiments/{id}/results` — the response shape is
identical to rule-bound experiments, with `bound_target.kind = "default_rule"` and
`bound_target.rule_id = null`. The Admin UI Experiment Detail page renders the
**"Bound to: Default rule"** badge in the Exposure / SRM panel.

### 4. Stop and Restart with Changes

Stopping the experiment (`new_status = stopped`) unlocks the flag. You can now:

- Edit the `default_rule_distribution` (the lock is gone).
- Add new variants, change rules, etc.
- Restart the experiment — a **new iteration** is created with a fresh snapshot capturing
  any changes to `default_rule_distribution`, `unit_context_types`, and metric configuration.

The previous iteration's `experiment_assignments` rows remain (they are partitioned by
iteration in the stats join), so historical results are preserved.

## Whole-Flag Lock Interaction

The whole-flag lock applies uniformly whether the experiment is rule-bound or
default-rule-bound. While the experiment is `running` or `paused`:

| Endpoint                                                  | Locked? |
|-----------------------------------------------------------|---------|
| `PUT  /v1/.../flags/{key}`                                | Yes — 409 |
| `DELETE /v1/.../flags/{key}`                              | Yes — 409 |
| `POST /v1/.../flags/{key}/variants` (and friends)         | Yes — 409 |
| `POST /v1/.../flags/{key}/rules` (and friends)            | Yes — 409 |
| `POST /v1/.../flags/{key}/default-rule-distribution`      | **Yes — 409** |
| `POST /v1/.../flags/{key}/enable` / `disable`             | Yes — 409 |
| `POST /v1/.../flags/{key}/archive`                        | Yes — 409 |
| `GET  /v1/.../flags/{key}`                                | No      |
| `POST /v1/.../flags/{key}/evaluate-preview`               | No      |
| SDK `evaluate()` calls                                    | No      |

The default-rule-distribution endpoint is included in the lock list so a running default-rule
experiment cannot have its distribution rewritten under it — the iteration snapshot is the
load-bearing source for both exposure routing and SRM expected-count math.

## Bool-Type Caveat

Boolean flags always have exactly two variants (`true` / `false`). A default-rule distribution
on a boolean flag must therefore allocate to both variants — there are no other valid
`variant_key` values. The Admin UI flag editor renders the two rows read-only and only lets
the user edit the weights.
