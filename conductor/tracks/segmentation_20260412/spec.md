# Spec: Segmentation

## Overview

Implement the complete Segmentation module: evaluation logic (`stitchd-core`),
persistence for segment rules and list entries (`stitchd-db`), and REST Admin API
endpoints for managing segments (`stitchd-server`). Auth enforcement is deferred
to the Auth track — endpoints exist but are unprotected.

Segmentation builds directly on the Rule Engine track (which provides
`ConditionExpr`, `Rule`, `RuleOutput`, and `EvaluationInput`) and the Domain
track (which provides the `segments` table, `SegmentId`, `SegmentRepository`,
and `Context` types).

## Functional Requirements

### Segment Definition Types (`stitchd-core`)

```rust
enum SegmentDefinition {
    RuleBased(RuleBasedSegment),
    ListBased(ListBasedSegment),
}

struct RuleBasedSegment {
    id: SegmentId,
    rules: Vec<Rule>,  // ordered; first match wins
}

struct ListBasedSegment {
    id: SegmentId,
    // context_type → per-context include/exclude lists
    lists: HashMap<String, ContextList>,
}

struct ContextList {
    include: HashSet<String>,  // context keys in the segment
    exclude: HashSet<String>,  // exclude wins over include
}
```

### Segment Evaluation Rules

**Rule-based segments:**
- Evaluate the segment's `Vec<Rule>` via the Rule Engine against provided contexts.
- Segments are fully independent — `InSegment`/`NotInSegment` conditions are NOT
  valid within segment rules and will be rejected at creation time with a
  validation error (`InvalidSegmentRule`). Segment composition is the
  responsibility of the Feature Flag layer.
- First matching rule wins. No match → not in segment.

**List-based segments:**
- For each `context_type` key in `lists`, look up the matching context from input.
- If context key ∈ `exclude` → NOT in segment (regardless of include).
- Else if context key ∈ `include` → IN segment.
- Else → NOT in segment.
- A context is in the segment if ANY context_type lookup yields true.
- If no matching context is provided for any listed context_type → not in segment.

### SegmentEvaluator API (`stitchd-core`)

```rust
struct MatchResult {
    matched: bool,
    trace: MatchTrace,
}

enum MatchTrace {
    RuleBased { matched_rule_index: Option<usize> },
    ListBased {
        context_type: Option<String>,
        reason: ListMatchReason,
    },
}

enum ListMatchReason { Included, Excluded, NoMatch, NoContext }

enum SegmentEvaluatorError {
    RuleEngine(RuleEngineError),
    InvalidSegmentRule,  // InSegment/NotInSegment used inside a segment rule
}
```

- `evaluate_one(contexts: &[Context], segment: &SegmentDefinition) -> MatchResult`
  — no `resolved` argument; segments have no knowledge of other segments.
- `evaluate_all(contexts: &[Context], segments: &[SegmentDefinition])
  -> HashMap<SegmentId, MatchResult>`
  — evaluates each segment independently; order does not matter.

Both accept `&[Context]` (one per context_type).

### Database Schema (`stitchd-db`)

Two new migration files (building on the `segments` table from Domain track):

**`20260412000001_segment_rules.sql`**
```sql
CREATE TABLE segment_rules (
  id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  segment_id  UUID NOT NULL REFERENCES segments(id),
  rule_index  INT  NOT NULL,
  rule_def    JSONB NOT NULL,
  UNIQUE (segment_id, rule_index)
);
CREATE INDEX ON segment_rules (segment_id, rule_index ASC);
```

**`20260412000002_segment_list_entries.sql`**
```sql
-- Requires: pg_partman extension
-- Auto-partitioned monthly on created_at; pg_partman pre-creates 3 months ahead.
-- NOTE: Partitioning strategy deferred for future review — see Deferred Decisions.

CREATE EXTENSION IF NOT EXISTS pg_partman;

CREATE TABLE segment_list_entries (
  id           UUID        NOT NULL DEFAULT gen_random_uuid(),
  segment_id   UUID        NOT NULL REFERENCES segments(id),
  context_type TEXT        NOT NULL,
  entry_key    TEXT        NOT NULL,
  list_type    TEXT        NOT NULL CHECK (list_type IN ('include', 'exclude')),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (segment_id, context_type, entry_key, list_type, created_at)
) PARTITION BY RANGE (created_at);

SELECT partman.create_parent(
  p_parent_table    => 'public.segment_list_entries',
  p_control         => 'created_at',
  p_interval        => 'monthly',
  p_premake         => 3,
  p_start_partition => date_trunc('month', now())::text
);

UPDATE partman.part_config
   SET retention = NULL, infinite_time_partitions = true
 WHERE parent_table = 'public.segment_list_entries';

CREATE INDEX ON segment_list_entries (segment_id, context_type, list_type);
```

