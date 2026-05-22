# Stitchd Feature Flag

[![CI](https://github.com/stitchd-dev/feature-flag/actions/workflows/ci.yml/badge.svg)](https://github.com/stitchd-dev/feature-flag/actions/workflows/ci.yml)
[![Backend coverage](https://codecov.io/gh/stitchd-dev/feature-flag/branch/main/graph/badge.svg?flag=backend&label=backend)](https://codecov.io/gh/stitchd-dev/feature-flag/flags?flag=backend)
[![Frontend coverage](https://codecov.io/gh/stitchd-dev/feature-flag/branch/main/graph/badge.svg?flag=admin&label=frontend)](https://codecov.io/gh/stitchd-dev/feature-flag/flags?flag=admin)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

A production-ready feature flag and experimentation platform built in Rust. Supports multi-tenant deployments, rule-based flag evaluation, audience segmentation, and server-side SDK integration via gRPC.

**[Documentation](https://stitchd-dev.github.io/feature-flag/)** · [REST API](https://stitchd-dev.github.io/feature-flag/api/rest.html) · [gRPC Reference](https://stitchd-dev.github.io/feature-flag/grpc/) · [Rust SDK](https://stitchd-dev.github.io/feature-flag/sdk/)

---

## Features

- **Rule-based evaluation** — Evaluate flags in-process using user attributes, percentage rollouts, and segment membership
- **Multi-tenancy** — Isolated projects and organisations with role-based access control (RBAC)
- **Server-side SDK** — Rust SDK with background gRPC polling, LFU cache for list-based segments, and thread-safe evaluation
- **Admin REST API** — Full CRUD for flags, variants, segments, environments, and SDK keys; OpenAPI schema included
- **gRPC flag sync** — Low-latency definition streaming to connected SDKs
- **Event tracking** — ClickHouse backend for experiment metrics (in progress)
- **Observability** — OpenTelemetry traces, Prometheus metrics, structured logging via `tracing`
- **PostgreSQL backend** — Compile-time verified queries via `sqlx`, schema migrations included

---

## Workspace Layout

```
crates/
  stitchd-core/                 # Domain models, rule engine, hashing, ID types
  stitchd-db/                   # PostgreSQL + ClickHouse repositories (sqlx)
  stitchd-events/               # ClickHouse event ingestion library
  stitchd-proto/                # Protobuf definitions and tonic stubs for all gRPC services
  stitchd-sdk/                  # Rust server-side SDK (in-process flag evaluation)
  stitchd-gateway/              # REST + gRPC gateway — single entry point for all clients
  stitchd-auth-service/         # Authentication & management gRPC service (:50051)
  stitchd-flag-service/         # Flag definitions, evaluation, SDK sync gRPC service (:50052)
  stitchd-segmentation-service/ # Segment membership gRPC service (:50053)
  stitchd-event-service/        # Experimentation event ingestion gRPC service (:50054)
  stitchd-experimentation-service/ # Experiment management gRPC service (:50055)
  xtask/                        # Build tool (mdBook docs generation)
docs/                           # mdBook source — published to GitHub Pages
```

---

## Quick Start

### Prerequisites

- Rust 1.85+
- PostgreSQL 16
- ClickHouse (optional, required for event tracking)
- Docker (for running dependencies locally)

### Run Locally

```bash
# 1. Copy environment config
cp .env.example .env

# 2. Start all services (PostgreSQL, ClickHouse, and all microservices)
docker-compose up -d

# 3. (Optional) Run individual services against local databases
#    Each service reads DATABASE_URL from the environment.
cargo run -p stitchd-gateway
```

The REST admin/SDK API is available at `http://localhost:8080`.
The gRPC flag sync service for SDKs is at `localhost:50050` (proxied by the gateway to `flag-service`).

Internal service ports (not normally exposed to clients directly):

| Service | Default Port |
|---------|-------------|
| `stitchd-auth-service` | `50051` |
| `stitchd-flag-service` | `50052` |
| `stitchd-segmentation-service` | `50053` |
| `stitchd-event-service` | `50054` |
| `stitchd-experimentation-service` | `50055` |

---

## SDK Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
stitchd-sdk = { git = "https://github.com/stitchd-dev/feature-flag" }
```

Initialize the client once at application startup and evaluate flags per request:

```rust
use stitchd_sdk::{SdkClient, SdkConfig, Context, EvaluationContext};

#[tokio::main]
async fn main() {
    let config = SdkConfig::new(
        "http://localhost:50050",  // gRPC flag-sync endpoint (gateway)
        "http://localhost:8080",   // REST endpoint for list-segment checks (gateway)
        "sdk_live_...",            // SDK key from the admin API
    );

    // Initializes definitions and starts background polling
    let client = SdkClient::init(config).await.expect("SDK init failed");

    // Evaluate a flag for a user context
    let ctx = EvaluationContext {
        contexts: vec![Context::new("user", "user-123")],
    };

    if let Some(variant) = client.evaluate("my-feature-flag", &ctx).await.expect("evaluation failed") {
        println!("Flag value: {:?}", variant);
    }
}
```

See the [SDK documentation](https://stitchd-dev.github.io/feature-flag/sdk/quickstart.html) for full configuration options, LFU cache tuning, and segment pre-warming.

---

## Architecture

```
                      ┌──────────────────────────────────────────┐
                      │           stitchd-gateway                │
  Admin / SDK ───────▶│  REST :8080   |   gRPC FlagSync :50050   │
                      └─────┬──────────────────────┬────────────┘
                            │ gRPC                 │ gRPC (proxy)
           ┌────────────────┼──────────────────────┼──────────────┐
           ▼                ▼                       ▼              ▼
  auth-service         flag-service        segmentation-    event-service
  :50051               :50052 (+ FlagSync) service :50053   :50054
  (Auth + Mgmt)        (Flags, Variants,   (Segment         (Event
                        SDK sync)           membership)      ingestion)
           │                │                       │              │
           └───────────────────────────────────────────────────────┘
                            │ sqlx                  │ ClickHouse
                     ┌──────┴──────┐         ┌──────┴──────┐
                     │ PostgreSQL  │         │ ClickHouse  │
                     │ (config)    │         │ (events)    │
                     └─────────────┘         └─────────────┘

  stitchd-sdk (in your app)
    ├── gRPC SyncDefinitions ──▶ gateway :50050 ──▶ flag-service
    └── REST list-segment check ▶ gateway :8080  ──▶ segmentation-service
```

- All external traffic enters through `stitchd-gateway`; backend services are not exposed directly.
- Flag definitions are stored in PostgreSQL and streamed to SDKs via gRPC; evaluation happens in-process in the SDK.
- List-based segment membership is resolved via REST with an LFU cache in the SDK to minimise round-trips.
- ClickHouse handles experiment event ingestion separately from the configuration path.

For a deeper dive see [Architecture Overview](https://stitchd-dev.github.io/feature-flag/architecture/).

---

## Development

```bash
# Run all checks (format, clippy, tests, coverage)
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Build documentation locally
cargo run --manifest-path crates/xtask/Cargo.toml -- docs
# Opens docs/book/index.html

# Run tests with coverage (requires cargo-tarpaulin)
cargo tarpaulin --workspace --out Xml
```

Coverage is enforced at **90%** in CI and reported to [Codecov](https://codecov.io/gh/stitchd-dev/feature-flag).

### Environment Variables

#### Gateway (`stitchd-gateway`)

| Variable | Default | Description |
|---|---|---|
| `GATEWAY_PORT` | `8080` | REST API listen port |
| `METRICS_PORT` | `9080` | Prometheus metrics port |
| `AUTH_SERVICE_ADDR` | `http://localhost:50051` | Auth service gRPC address |
| `FLAG_SERVICE_ADDR` | `http://localhost:50052` | Flag service gRPC address |
| `SEGMENTATION_SERVICE_ADDR` | `http://localhost:50053` | Segmentation service gRPC address |
| `EVENT_SERVICE_ADDR` | `http://localhost:50054` | Event service gRPC address |
| `EXPERIMENTATION_SERVICE_ADDR` | `http://localhost:50055` | Experimentation service gRPC address |

#### Auth Service (`stitchd-auth-service`)

| Variable | Default | Description |
|---|---|---|
| `AUTH_SERVICE_PORT` | `50051` | gRPC listen port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `JWT_SECRET` | — | Secret for JWT signing (required) |
| `SUPERADMIN_EMAIL` | — | Seed superadmin email on first boot |
| `SUPERADMIN_PASSWORD` | — | Seed superadmin password (hashed with Argon2id) |

#### Flag Service (`stitchd-flag-service`)

| Variable | Default | Description |
|---|---|---|
| `FLAG_SERVICE_PORT` | `50052` | gRPC listen port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |

#### Segmentation Service (`stitchd-segmentation-service`)

| Variable | Default | Description |
|---|---|---|
| `SEGMENTATION_SERVICE_PORT` | `50053` | gRPC listen port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |

#### Event Service (`stitchd-event-service`)

| Variable | Default | Description |
|---|---|---|
| `EVENT_SERVICE_PORT` | `50054` | gRPC listen port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `CLICKHOUSE_URL` | — | ClickHouse HTTP connection string (required) |

#### Experimentation Service (`stitchd-experimentation-service`)

| Variable | Default | Description |
|---|---|---|
| `EXPERIMENTATION_SERVICE_PORT` | `50055` | gRPC listen port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `FLAG_SERVICE_ADDR` | `http://localhost:50052` | Flag service gRPC address |

#### All Services

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Log filter (e.g. `debug,sqlx=warn`) |

See `.env.example` and `docker-compose.yml` for a complete reference.

---

## License

Licensed under the [MIT License](LICENSE).