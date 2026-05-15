# Implementation Plan: Context Intelligence & Evaluation Telemetry

Track: `context_intel_20260515`

---

## Phase 1: ClickHouse Evaluation Log
<!-- execution: sequential -->

- [ ] Task 1: ClickHouse migration — `flag_evaluation_log` table
  - Schema: env_id UUID, flag_id UUID, flag_key String, variant_key String,
    is_disabled Bool, evaluated_at DateTime64(3,'UTC'), context_type String,
    context_key String, params_json String
  - Engine: MergeTree() PARTITION BY toYYYYMM(evaluated_at)
    ORDER BY (env_id, flag_id, evaluated_at)
  - TTL: evaluated_at + INTERVAL 90 DAY DELETE

- [ ] Task 2: Privacy masking + fire-and-forget write path in `stitchd-flag-service`
  - `EvalLogWriter` struct; masks private param values → `"********"` before write
  - One row per context per evaluation; tokio::spawn (non-blocking, < 5 ms p95)
  - Wired into `evaluate` handler only — `evaluate_preview` does NOT write
  - Unit tests: masking correctness, spawn behavior, preview exclusion

- [ ] Task 3: Integration test — rows appear in ClickHouse after evaluate call
  - Assert correct masking, correct row count (N contexts → N rows)
  - Assert no rows written for evaluate_preview calls

- [ ] Task: Conductor - User Manual Verification 'ClickHouse Evaluation Log' (Protocol in workflow.md)

---

## Phase 2: Context Parameter Registry
<!-- depends: phase1 -->
<!-- execution: sequential -->

- [ ] Task 1: PostgreSQL migrations — `context_type_registry` + `context_param_registry`
  - `context_type_registry`: (env_id, context_type) PK, first_seen_at, last_seen_at
  - `context_param_registry`: (env_id, context_type, param_key) PK,
    inferred_type text CHECK('str','int','double','bool','semver','unknown'),
    is_private bool, first_seen_at, last_seen_at

- [ ] Task 2: Repository trait + Postgres impl
  - `ContextRegistryRepository` trait: upsert_context_type, upsert_param,
    list_types (90-day filter), list_params (90-day filter), purge_stale
  - `PgContextRegistryRepository` impl
  - sqlx::test for each method including 90-day boundary assertions

- [ ] Task 3: Type inference logic
  - `infer_type(value: &str) -> InferredType` — tries: bool → int → double → semver → str
  - Private param (value == "********") → Unknown
  - Unit tests for all type variants + private param case

- [ ] Task 4: Stats-service registry refresh job (15-min interval)
  - `ContextRegistryRefresher` in stitchd-stats-service
  - Queries ClickHouse: GROUP BY env_id, context_type, context_key
    WHERE evaluated_at >= now() - INTERVAL 90 DAY
  - Upserts PostgreSQL registry; purges entries with last_seen_at < 90 days ago
  - Wired into existing scheduler on 15-min cadence
  - Unit tests with mocked ClickHouse + PostgreSQL repos

- [ ] Task: Conductor - User Manual Verification 'Context Parameter Registry' (Protocol in workflow.md)

---

## Phase 3: Evaluation Graph API + UI
<!-- depends: phase1 -->
<!-- execution: sequential -->

- [ ] Task 1: Eval stats gateway route + ClickHouse query
  - GET /v1/projects/:pid/flags/:fid/eval-stats?from=&to=&granularity=hour|day
  - Auto-granularity: override to `day` if range > 24 h; HTTP 400 if range > 90 days
  - ClickHouse: GROUP BY toStartOfHour/toStartOfDay, variant_key, is_disabled;
    COUNT(*) + COUNT(DISTINCT context_key)
  - Response: { buckets: [{ts, total, by_variant:{k:n}, disabled_count, unique_context_keys}] }
  - Unit tests: granularity auto-switching, 90-day range rejection

- [ ] Task 2: Analytics tab on Flag Detail page (`AnalyticsTab.tsx`)
  - Time range selector: Last 1h / 6h / 24h / 7d / 30d (capped at 90d)
  - Auto-selects granularity; shows "Hourly" / "Daily" label
  - ComposedChart (recharts): stacked bars per variant, disabled series shaded,
    unique context keys on secondary Y-axis (line)
  - Polls every 60 s when range ≤ 24 h
  - Vitest tests: granularity switching, empty state, series key rendering

- [ ] Task: Conductor - User Manual Verification 'Evaluation Graph' (Protocol in workflow.md)

---

## Phase 4: Context Intelligence API + Autocomplete + Explorer
<!-- depends: phase2 -->
<!-- execution: sequential -->

- [ ] Task 1: Gateway context intelligence routes
  - GET .../environments/:eid/context-types → [{context_type, last_seen_at}]
    (last_seen_at within 90 days, ordered by recency)
  - GET .../environments/:eid/context-types/:type/params
    → [{param_key, inferred_type, is_private, last_seen_at}]
    (API-level 90-day safety filter applied)
  - RBAC: flag:read
  - Integration tests with seeded registry data

- [ ] Task 2: `useContextSuggestions` hook + `SuggestionInput` component
  - Hook: fetches intelligence API, debounced 200 ms, handles loading/error
  - SuggestionInput: input + dropdown, keyboard nav, private params show 🔒
  - Vitest: debounce, empty state, error state, keyboard navigation

- [ ] Task 3: Autocomplete in Segment rule builder
  - context_type field → type suggestions via SuggestionInput
  - param key field → param suggestions for selected type; private params show 🔒
  - Vitest: suggestions appear for known types/params

- [ ] Task 4: Autocomplete in Flag rule builder
  - Same as Task 3 for flag condition editor
  - Vitest: component test

- [ ] Task 5: Autocomplete in Preview tab form builder
  - _type input and param key inputs → SuggestionInput
  - Vitest: component test

- [ ] Task 6: Context Explorer page
  - Route: .../environments/:eid/context-explorer
  - Lists context types (last 90 days only), sorted by last_seen_at desc
  - Expandable rows: param key, inferred type chip, 🔒 badge, last seen date
  - Empty state when no data in 90-day window
  - Linked from environment sidebar nav
  - Vitest: expand/collapse, empty state, private badge rendering

- [ ] Task: Conductor - User Manual Verification 'Context Intelligence & Explorer' (Protocol in workflow.md)
