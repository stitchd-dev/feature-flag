# Track Learnings: platform_hardening_20260608

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

From `conductor/patterns.md` — directly relevant to this track:

- **REST pagination is currently `page`+`per_page` (1-based) → `PaginatedResponse<T>`
  (`{items,total,page,per_page}`)** via shared `gateway::pagination`; internal CH list
  RPCs use `offset`+`limit` at the proto level. **Phase 4 supersedes this pattern** —
  update `patterns.md` line ~214 when the cursor contract lands. (from: domain_boundaries_20260530)
- **Pagination total without second query:** `COUNT(*) OVER()` window function returns
  total in every row. Keyset/cursor pagination drops the total — Phase 4 must decide
  whether `total` is still surfaced or removed from the envelope. (from: db_optim_20260516)
- **Gateway = pure REST↔gRPC translation + cross-cutting, ZERO domain logic.** Idempotency
  middleware (Phase 1) is a legitimate cross-cutting gateway concern (like auth/quota/tracing) —
  it belongs in the gateway, not a service. (from: domain_boundaries_20260530)
- **Migration baseline synthesis: `IF NOT EXISTS` throughout, drop-aware.** Relevant to
  Phase 5 DB-reset tooling and the Phase 1 `idempotency_keys` migration — fresh deploys must
  apply cleanly from the V1 baseline. (from: schema_cutover_20260525)
- **Single-command doc pipeline + idempotency CI gate.** New env var
  (`STITCHD_GATEWAY_IDEMPOTENCY_TTL_SECS`) must flow through the env-vars scraper; run
  `cargo xtask docs && git diff --exit-code`. (from: docs_refresh_20260522)
- **sqlx offline cache discipline:** after adding/removing `query!`s, run
  `SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets --features
  stitchd-sdk-rust/test-util`. (from: workflow.md)
- **Live-CH stats tests use an EXPLICIT CI `--test` list** in `.github/workflows/ci.yml`
  (Coverage job, "Live-ClickHouse integration tests (stats-service)" step). Phase 3's new
  self-seeding test file MUST be added to that list or CI goes red on next push, invisible
  to local `cargo test --workspace`. (from: seqtest_20260603 / ci-live-ch memory)

## Track-Specific Context

- **feature-flag-uga** (Phase 3): on-demand `run_recompute` (`grpc/service.rs:185`) drives
  per-experiment stats + bandit reallocation via the `ExperimentRecomputer` trait but never
  calls `run_interaction_sweep` (`interaction_compute.rs:934`). The on-demand recomputer
  lacks the CH reader/writer + interaction repo the scheduled tick (`main.rs:422`) holds.
- **feature-flag-7rp** (Phase 5): dev Postgres drifted — baseline checksum mismatch +
  unapplied pendings. CI runs against a fresh-from-scratch DB, so local verification must
  match that to catch issues CI catches.
- **Idempotency privacy (NFR-1):** store hashes the request body for fingerprinting (one-way,
  raw body NOT persisted) and stores the response body (already private-param-free). No new
  `privateParameters` leak surface.

---

<!-- Learnings from implementation will be appended below -->
