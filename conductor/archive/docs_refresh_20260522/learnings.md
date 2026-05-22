# Track Learnings: docs_refresh_20260522

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

Project-wide patterns are in [`conductor/patterns.md`](../../patterns.md). Most relevant to
this track:

- **xtask layout** — `crates/xtask/src/main.rs` already orchestrates proto-doc gen,
  OpenAPI export, SDK rustdoc extract, mdbook build. Extensions in Phase 2 layer on top of
  this; don't restructure.
- **Parallel worker file-ownership boundary** — workflow.md "File-ownership boundary in
  worker prompts" section. Phase 3 workers MUST receive explicit owned/forbidden file lists.
- **Beads close gotcha** — `bd close --no-auto` is unreliable in current Beads; use plain
  `bd close <id>` and `--force` on phantom-dep errors. Documented in
  `conductor/patterns.md` "Experimentation Patterns" section.
- **`cargo sqlx prepare -- --tests`** — N/A for this track (no sqlx changes expected) but
  keep in mind if any task touches sqlx macros.

## Inherited from prior docs tracks

Seeded from `conductor/archive/mdbook_docs_20260418/` and
`conductor/archive/docs_microservices_20260422/`:

- mdBook source lives at `docs/src/`; `SUMMARY.md` is the canonical TOC. Any new page must
  be added to `SUMMARY.md` or it becomes orphan.
- `docs/src/grpc/*` is auto-generated; never hand-edit. Run `cargo xtask docs` to regen.
- `docs/openapi-pre-decomposition.json` is a frozen reference used by
  `scripts/check_openapi_contract.py` contract-check CI job — do NOT delete without
  understanding the contract check.

---

<!-- Learnings from implementation will be appended below -->

## 2026-05-22 — Phase 1 Discovery findings

### Doc inventory headline numbers
- 39 `.md` files in `docs/src/` (after orphan delete: 35)
- 5 orphans (4 under `internal/` + `api/rest.md`) — all deleted in Tasks 1.2 + 1.3
- 13 crate-level READMEs; 2 crates MISSING readmes (`stitchd-analytics-service`,
  `stitchd-stats-service`) — `cargo rdme` in Task 2.2 will create these.
- 17 narrative pages remaining to refresh in Phase 3 across topics A/B/C/D (Topic E spot-check only)

### Gitignore facts
- `docs/src/grpc/*.md` (except `README.md`) is gitignored — pure build artifacts.
- `docs/src/api/openapi.json` is gitignored — pure build artifact.

### Generator stack (already wired in `crates/xtask/src/main.rs`)
1. `generate_grpc_docs()` — scans `proto/*.proto`, writes domain-grouped Markdown
   to `docs/src/grpc/<name>.md`; rewrites `# Internal gRPC Services` section of `SUMMARY.md`.
2. `export_openapi()` — `cargo build -p stitchd-gateway` + run binary with
   `--export-openapi docs/src/api/openapi.json`. Reads `#[utoipa::path]` annotations.
3. `generate_sdk_rustdoc()` — `cargo doc --no-deps -p stitchd-sdk-rust` → copy to
   `docs/book/rustdoc/`; extract `# Quickstart` from `sdks/rust/src/lib.rs` `//!` →
   `docs/src/sdk/quickstart.md`.
4. `mdbook_build()` — `mdbook build docs/` → `docs/book/`.

### Contract-check load-bearing file
- `docs/openapi-pre-decomposition.json` is used by `scripts/check_openapi_contract.py`.
  KEEP — do NOT delete. Documents intentional surface gaps from
  `boundaries_20260518` canonical-URL refactor.

### Baseline snapshot (post-xtask run)
- `/tmp/docs_refresh_baseline_20260522/` contains all generator outputs as they were
  immediately after `cargo xtask docs` against commit `fcf204c`:
  - `grpc/` (14 files: 1 README + 13 per-proto pages, gitignored)
  - `quickstart.md` (auto-extracted from `sdks/rust/src/lib.rs`)
  - `openapi.json` (147KB, gitignored, exported by `stitchd-gateway --export-openapi`)
- Phase 2 must produce zero diff against this snapshot for the existing generators.

