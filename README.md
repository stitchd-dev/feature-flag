# Stitchd Feature Flag

[![CI](https://github.com/stitchd-dev/feature-flag/actions/workflows/ci.yml/badge.svg)](https://github.com/stitchd-dev/feature-flag/actions/workflows/ci.yml)
[![Backend coverage](https://codecov.io/gh/stitchd-dev/feature-flag/branch/main/graph/badge.svg?flag=backend&label=backend)](https://codecov.io/gh/stitchd-dev/feature-flag/flags?flag=backend)
[![Frontend coverage](https://codecov.io/gh/stitchd-dev/feature-flag/branch/main/graph/badge.svg?flag=admin&label=frontend)](https://codecov.io/gh/stitchd-dev/feature-flag/flags?flag=admin)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.95+](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org)

A production-ready feature-flag and experimentation platform built in Rust. Multi-tenant, self-hosted, with rule-based flag evaluation, audience segmentation, eval-log-driven first-exposure experiment attribution, and a React admin console.

**[Documentation](https://stitchd-dev.github.io/feature-flag/)** · [REST API](https://stitchd-dev.github.io/feature-flag/api/rest.html) · [gRPC Reference](https://stitchd-dev.github.io/feature-flag/grpc/) · [Rust SDK](https://stitchd-dev.github.io/feature-flag/sdk/) · [Experimentation](https://stitchd-dev.github.io/feature-flag/experimentation/)

---

## Features

- **Flags** — Typed flags (`bool` / `int` / `double` / `string` / `json`), variants, ordered rule list with AND/OR/NOT condition trees, percentage rollouts with 0.1% granularity, "Is in Segment" + "Flag evaluated with variant X" rule kinds, and a server-side **Evaluate-Preview** with rule traces + rollout debug info.
- **Segmentation** — Rule-based segments (condition expression builder) AND list-based segments (per-context-type include/exclude key lists) backed by ScyllaDB wide-row storage for million-scale memberships with LWT-based generation swaps.
- **Events + Metrics** — Pre-registered events with multi-context attribution (`{type: key, ...}` flat-map in `events_v2`), composable metric primitives (Aggregation / Ratio / Funnel) with JsonLogic `where_clause` filters, and ClickHouse-backed metric preview.
- **Experimentation** — First-exposure (intent-to-treat) attribution derived server-side from `flag_evaluation_log` via a `experiment_assignments_mv` materialized view; rule-scoped + context-type-scoped; whole-flag lock while running; default-rule percentage distribution; Frequentist (Welch's t, Bonferroni) + Bayesian (Beta-Binomial, Normal-Normal, PtB) + CUPED variance reduction + SRM detection + guardrail metrics.
- **Multi-tenancy** — Organisations → projects → environments → SDK keys; role-based access control (RBAC), audit log on every mutation, optimistic concurrency on every entity.
- **Auth** — JWT (jsonwebtoken 10) + OAuth2/OIDC (openidconnect 4) + SAML 2.0 + MFA (TOTP) + invites + Argon2id password hashing + smart-IP rate limiting.
- **Server-side Rust SDK** — `stitchd-sdk-rust` with background gRPC definition polling, in-process rule evaluation, LRU membership cache for list-based segments, batched event emission via `client.track()`, and a `test-util` feature for conformance testing.
- **Admin REST API + Admin UI** — Full CRUD over a React 19 + Vite 8 + TypeScript 6 console (`admin/`): flags, variants, rule builder, segments, events, metrics, experiments (with per-context-type results, SRM panel, time-series, iteration history), environments, SDK keys, org users, audit log.
- **Observability** — OpenTelemetry traces (0.32), Prometheus metrics, structured logging via `tracing`.
- **Storage** — PostgreSQL 16+ (config), ClickHouse 24+ (events / experiment results / eval log MVs), ScyllaDB 6+ (list-segment storage). Compile-time SQL via `sqlx` 0.8.

---

## Workspace Layout

```
crates/
  stitchd-core/                       # Domain models, rule engine, hashing, ID types, stats math (SRM/Frequentist/Bayesian/CUPED)
  stitchd-db/                         # PostgreSQL + ClickHouse + ScyllaDB repositories (sqlx + clickhouse-rs + scylla)
  stitchd-event-writer/               # ClickHouse event ingestion + migration runner (library)
  stitchd-proto/                      # Protobuf definitions + tonic stubs for all gRPC services
  stitchd-gateway/                    # REST + gRPC gateway — single trust boundary, holds gRPC client channels only
  stitchd-auth-service/               # JWT / SDK-key credential validation, RBAC context assembly (:50051)
  stitchd-flag-service/               # Flag CRUD, server-streaming definition sync, evaluate-preview, whole-flag lock (:50052)
  stitchd-segmentation-service/       # Segment CRUD, rule + list membership evaluation (ScyllaDB-backed) (:50053)
  stitchd-analytics-service/          # Event-definition CRUD, event ingestion, metric-definition CRUD + preview (:50054)
  stitchd-experimentation-service/    # Experiment lifecycle, iteration management, results read path (:50055)
  stitchd-stats-service/              # Scheduled stats computation (60-min ticker), ClickHouse query builders, TriggerRecompute RPC (:50056)
  xtask/                              # Build tool — `cargo run --package xtask -- docs` builds the mdBook site
sdks/
  rust/                               # `stitchd-sdk-rust` — server-side Rust SDK (in-process eval + polling + LRU cache)
  spec/                               # SDK contract specs (proto, OpenAPI, fixtures) — language-neutral
admin/                                # React 19 + Vite 8 admin console (TypeScript 6, Formik + Yup, vitest)
docs/                                 # mdBook source — published to GitHub Pages
```

---

## Quick Start

### Prerequisites

- Rust 1.95+ (workspace MSRV; CI tracks `stable`)
- Docker (PostgreSQL 16, ClickHouse 24, ScyllaDB 6 via `docker compose`)
- Node 20+ (only for the admin UI in `admin/`)

### Run Locally

```bash
# 1. Copy environment config
cp .env.example .env

# 2. Start everything (PostgreSQL, ClickHouse, ScyllaDB, all 6 backend services + gateway + admin UI)
docker compose up -d --wait

# 3. (Optional) Run an individual service against the running infra
cargo run -p stitchd-gateway
```

After `docker compose up`:

- **Admin UI:** `http://localhost:5173`
- **Gateway REST:** `http://localhost:8080`
- **Gateway Prometheus metrics:** `http://localhost:9080/metrics`
- **SDK gRPC sync:** `localhost:50050` (proxied by the gateway → `flag-service`)

Internal gRPC service ports (the gateway is the only public surface — backend services are not directly exposed):

| Service | Default Port |
|---|---|
| `stitchd-auth-service` (gRPC) | `50051` |
| `stitchd-flag-service` (gRPC) | `50052` |
| `stitchd-segmentation-service` (gRPC) | `50053` |
| `stitchd-analytics-service` (gRPC) | `50054` |
| `stitchd-experimentation-service` (gRPC) | `50055` |
| `stitchd-stats-service` (gRPC) | `50056` |

---

## SDK Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
stitchd-sdk-rust = { git = "https://github.com/stitchd-dev/feature-flag", package = "stitchd-sdk-rust" }
```

Initialise the client once at startup and evaluate per-request:

```rust
use stitchd_sdk_rust::{SdkClient, SdkConfig};
use stitchd_core::context::{Context, EvaluationContext};

#[tokio::main]
async fn main() {
    let config = SdkConfig::new(
        "http://localhost:50050",  // gateway gRPC SDK port
        "http://localhost:8080",   // gateway REST (for list-segment membership lookups)
        "sdk_live_...",            // SDK key from the admin UI
    );

    // Blocks until the first definition sync completes, then starts background polling.
    let client = SdkClient::init(config).await.expect("SDK init failed");

    // Evaluate a flag against a multi-context user.
    let ctx = EvaluationContext {
        contexts: vec![Context::new("user", "user-123")],
    };

    if let Some(variant) = client.evaluate("my-feature-flag", &ctx).await.expect("eval failed") {
        println!("Served variant: {variant:?}");
    }

    // Track a metric event for experiment attribution (multi-context flat-map).
    client.track("checkout_completed", &ctx, /* value = */ None, /* properties = */ None)
        .await
        .expect("track failed");
}
```

See the [SDK quickstart](https://stitchd-dev.github.io/feature-flag/sdk/quickstart.html) for full configuration, LRU cache tuning, batch event flush semantics, and the conformance test harness.

---

## Architecture

```
                          ┌────────────────────────────────────────────┐
                          │             stitchd-gateway                │
   Admin UI / SDK ───────▶│   REST :8080   |   gRPC SDK-sync :50050    │
                          └─────────────────────────┬──────────────────┘
                                                    │ gRPC (client channels only — gateway holds no DB conn)
        ┌─────────────────────┬───────────────────┬─┴─────────────────┬─────────────────────┬──────────────────────┐
        ▼                     ▼                   ▼                   ▼                     ▼                      ▼
  auth-service :50051   flag-service :50052   segmentation     analytics-service :50054   experimentation-     stats-service :50056
  (JWT, SDK keys,       (Flags, Variants,     :50053           (Events, Metrics,          service :50055        (Scheduled stats
   RBAC)                Eval, SDK sync,       (Rule + list     Preview, MV writes)        (Lifecycle, iter,     compute → CH
                        Whole-flag lock)      membership)                                  Results read path)    experiment_results)
        │                     │                   │                   │                     │                      │
        └─────────────────────┴───────────────────┴───────────┬───────┴─────────────────────┴──────────────────────┘
                                  PostgreSQL (sqlx)           │  ClickHouse        ScyllaDB
                                  ┌──────────────┐            │  ┌──────────────┐  ┌─────────────────┐
                                  │ config /     │            └─▶│ events_v2    │  │ list-segment    │
                                  │ tenants /    │               │ flag_eval_log│  │ wide-row entry  │
                                  │ experiments  │               │ exp_assignmts│  │ storage         │
                                  └──────────────┘               │ exp_results  │  └─────────────────┘
                                                                 └──────────────┘

  stitchd-sdk-rust (in your app)
    ├── gRPC SyncDefinitions ─▶ gateway :50050 ─▶ flag-service
    ├── REST list-segment      ─▶ gateway :8080 ─▶ segmentation-service (LRU-cached per (context_type, key))
    └── REST event track       ─▶ gateway :8080 ─▶ analytics-service (batched)
```

Key invariants:

- **Gateway is the sole trust boundary.** Backend services never validate SDK keys; they trust the `x-env-id` gRPC metadata header propagated by the gateway after key validation.
- **Whole-flag lock while experiment running.** Any flag/variant/rule/default-distribution mutation against a flag with a running or paused experiment returns HTTP `409 FLAG_LOCKED_BY_EXPERIMENT`. Stop the experiment to modify; restart creates a new iteration.
- **First-exposure (ITT) attribution.** A context's variant assignment is derived server-side from the FIRST `flag_evaluation_log` row matching the experiment's bound rule (or default-rule fall-through) within the iteration window. Pre-exposure events are excluded; re-exposures don't reassign.
- **Server-derived attribution = SDK-agnostic.** SDKs don't tag events with experiment context. Any future SDK (JS, iOS, Android) gets experiment attribution for free.

For a deeper dive see the [Architecture Overview](https://stitchd-dev.github.io/feature-flag/architecture/) and the experimentation [attribution model](https://stitchd-dev.github.io/feature-flag/experimentation/attribution.html).

---

## Development

The CI command is the authoritative source of every check. Mirror it locally:

```bash
# 1. Formatting
cargo fmt --all --check

# 2. SQLx offline cache integrity (live PG required — uses your dev DB)
SQLX_OFFLINE=false cargo sqlx prepare --workspace --check -- --all-targets

# 3. Linting (CI uses `--features stitchd-sdk-rust/test-util` so the conformance test compiles)
cargo clippy --workspace --all-targets --features stitchd-sdk-rust/test-util -- -D warnings

# 4. Tests + coverage (workspace, cobertura output for Codecov)
cargo install cargo-llvm-cov  # one-time
cargo llvm-cov \
  --workspace --exclude stitchd-proto --exclude xtask \
  --features stitchd-sdk-rust/test-util \
  --ignore-filename-regex 'main\.rs' \
  --cobertura --output-path coverage/cobertura.xml

# 5. Admin UI checks
(cd admin && npm ci && node_modules/.bin/tsc --noEmit -p tsconfig.app.json && npm run lint && npm run test:coverage)

# 6. mdBook docs
cargo run --manifest-path crates/xtask/Cargo.toml -- docs
# opens at docs/book/index.html
```

Coverage is enforced at **90%** for the Rust workspace (`backend` flag) and is informational-only for the admin UI (`admin` flag) — see [`codecov.yml`](./codecov.yml).

### Resetting the dev databases (un-drift)

CI provisions brand-new database containers on every run, so it always sees a
clean, fully-migrated schema. A long-lived local dev DB can **drift** from that
state — most commonly a "different checksum" on an already-applied migration
(e.g. the V1 baseline edited after it was first applied), which makes
`sqlx migrate run` halt and leaves later migrations pending. When that happens,
step 2 above (`cargo sqlx prepare`) and any `cargo test` against the dev DB
silently diverge from CI.

The fix is to drop and recreate the databases from the V1 baseline so local
matches CI fresh-from-scratch:

```bash
# Postgres only (the common case — un-drifts the migration history)
scripts/reset_dev_db.sh

# Postgres + ClickHouse + ScyllaDB (full clean slate)
scripts/reset_dev_db.sh --all
```

The script is **non-interactive and idempotent** (safe to re-run). It reads
`STITCHD_DATABASE_URL` (falling back to the docker-compose default) and derives
the plain `DATABASE_URL` that `sqlx-cli` needs. Verify afterwards with:

```bash
DATABASE_URL="$STITCHD_DATABASE_URL" \
  cargo sqlx migrate info --source crates/stitchd-db/migrations   # all "installed"
```

> The `--all` ClickHouse path issues a `DROP DATABASE … SYNC` and then sweeps any
> orphaned `Replicated*MergeTree` replica registrations out of Keeper before
> recreating — otherwise a previously-interrupted reset can leave the schema
> tables un-creatable (`REPLICA_ALREADY_EXISTS`). ClickHouse migrations are
> applied by `cargo xtask ch-migrate` (the canonical event-writer migration set).

### Environment Variables

All Stitchd-owned env vars carry the `STITCHD_` prefix (the only exception is `RUST_LOG`, which follows the Rust ecosystem standard). Service ports follow a predictable pattern: `STITCHD_{SERVICE}_GRPC_PORT` + `STITCHD_{SERVICE}_METRICS_PORT`.

> The authoritative, complete list is auto-generated at [`docs/src/deployment/env-vars.md`](docs/src/deployment/env-vars.md) by `cargo xtask docs`. The tables below show the most commonly-set variables.

#### Gateway

| Variable | Default | Description |
|---|---|---|
| `STITCHD_GATEWAY_HTTP_PORT` | `8080` | REST API listen port (also serves `/metrics` for Prometheus scrape) |
| `STITCHD_GATEWAY_GRPC_PORT` | `50050` | SDK gRPC sync listen port |
| `STITCHD_AUTH_SERVICE_ADDR` | `http://localhost:50051` | Auth service gRPC address |
| `STITCHD_FLAG_SERVICE_ADDR` | `http://localhost:50052` | Flag service gRPC address |
| `STITCHD_SEGMENTATION_SERVICE_ADDR` | `http://localhost:50053` | Segmentation service gRPC address |
| `STITCHD_ANALYTICS_SERVICE_ADDR` | `http://localhost:50054` | Analytics service gRPC address |
| `STITCHD_EXPERIMENTATION_SERVICE_ADDR` | `http://localhost:50055` | Experimentation service gRPC address |
| `STITCHD_STATS_SERVICE_ADDR` | `http://localhost:50056` | Stats service gRPC address |
| `STITCHD_EVENT_QUOTA_PER_SEC` | `1000` | Per-environment event ingestion quota |

#### Backend Services (shared)

| Variable | Required | Description |
|---|---|---|
| `STITCHD_DATABASE_URL` | ✓ | PostgreSQL connection string (`postgresql://stitchd:stitchd@localhost:5432/stitchd`) |
| `STITCHD_CLICKHOUSE_URL` | analytics, stats, flag, experimentation | ClickHouse HTTP URL (`http://localhost:8123`) |
| `STITCHD_CLICKHOUSE_USER` | with ClickHouse | `stitchd` |
| `STITCHD_CLICKHOUSE_PASSWORD` | with ClickHouse | `stitchd` |
| `STITCHD_CLICKHOUSE_DB` | with ClickHouse | `stitchd` |
| `STITCHD_SCYLLA_CONTACT_POINTS` | segmentation | ScyllaDB CQL contact points (`localhost:9042`) |
| `STITCHD_JWT_SECRET` | auth | JWT signing secret |
| `STITCHD_SUPERADMIN_EMAIL` | auth (first boot only) | Seed superadmin email |
| `STITCHD_SUPERADMIN_PASSWORD` | auth (first boot only) | Seed superadmin password (Argon2id-hashed at rest) |
| `STITCHD_{SERVICE}_GRPC_PORT` | per-service | gRPC listen port for each service (see table above) |
| `RUST_LOG` | optional | Log filter (e.g. `info,sqlx=warn`) |

> ℹ️  `sqlx-cli` requires a plain `DATABASE_URL` env var (not `STITCHD_DATABASE_URL`). Alias before running sqlx commands:
> `export DATABASE_URL="$STITCHD_DATABASE_URL"`.

See [`.env.example`](./.env.example) and [`docker-compose.yml`](./docker-compose.yml) for the complete reference.

---

## Project Status

| Module | Status |
|---|---|
| Core domain + rule engine + segmentation | ✅ Complete |
| Feature flags + evaluate-preview | ✅ Complete |
| Events + composable metrics (Aggregation/Ratio/Funnel) | ✅ Complete |
| Experimentation (eval-log attribution, per-context-type stats, default-rule, Frequentist/Bayesian/CUPED/SRM/Guardrails) | ✅ Complete |
| Server-side Rust SDK + conformance harness | ✅ Complete |
| Human Auth (JWT, password, OIDC, SAML, MFA, rate limiting) | ✅ Complete |
| Microservice decomposition (6 services + gateway) | ✅ Complete |
| Admin UI (Superadmin, Org, Flags, Segments, Events, Metrics, Experiments, RBAC) | ✅ Complete |
| ScyllaDB list-segment storage | ✅ Complete |
| ClickHouse event MVs, query optimisations | ✅ Complete |

See [`conductor/product.md`](./conductor/product.md) for the canonical status sheet and [`conductor/archive/`](./conductor/archive/) for the per-track delivery history.

---

## License

Licensed under the [MIT License](LICENSE).