Maintenance: `stitchd-server` calls `partman.run_maintenance(false)` at startup
and on a 1-hour background Tokio interval.

### Repository Layer (`stitchd-db`)

Extend `SegmentRepository` trait with:
- `find_with_rules(id: SegmentId) -> Result<RuleBasedSegment, RepositoryError>`
- `find_with_list(id: SegmentId) -> Result<ListBasedSegment, RepositoryError>`
- `upsert_rules(id: SegmentId, rules: &[Rule]) -> Result<(), RepositoryError>`
- `set_list_entries(id: SegmentId, context_type: &str, include: &[String],
  exclude: &[String]) -> Result<(), RepositoryError>`

`PgSegmentRepository` implements all methods using `sqlx::query!` macros.

Integration tests via `#[sqlx::test]`:
- Rule-based: upsert rules → find with rules → round-trips correctly
- List-based: set entries → find with list → round-trips correctly
- Soft-delete → absent from `list_by_environment`

### REST Admin API (`stitchd-server`)

Base path: `/v1/environments/{env_id}/segments`. No auth (deferred).

| Method | Path | Description |
|---|---|---|
| `GET` | `/v1/environments/{env_id}/segments` | List active segments |
| `POST` | `/v1/environments/{env_id}/segments` | Create segment |
| `GET` | `/v1/environments/{env_id}/segments/{seg_id}` | Get segment + definition |
| `PUT` | `/v1/environments/{env_id}/segments/{seg_id}` | Replace definition |
| `DELETE` | `/v1/environments/{env_id}/segments/{seg_id}` | Soft-delete |

`POST` / `PUT` body includes `key`, `segment_type`, and either `rules` or `lists`.
`PUT` requires `version: i64` for optimistic locking.

HTTP status codes: `200/201` success, `404` not found, `409` version conflict
or unique violation, `422` validation errors.

## Non-Functional Requirements

- `SegmentEvaluator` is pure (no I/O, no async)
- List-based lookup is O(1) per context_type
- `evaluate_all` evaluates each segment independently (no topological sort needed)
- All `sqlx::query!` macros compile-time verified via `.sqlx/` cache

## Acceptance Criteria

- [ ] List-based: exclude wins when a key is in both lists
- [ ] List-based: no matching context provided → not in segment
- [ ] Rule-based: first matching rule wins; no match → not in segment
- [ ] `InSegment`/`NotInSegment` inside a segment rule → `InvalidSegmentRule`
      error at segment creation time (validated before persistence)
- [ ] `evaluate_all` evaluates each segment independently (no ordering dependency)
- [ ] `MatchTrace` identifies matched rule index or list context_type and reason
- [ ] Both migration files run cleanly against a fresh PostgreSQL instance
- [ ] Repository integration tests pass (`#[sqlx::test]`)
- [ ] All 5 REST endpoints return correct HTTP status codes
- [ ] `PUT` with stale `version` → `409 Conflict`
- [ ] `cargo test -p stitchd-core -p stitchd-db -p stitchd-server` ≥90%
      coverage on segmentation modules
- [ ] `cargo clippy -- -D warnings` passes clean

## Deferred Decisions

- **`segment_list_entries` partitioning strategy:** Current approach uses monthly
  range partitioning via `pg_partman`. Revisit when the table approaches scale —
  consider hash sub-partitioning on `segment_id` within monthly partitions,
  finer granularity (weekly), or a retention/archival policy for stale entries.

## Out of Scope

- Auth middleware / RBAC enforcement (Auth track)
- Flag rule evaluation using resolved segments (Flag Evaluation track)
- Inter-segment dependencies — segments are fully independent; composition is
  the responsibility of the Feature Flag layer
- gRPC endpoints for segmentation
- Event ingestion or experimentation
- Admin UI