### Discovered out-of-scope warnings (filed inline for follow-up)
1. **3 rustdoc warnings** in `sdks/rust/src/client.rs:19–21` about public docs linking to
   private items (`GrpcDefinitionFetcher`, `HttpMembershipFetcher`, `HttpEventSink`).
   Either make the items `pub`, or update the doc-comments to use non-link form.
   Filed as `feature-flag-0yf` discovered-during note; will be addressed if it lands
   naturally during Task 3.4 (SDK landing).
2. **2 mdbook warnings** about unclosed HTML tags `<context>` and `<contextpreviewresult>`
   in `docs/src/grpc/flags_v1_flag_service.md`. These come from the proto-md generator
   (`crates/xtask/src/main.rs::proto_to_markdown`) not escaping angle-bracket type names
   inside table cells. Fix: wrap type names in backticks (already done for some, missed
   for these). To address in Task 2.3 or 2.4 alongside the link-checker work.

### Phase 3 file ownership table (for parallel workers)
| Worker | Topic | Files |
|--------|-------|-------|
| A | Intro + Architecture | `docs/src/introduction.md`, `docs/src/architecture/{README,multi-tenancy,evaluation-flow,data-stores,events,metrics,service-flows}.md` |
| B | Gateway | `docs/src/gateway/{overview,sdk-api,admin-api,grpc,openapi}.md` |
| C | Deployment (minus env-vars) | `docs/src/deployment/{README,postgres,clickhouse,scylladb,sdk-keys}.md` |
| D | SDK | `docs/src/sdk/README.md` + `sdks/rust/src/lib.rs` `//!` Quickstart section |
| E | Experimentation spot-check | `docs/src/experimentation/{index,attribution,default-rule-experiments}.md` |

## 2026-05-22 — Phase 3 narrative-worker findings

5 parallel topic workers (A/B/C/D run as Task subagents; E spot-check done inline).
File-ownership boundaries were respected — zero merge conflicts. Each worker
surfaced real drift / latent bugs while reading source:

### Worker A — Intro + Architecture
- **Gateway-as-trust-boundary**: backend services REQUIRE the `x-env-id` gRPC metadata
  header and return `Unauthenticated` without it (`sdks/spec/proto/sdk/v1/backend.proto`).
  The SDK key never leaves the gateway.
- **Prometheus exposition** is served at `GET /metrics` on the gateway's main HTTP port
  (8080), NOT on the separate 9080 port that docker-compose exposes. The 9080 port is
  exposed in compose but no listener is bound — dead config.
- **`experiment_iterations_active` CH dictionary TTL** is 30-60s (per the live migration),
  not 300-600 as some reference docs claimed.
- **Events `missing_contexts` rejection** is enforced server-side at ingest — empty
  `contexts` map is rejected; downstream attribution depends on this.
- **`experiment_assignments` first-exposure trick**: ReplacingMergeTree with
  `_version = -toUnixTimestamp64Milli(assigned_at)` so `MAX(_version)` = earliest
  assignment. Elegant; documented.

### Worker B — Gateway
- **No REST `/v1/sdk/flags:sync`** — flag-sync is **gRPC-only** on port 50050
  (`SdkService::SyncDefinitions` from `sdks/spec/proto/sdk/v1/service.proto`).
  REST `/v1/sdk/*` covers only `segments/list:batch` and `events:batch`.
- **OpenAPI declares ghost routes**: `events::ingest_event` + `events::ingest_batch`
  (`POST /v1/environments/{id}/events` + `/events/batch`) are in `ApiDoc` and a
  `test_router()` but NOT wired into `build_router`. Omitted from admin-api.md.
- **`check_openapi_contract.py` compares against the FROZEN snapshot**
  (`docs/openapi-pre-decomposition.json`), NOT the live `docs/src/api/openapi.json`.
  The prior `openapi.md` was misleading here.
- **`/v1/auth/me/orgs` is PUBLIC** (no auth middleware) but `/v1/auth/me/permissions`
  is JWT-tier — surprising asymmetry; both listed accurately.
