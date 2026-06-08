# Spec: Platform Hardening — Idempotency Keys, On-Demand Interaction Recompute, Cursor Pagination, Fresh-DB Tooling

> **Last Revised: 2026-06-08 (Revision #1)** — FR-4 (cursor pagination) amended to
> the as-built approach: the cursor **contract** is delivered via an opaque
> encoded-offset at the gateway over the existing offset-based service RPCs (true
> keyset internals deferred as follow-up `feature-flag-cj5`); cursor scope is the
> top-level resource collections only — experiment-detail sub-lists
> (iterations, exposures) stay page-based. See `revisions.md`.

## Overview

A robustness/operational track closing confirmed gaps left across prior tracks:

1. **Idempotency keys on REST mutations** — a mandated API principle
   (`product-guidelines.md:13`, "Mutation endpoints accept idempotency keys")
   that was never implemented in the gateway. Safe client retries currently
   risk duplicate creates/mutations.
2. **Idempotency on SDK gRPC / event-ingest paths** — duplicate event batches on
   at-least-once retry can double-count; the ingest path needs exactly-once
   server-side semantics via a per-batch idempotency key.
3. **On-demand interaction recompute** (`feature-flag-uga`) — cross-experiment
   `run_interaction_sweep` runs only on the 60-min scheduler tick; the
   `TriggerRecompute` RPC drives per-experiment stats + bandit reallocation
   but never refreshes `experiment_interactions`.
4. **Cursor-based pagination migration** — `product-guidelines.md:11` mandates
   "All list endpoints use cursor-based pagination", but the implementation uses
   page-based `{items, total, page, per_page}` (made canonical in
   `domain_boundaries_20260530`). This phase resolves the divergence toward the
   guidelines. **NOTE:** this reverses a deliberate prior decision and is the
   heaviest phase — gated behind a `tech-stack.md` design note.
5. **Fresh-DB reset tooling** (`feature-flag-7rp`) — the dev Postgres DB drifts
   from branch migrations (baseline checksum mismatch + unapplied pendings),
   making local verification unreliable. Needs a documented, automated reset path.

No new end-user product surface — this hardens existing behavior, correctness, and
dev ergonomics.

## Functional Requirements

### FR-1 — Idempotency-Key middleware (gateway, REST admin mutations)
- FR-1.1 A new tower/axum middleware layer intercepts **all mutating methods**
  (POST, PUT, PATCH, DELETE) on `/v1/*` admin routes. Read methods (GET/HEAD)
  bypass entirely.
- FR-1.2 The `Idempotency-Key` request header is **honored when present**. When
  absent, the request proceeds normally (header is optional, not forced — avoids
  breaking existing clients/tests).
- FR-1.3 Scope key uniqueness by **(actor identity, idempotency key, request
  fingerprint)** where actor = authenticated user/org from the existing auth
  middleware, and fingerprint = hash(method + path + canonical body).
- FR-1.4 **First request:** execute the handler, then persist
  `(scope, key, request_hash, response_status, response_body, created_at)` before
  returning. Concurrent in-flight duplicates of the same key serialize (via a PG
  row lock / `INSERT … ON CONFLICT`) so only one handler runs.
- FR-1.5 **Replayed request (same key + same fingerprint):** return the stored
  status + body verbatim, with an `Idempotent-Replayed: true` response header. The
  handler is **not** re-executed.
- FR-1.6 **Key reuse with a different fingerprint:** return **422 Unprocessable
  Entity** (`code: idempotency_key_reuse`) — the key was already used for a
  different request.
- FR-1.7 Only **successful** responses (2xx) are stored for replay; 4xx/5xx
  responses release the key so the client may legitimately retry. (Default:
  persist 2xx only.)

### FR-2 — Idempotency store + sweeper
- FR-2.1 New PG table `idempotency_keys` in a `stitchd-db` migration (`key`,
  `scope`, `request_hash`, `response_status`, `response_body jsonb`, `created_at`),
  unique on `(scope, key)`.
- FR-2.2 TTL = **24h**, configurable via `STITCHD_GATEWAY_IDEMPOTENCY_TTL_SECS`
  (default 86400). A periodic sweeper task (gateway tokio interval, mirroring
  existing background-task patterns) deletes rows past TTL.
- FR-2.3 The middleware degrades safely: a store error logs + fails **open**
  (request proceeds without idempotency protection) rather than 500-ing the API.

### FR-3 — On-demand interaction recompute (feature-flag-uga)
- FR-3.1 The `TriggerRecompute` path (`run_recompute` →
  `ExperimentRecomputer::recompute`) additionally runs `run_interaction_sweep` so
  `experiment_interactions` refreshes on demand, not only on the 60-min tick.
- FR-3.2 The on-demand sweep covers the candidate pairs/triples that include the
  recomputed experiment (or the full env sweep if simpler + correct), honoring
  `STITCHD_STATS_MAX_INTERACTION_ORDER`.
- FR-3.3 Requires wiring the ClickHouse reader/writer + interaction repo into the
  on-demand recomputer (the same deps the scheduled tick uses). A sweep failure
  marks the recompute job failed (consistent with stats-failure handling).

### FR-4 — Cursor-based pagination migration
> **Scope (Revision #1):** cursor applies to the **top-level resource
> collections** — flags, experiments, segments, events, metrics, sdk-keys,
> org-users (mgmt + admin), exclusion-groups (8 endpoints). **Experiment-detail
> sub-lists** (`iterations`, `exposures`) intentionally remain page-based: they
> back numbered detail views, and the Admin UI's exposure-count stat depends on
> the `total` the cursor envelope omits.
- FR-4.1 Document the cursor contract in `tech-stack.md` **before** implementation
  (per `workflow.md` principle 2) — opaque cursor token, `?cursor=&limit=`,
  response envelope `{items, next_cursor}`. ✅
- FR-4.2 Shared cursor primitives in the gateway (`CursorParams` + `CursorPage<T>`
  + `encode_cursor`/`decode_cursor`); opaque token = base64url(JSON(position)),
  treated as opaque by clients. ✅
- FR-4.3 **(Revised)** Deliver the cursor contract as an **opaque encoded-offset**
  at the gateway over each service's existing `(offset, limit) → (items, total)`
  RPC — `CursorParams::offset()` decodes the token to a start offset,
  `CursorPage::from_offset(items, total, start)` emits `next_cursor` iff more rows
  remain. **No proto or repo change.** *Originally specified as a per-repo keyset
  rewrite (`WHERE (sort_key, id) > cursor … LIMIT n+1`, dropping `OFFSET` +
  `COUNT(*) OVER()`); that true-keyset internal — which buys O(1) deep-page scans
  + concurrent-insert stability and swaps only the token payload, not the
  contract — is deferred to follow-up `feature-flag-cj5`. The
  `CursorPage::from_overfetch` primitive is in place for it.*
- FR-4.4 Migrate gateway list routes + the OpenAPI contract surface. ✅ (8 top-level
  list routes; the OpenAPI contract-check verifies `(method, path)` pairs, which
  are unchanged.)
- FR-4.5 Migrate Admin UI list views from `?page=N&per_page=M` to cursor tokens
  (next/prev navigation; page numbers dropped — cursors can't random-access). ✅
  (shared `usePaginatedList` + `Pagination` + 6 views.)

### FR-5 — SDK gRPC / event-ingest idempotency
- FR-5.1 Event-ingest (`/v1/sdk/events:batch` REST + the gRPC ingest path) accepts
  a per-batch idempotency/batch key; duplicate batches are deduped server-side
  (dedup ledger or ClickHouse ReplacingMergeTree dedup-key) so a retried flush does
  not double-count.
- FR-5.2 The Rust SDK stamps each flush batch with a stable idempotency key so its
  at-least-once `FlushTask` re-enqueue becomes exactly-once at the server.

### FR-6 — Fresh-DB reset tooling (feature-flag-7rp)
- FR-6.1 A documented, idempotent command (`cargo xtask db-reset` or
  `scripts/reset_dev_db.sh`) that drops + recreates the dev Postgres DB and re-runs
  all migrations from the V1 baseline, resolving baseline checksum drift.
- FR-6.2 Document the fresh-DB verification flow (README / `workflow.md`) so local
  verification matches CI's fresh-from-scratch-DB behavior.
- FR-6.3 (Optional) extend to ClickHouse + ScyllaDB reset for a full clean-slate
  local stack, if low-cost.

## Non-Functional Requirements
- NFR-1 **No `privateParameters` leak:** the idempotency store hashes request
  bodies for fingerprinting and persists response bodies — responses already
  exclude private params, and the request fingerprint is a one-way hash (raw
  request body is NOT stored), so no new privacy surface.
- NFR-2 **Backward compatible (idempotency):** absent `Idempotency-Key` → unchanged
  behavior; existing tests/clients keep passing.
- NFR-3 **≥90% coverage** on new code (CI gate); TDD per `workflow.md`.
- NFR-4 **sqlx offline cache** regenerated for new `query!`s; **docs idempotent**
  (`cargo xtask docs` zero-diff); new env var documented in the env-vars scraper.
- NFR-5 Interaction-sweep on-demand path adds no behavior change to the scheduled
  tick (shared code path, no divergence).
- NFR-6 The cursor migration is a **breaking API change** to the top-level list
  endpoints — applied atomically across gateway + Admin UI within the phase;
  OpenAPI contract + `domain_boundaries`-era assumptions updated in lockstep.
  (Revision #1: keeping detail sub-lists page-based avoids breaking the
  exposure-count stat while the top-level contract flips.)

## Acceptance Criteria
- AC-1 `POST /v1/projects/{}/flags` twice with the same `Idempotency-Key` +
  identical body creates **one** flag; the second call returns the first response
  with `Idempotent-Replayed: true`.
- AC-2 Same key + different body → **422 idempotency_key_reuse**.
- AC-3 No `Idempotency-Key` header → behaves exactly as today.
- AC-4 Rows in `idempotency_keys` older than the TTL are swept.
- AC-5 `TriggerRecompute` for an experiment that overlaps another refreshes the
  relevant `experiment_interactions` rows (live-CH integration test: seed two
  overlapping experiments → trigger recompute → assert interaction rows written
  without waiting for the tick).
- AC-6 A retried event batch with the same batch key does not double-count in
  ClickHouse (live-CH integration test).
- AC-7 **Top-level** list endpoints accept `?cursor=&limit=` and return
  `{items, next_cursor}`; paging through with the returned cursor yields every row
  exactly once with no duplicates/gaps (under stable data); Admin UI list views
  navigate via next/prev cursors. (Detail sub-lists iterations/exposures remain
  page-based — see FR-4 scope.)
- AC-8 The DB-reset command takes a drifted dev DB to a clean, fully-migrated state
  in one non-interactive command; documented.
- AC-9 CI green: workspace tests, clippy `-D warnings`, fmt, sqlx-check, docs
  idempotent, OpenAPI contract, admin vitest/tsc/lint/build.

## Out of Scope
- Streaming flag sync / server-pushed updates (separate future track).
- Client-side (browser/mobile) SDKs.
- Warehouse-backed event ingestion.
