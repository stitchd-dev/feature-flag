# Attribution Model

Stitchd derives experiment exposure server-side from the flag-evaluation log. SDKs do **not**
need to know about experiments — every eval call that the SDK makes already produces the data
required to attribute downstream events to a variant.

## Pipeline Overview

```mermaid
flowchart LR
    SDK[Server-side SDK<br/>FlagSdkBackend RPC] -->|eval row| EVAL[(flag_evaluation_log_v2<br/>+ targeting_on + matched_rule_id)]
    EVAL -->|JOIN via dictionary| MV{{experiment_assignments_mv}}
    DICT[(experiment_iterations_active<br/>CH dictionary)] -. refreshed on start/stop .-> MV
    MV -->|first-exposure rows| ASSIGN[(experiment_assignments<br/>ReplacingMergeTree)]
    EVENTS[(events_v2)] --> STATS[stats-service<br/>queries::{aggregation,ratio,funnel,preview}]
    ASSIGN --> STATS
    STATS --> RESULTS[(experiment_results)]
    RESULTS --> GATEWAY[GET /v1/.../experiments/{id}/results]
```

Five components:

1. **`flag_evaluation_log_v2`** — every SDK eval is logged here. The row carries
   `(env_id, flag_id, context_type, context_key, evaluated_at, variant_key, targeting_on,
   matched_rule_id, ...)`. `matched_rule_id` is `NULL` when the eval fell through to the
   flag's default rule, set to the rule UUID when a custom rule matched, and the row is
   not written at all when the flag was disabled (`targeting_on = false` evals never
   produce experiment exposures).
2. **`experiment_iterations_active`** — a ClickHouse dictionary refreshed from PostgreSQL
   (`SYSTEM RELOAD DICTIONARY`) on every iteration start / stop. Keyed on
   `(env_id, flag_id, matched_rule_id, context_type)`. The dictionary is the routing table
   from "what evaluated" to "which experiment-iteration this eval belongs to".
3. **`experiment_assignments_mv`** — a materialized view over `flag_evaluation_log_v2`.
   For each new eval row it `dictGet`-joins the iteration dictionary; if a match exists
   AND `context_type ∈ unit_context_types` AND `targeting_on = true`, it writes a row
   into `experiment_assignments`.
4. **`experiment_assignments`** — `ReplacingMergeTree` keyed on
   `(experiment_id, context_type, context_key)`. The `version` column is
   `-toUnixTimestamp(evaluated_at)` so that `MAX(version)` returns the **earliest**
   timestamp — i.e. first exposure wins, later exposures with a different variant do
   not reassign the context within an iteration.
5. **Stats queries** — `crates/stitchd-stats-service/src/queries/{aggregation, ratio, funnel,
   preview}.rs` JOIN `events_v2 e` against `experiment_assignments a` on
   `(env_id, context_type, context_key)` using
   `arrayExists(t -> t.1 = a.context_type AND t.2 = a.context_key, e.contexts)` and
   filter `e.occurred_at >= a.assigned_at` to enforce ITT. Results GROUP BY
   `(a.context_type, a.variant_key)`.

## Semantic Guarantees