- **`stitchd-auth-service` hosts 5 tonic services on port 50051**: `AuthService`,
  `ManagementService`, `AuthProviderService`, `OidcLoginService`, `SamlLoginService`.

### Worker C — Deployment
1. **CH migrations NOT auto-applied on service boot**. `event_writer::migrations::run`
   is invoked only from integration tests; analytics-service `main.rs` constructs the
   CH client and starts serving. On a fresh deploy the first event ingestion fails with
   `UNKNOWN_TABLE` until migrations are run out-of-band. No documented runner exists.
   **Production gap** — worth a follow-up bead.
2. **CH `experiment_iterations_active` dictionary hard-codes `host.docker.internal`**
   as its Postgres source. Works in compose-dev; breaks any production deploy where CH
   and PG live on different hosts. Operators must patch the migration or
   `ALTER DICTIONARY` after first apply.
3. **CH migrations split across 3 dirs**: `crates/stitchd-event-writer/migrations/`
   (live), `crates/stitchd-analytics-service/clickhouse-migrations/` (live, `experiment_results`
   only), and `crates/stitchd-db/clickhouse-migrations/` (reference-only, NOT wired into
   any runner). The track instructions pointed to the reference-only dir; docs corrected.
4. **PG migrations MUST run before CH** — `experiment_iterations_active` dict pulls from
   a PG view created in `20260521000004_v_experiment_iterations_active.sql`. Out-of-order
   migration causes a silent dict-load failure and the attribution MV becomes a no-op.
5. **Scylla keyspace bootstraps with `SimpleStrategy RF=1`** — fine for dev, must be
   `ALTER`ed to NetworkTopologyStrategy for prod.
6. **Sweeper env vars were renamed**: docs used `SWEEPER_RETENTION_SECS`; real vars are
   `STITCHD_SEGMENTATION_SWEEPER_RETENTION_SECS` / `_INTERVAL_SECS`. Track-instructions
   also stale — picked up automatically from current source by the env-vars generator.
7. **SDK key prefix `sdk_live_*` is a docs convention, not enforced** — the live generator
   emits a raw 64-char hex token via `generate_opaque_token()`. The auth path hashes the
   whole string. Documented honestly.

### Worker D — SDK
- Prior Quickstart was BROKEN: referenced `EvalRequest::flag(...)` (no such constructor)
  and `..Default::default()` on `SdkConfig` (no `Default` impl exists). Rewritten to
  match the real API: struct-literal `EvalRequest`, `SdkConfig::new(gateway_url, sdk_key)`,
  `client.shutdown(Duration::from_secs(5))`.
- The single example `sdks/rust/examples/live_verify.rs` compiles cleanly against current
  code (verified `cargo check --example live_verify`).
- 3 rustdoc private-link warnings in `sdks/rust/src/client.rs:19–21` cleaned up by
  switching `[`Ident`]` form to backtick `` `Ident` `` form. `cargo doc -p stitchd-sdk-rust`
  now produces ZERO warnings.

### Link-check follow-up on Worker D output
Worker D's `sdk/README.md` had 7 broken-link issues caught by the link checker:
- `../rustdoc/stitchd_sdk_rust/index.html` — extra `stitchd_sdk_rust/` segment (the
  cargo-doc copy flattens that). Corrected to `../rustdoc/index.html`.
- 6 `../../sdks/spec/docs/*.md` links should be `../../../sdks/spec/docs/*.md`
  (off by one — `docs/src/sdk/` is 3 deep from root, not 2). Corrected via sed.

The link-checker (Task 2.3) caught both issues automatically; this validates the
checker's value before it even hits CI.

### Follow-up beads to file (production-affecting drift surfaced during Phase 3)
- CH migrations not auto-run on service boot (Worker C #1 above) — needs a binary
  `cargo xtask ch-migrate` parallel to the existing `scylla-migrate`.
- CH dict hard-codes `host.docker.internal` (Worker C #2) — needs templating from env.
- `events::ingest_event` ghost routes in OpenAPI (Worker B #2) — either wire them up
  or remove from `ApiDoc`.
- Dead 9080 metrics port in docker-compose (Worker A) — drop the port mapping.
