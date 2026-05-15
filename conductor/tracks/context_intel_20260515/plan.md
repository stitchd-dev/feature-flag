# Implementation Plan: Context Intelligence & Evaluation Telemetry

Track: `context_intel_20260515`

---

## Phase 1: ClickHouse Evaluation Log
<!-- execution: sequential -->

- [x] Task 1: ClickHouse migration — `flag_evaluation_log` table (0a22243)
  - Schema: env_id UUID, flag_id UUID, flag_key String, variant_key String,
    is_disabled Bool, evaluated_at DateTime64(3,'UTC'), context_type String,
    context_key String, params_json String
  - Engine: MergeTree() PARTITION BY toYYYYMM(evaluated_at)
    ORDER BY (env_id, flag_id, evaluated_at)
  - TTL: evaluated_at + INTERVAL 90 DAY DELETE

- [x] Task 2: Privacy masking + fire-and-forget write path in `stitchd-flag-service` (ddab318)
  - `eval_log_writer` module; masks private param values → `"********"` before write
  - Per-context variant_key pairing; one row per context; tokio::spawn fire-and-forget
  - Wired into `evaluate_preview` handler via optional `ch_client` on `FlagServiceImpl`
  - Proto: `environment_id` field added to `EvaluatePreviewRequest`; gateway + UI forward it
  - Unit tests: masking, per-context variants, all param types, empty/disabled cases

- [x] Task 3: Integration test — rows appear in ClickHouse after evaluate_preview call (161aebd)
  - Fixed EvalLogRow serde: UUID fields → clickhouse::serde::uuid, DateTime64 → datetime64::millis
  - eval_log_rows_appear_in_clickhouse: masking, variant_key, env_id, context assertions
  - eval_log_n_contexts_produces_n_rows: 5 contexts → 5 rows in table
  - Tests skip gracefully when CLICKHOUSE_URL not set

- [x] Task: Conductor - User Manual Verification 'ClickHouse Evaluation Log' (fb8d214)
  - Live test confirmed: evaluate_preview → 2 rows in flag_evaluation_log
  - alice: email=********, plan=pro (masking verified)
  - bob: plan=free (plain params preserved)

---

## Phase 2: Context Parameter Registry
<!-- depends: phase1 -->
<!-- execution: sequential -->

- [x] Task 1: PostgreSQL migrations — `context_type_registry` + `context_param_registry` (c8206e0)
  - `context_type_registry`: (env_id, context_type) PK, first_seen_at, last_seen_at
  - `context_param_registry`: (env_id, context_type, param_key) PK,
    inferred_type text CHECK('str','int','double','bool','semver','unknown'),
    is_private bool, first_seen_at, last_seen_at
  - Indexes on (env_id, last_seen_at DESC) for 90-day window queries

- [x] Task 2: Repository trait + Postgres impl (7634076)
  - `ContextRegistryRepository` trait: upsert_context_type, upsert_param,
    list_types (90-day filter), list_params (90-day filter), purge_stale
  - `PgContextRegistryRepository` impl using sqlx non-macro query_as
  - ON CONFLICT upsert for first_seen_at/last_seen_at refresh semantics
  - 6 sqlx::test integration tests including 90-day boundary assertions

- [x] Task 3: Type inference logic (7634076)
  - `InferredType::infer(value: &str) -> InferredType` in stitchd-core::context
  - Priority: bool → int → double → semver → str; "********" → Unknown
  - 8 unit tests: all type variants, private param, priority ordering, FromStr round-trip

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
