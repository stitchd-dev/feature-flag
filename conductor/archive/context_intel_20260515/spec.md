# Spec: Context Intelligence & Evaluation Telemetry

Track: `context_intel_20260515`

---

## Overview

Implements the Context Intelligence Layer described in the product guide. Every flag
evaluation is logged to a new ClickHouse table; a scheduled refresh job derives a
registry of observed context types and parameter keys (with inferred data types)
stored in PostgreSQL. This registry powers autocomplete suggestions across the admin UI.
A new evaluation graph on the Flag Detail page visualises evaluation volume over time
with automatic hour/day granularity switching.

The **90-day rolling window** is a universal constraint: no data older than 90 days is
surfaced anywhere — not in autocomplete, not in the Context Explorer, not in the
evaluation graph, and not in any API response.

---

## Functional Requirements

### FR-1: ClickHouse Evaluation Log (`flag_evaluation_log`)

- A new ClickHouse table records every flag evaluation event.
- **Schema** (one row per context per evaluation call):
  `env_id` UUID, `flag_id` UUID, `flag_key` String, `variant_key` String,
  `is_disabled` Bool, `evaluated_at` DateTime64(3, 'UTC'), `context_type` String,
  `context_key` String, `params_json` String (JSON object of key → value).
- **Privacy**: params listed in the context's `privateParameters` have their values
  replaced with `"********"` before the write. Keys are preserved.
- **TTL**: 90 days from `evaluated_at` (ClickHouse TTL clause — hard delete).
- Writes are **fire-and-forget** (`tokio::spawn`) — a write failure is logged but
  never propagates to the evaluation response.
- N contexts per evaluation → N rows.
- `evaluate_preview` calls do **not** write to this table.

### FR-2: Context Parameter Registry (PostgreSQL)

- **`context_type_registry`** table: `(env_id, context_type)` PK,
  `first_seen_at`, `last_seen_at`.
- **`context_param_registry`** table: `(env_id, context_type, param_key)` PK,
  `inferred_type` (text CHECK: `str | int | double | bool | semver | unknown`),
  `is_private` bool, `first_seen_at`, `last_seen_at`.
- Refreshed by `stitchd-stats-service` on a **15-minute interval**.
  The refresh queries ClickHouse for the rolling 90-day window and upserts each
  observed `(context_type, param_key)` pair.
- Entries with `last_seen_at` older than 90 days are purged on each refresh cycle.

### FR-3: Context Intelligence API (gateway)

- `GET /v1/projects/:pid/environments/:eid/context-types`
  → list of `{context_type, last_seen_at}` ordered by recency.
  Only entries with `last_seen_at >= NOW() - 90 days` returned.
- `GET /v1/projects/:pid/environments/:eid/context-types/:type/params`
  → list of `{param_key, inferred_type, is_private, last_seen_at}`.
  API-level 90-day filter applied as safety net even if registry row exists.
- Auth: `flag:read` permission.

### FR-4: Autocomplete Integration (Admin UI)

- **Segment rule builder** — `context_type` field and `param` field both show
  typeahead suggestions from the context intelligence API.
- **Flag rule builder** — same as segment rule builder.
- **Preview tab form builder** — `_type` input and param key inputs show suggestions.
- **Context Explorer page** at `.../environments/:eid/context-explorer` — lists all
  observed context types (last 90 days only), expandable to show each type's params
  with inferred type chip, `is_private` badge (🔒), and last-seen timestamp.
  Linked from the environment sidebar nav.

### FR-5: Evaluation Graph (Flag Detail — new "Analytics" tab)

- **Time range selector**: Last 1 h / 6 h / 24 h / 7 d / 30 d (max 90 d).
- **Auto-granularity**:
  - Selected range ≤ 24 h → **hourly** buckets.
  - Selected range > 24 h → **daily** buckets.
  - Requests with range > 90 days are rejected (HTTP 400).
- **Series** (all on one chart):
  - Total evaluation count.
  - Per-variant breakdown (stacked bars or multi-line, one series per variant key).
  - Disabled evaluations (distinct series or shaded region).
  - Unique context keys per bucket (secondary Y-axis, line).
- Polls every 60 s when range ≤ 24 h.
- **Data API**: `GET /v1/projects/:pid/flags/:fid/eval-stats?from=&to=&granularity=hour|day`
  → `{ buckets: [{ts, total, by_variant: {k: n}, disabled_count, unique_context_keys}] }`.
- Backend queries `flag_evaluation_log` directly (no materialized view — deferred).

---

## Non-Functional Requirements

- **NFR-1**: Evaluation log write must not add > 5 ms p95 to flag evaluation latency.
- **NFR-2**: Context registry refresh runs in stats-service on 15-min cadence; never
  triggered per-request.
- **NFR-3**: Private parameter values are masked **in-process** before any ClickHouse
  write; they never leave the flag-service in plain text.
- **NFR-4: Universal 90-day data horizon** — all data surfaces are bounded by a strict
  90-day rolling window. No data older than 90 days is surfaced anywhere:
  - ClickHouse TTL = 90 days (hard delete).
  - Registry refresh queries only `evaluated_at >= NOW() - INTERVAL 90 DAY`.
  - Registry entries with `last_seen_at` older than 90 days are purged each refresh.
  - Evaluation graph time range selector maximum = 90 days.
  - Context intelligence API applies a 90-day filter at the API level as a safety net.
  - Context Explorer shows no entries older than 90 days.
  - Autocomplete never returns a suggestion with `last_seen_at` older than 90 days.

---

## Acceptance Criteria

- [ ] `flag_evaluation_log` ClickHouse table exists; evaluations appear within 60 s.
- [ ] Private param values stored as `"********"`; keys present and queryable.
- [ ] `evaluate_preview` calls produce zero rows in `flag_evaluation_log`.
- [ ] Context registry refreshes every 15 min; new keys appear in autocomplete within
      one refresh cycle.
- [ ] No data older than 90 days is returned from any API or shown in any UI surface.
- [ ] Segment builder, flag builder, and preview tab all show live type/param suggestions.
- [ ] Context Explorer lists observed types/params; private params show 🔒 badge.
- [ ] Evaluation graph shows correct granularity (hourly ≤ 24 h, daily > 24 h).
- [ ] All four chart series render: total, per-variant, disabled, unique context keys.
- [ ] No Clippy warnings, no TypeScript errors, ≥ 90 % coverage on new Rust code.

---

## Out of Scope

- Value-level autocomplete (keys only — no observed values surfaced).
- ClickHouse materialized views for eval-graph queries (deferred).
- Real-time graph streaming (polling on 60 s interval is sufficient).
- Client-side SDK context capture (server-side evaluation path only).
- Cross-environment context aggregation (registry is per-environment).