The pipeline is designed around three hard invariants. Each is enforced by tests in
`crates/stitchd-experimentation-service/tests/` and exercised end-to-end by the
[lifecycle E2E test](#end-to-end-test).

### 1. Rule-Scoped Exposure

Only evals where `flag_evaluation_log_v2.matched_rule_id` matches the bound experiment's
`flag_rule_id` count as exposure. Evals matching a **different** rule on the same flag do
not produce assignments.

- Experiment binds to a specific `flag_rule_id` → assignment fires only when
  `flag_evaluation_log_v2.matched_rule_id = experiment.flag_rule_id`.
- Experiment binds to the default rule (`targets_default_rule = true`) → assignment fires
  only when `flag_evaluation_log_v2.matched_rule_id IS NULL` (fell through to default).

The XOR is enforced at the PG layer by a `CHECK` constraint on `experiments`:

```sql
CHECK ((flag_rule_id IS NOT NULL AND targets_default_rule = false)
    OR (flag_rule_id IS NULL     AND targets_default_rule = true))
```

### 2. Context-Type-Scoped Exposure

Each experiment requires `unit_context_types text[] NOT NULL` with at least one entry
(default `{user}`). Each entry must be a known context type for the environment (validated
against `context_type_registry`).

Only evals where `flag_evaluation_log_v2.context_type ∈ experiment.unit_context_types`
count. The MV `dictGet` keys on `(env_id, flag_id, matched_rule_id, context_type)` —
context types not in the iteration's snapshot are simply not in the dictionary and the row
is dropped.

`unit_context_types` is **snapshotted** into `experiment_iterations` at iteration start so
that changes between iterations are captured per-iteration (a restart-with-changes flow).

### 3. First-Exposure ITT

Once a row exists in `experiment_assignments` for `(experiment_id, context_type, context_key)`,
subsequent evals — even with a different variant — do NOT overwrite it within the iteration.
This is the **intent-to-treat** semantic: a context's variant is fixed at first exposure.

Implementation: `experiment_assignments` is a `ReplacingMergeTree` where the version column is
`-toUnixTimestamp(assigned_at)` so `MAX(version)` returns the **earliest** timestamp.

Events fired **before** the first exposure (`e.occurred_at < a.assigned_at`) are filtered out
of the stats join — pre-exposure events do NOT count.

## CH Dictionary Refresh

The `experiment_iterations_active` dictionary keys on:

```text
(env_id, flag_id, matched_rule_id, context_type) -> (experiment_id, iteration_id, ...)
```

`matched_rule_id` is `Nullable(UUID)` so the default-rule path (`matched_rule_id IS NULL`)
maps to default-rule-bound experiments. `context_type` is the inner-most key so the same
iteration appears once per row in `unit_context_types`.

The dictionary is configured `LIFETIME(MIN 300 MAX 600)` (5-10 min refresh) plus
**explicit `SYSTEM RELOAD DICTIONARY experiment_iterations_active`** on every iteration
start / stop transition. The explicit reload ensures attribution starts firing within
milliseconds of a transition — without it, the MV would silently drop evals for newly-running
experiments for up to 10 minutes.

## Backfill

A one-shot migration backfills the last 90 days from `flag_evaluation_log_v2` into
`experiment_assignments`:

```sql
INSERT INTO experiment_assignments
SELECT
    iter.experiment_id,
    iter.iteration_id,
    iter.flag_id,
    e.env_id,
    e.context_type,
    e.context_key,
    e.variant_key,
    e.evaluated_at AS assigned_at,
    e.matched_rule_id,
    -toUnixTimestamp(e.evaluated_at) AS _version
FROM flag_evaluation_log_v2 e
JOIN experiment_iterations_active iter
    ON  iter.env_id = e.env_id
    AND iter.flag_id = e.flag_id
    AND ((iter.matched_rule_id IS NULL AND e.matched_rule_id IS NULL)
        OR iter.matched_rule_id = e.matched_rule_id)
    AND iter.context_type = e.context_type
WHERE e.targeting_on = true
  AND e.evaluated_at >= iter.started_at
  AND e.evaluated_at < COALESCE(iter.ended_at, now())
  AND e.evaluated_at >= now() - INTERVAL 90 DAY;
```

Backfill is **skipped** for rows where `matched_rule_id` is absent (pre-migration rows
written before the schema added the column) — those contexts get attributed only from
go-forward evals.

## What Replaced What

| Before (`events_metrics_20260519`)                            | After (`experimentation_full_20260521`)                            |
|---------------------------------------------------------------|--------------------------------------------------------------------|
| SDK tags every event with `(experiment, iteration, variant)`  | SDK is experiment-unaware; eval log carries the attribution data   |
| Stats query filters on `e.contexts`'s experiment/variant tags | Stats query JOINs `events_v2` ⨝ `experiment_assignments`            |
| One result per `(metric_key, variant_key)`                    | One result per `(metric_key, context_type, variant_key)`           |
| Per-rule `frozen` flag                                        | Whole-flag lock derived from `EXISTS(experiment WHERE flag_id ...)`|
| `experiments.experiment_event_keys`                           | `experiments.metric_ids` + `guardrail_metric_ids`                  |

## End-to-end Test

The full pipeline is exercised by
`crates/stitchd-experimentation-service/tests/experiment_lifecycle_e2e.rs`. The test:

1. Creates a flag + percentage rule.
2. Creates an experiment with `unit_context_types = ['user', 'account']`.
3. Transitions draft → running and verifies the dictionary refresh.
4. Emits eval-log rows for **both** context types, mixing `targeting_on=true / false`,
   pre-iteration, during, and post-iteration timestamps.
5. Emits `events_v2` rows including pre-assignment ones that must be ITT-filtered out.
6. Verifies `experiment_assignments` contains exactly the first-exposure-during-iteration
   rows for `unit_context_types`-matching context types.
7. Calls the stats compute path and verifies per-context-type results.
8. Transitions running → stopped and verifies the flag unlocks.
9. Verifies guardrail direction-violation detection against seeded data.

The test is `#[ignore = "needs live PG + CH + ScyllaDB stack"]` by default. Opt in with:

```bash
DATABASE_URL=postgresql://stitchd:stitchd@localhost:5432/stitchd \
STITCHD_CLICKHOUSE_URL=http://localhost:8123 \
cargo test -p stitchd-experimentation-service --test experiment_lifecycle_e2e -- --ignored
```
