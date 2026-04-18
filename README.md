# Stitchd Feature Flag

[![CI](https://github.com/stitchd-dev/feature-flag/actions/workflows/ci.yml/badge.svg)](https://github.com/stitchd-dev/feature-flag/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/stitchd-dev/feature-flag/graph/badge.svg)](https://codecov.io/gh/stitchd-dev/feature-flag)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

A production-ready feature flag and experimentation platform built in Rust. Supports multi-tenant deployments, rule-based flag evaluation, audience segmentation, and server-side SDK integration via gRPC.

**[Documentation](https://stitchd-dev.github.io/feature-flag/)** · [REST API Reference](https://stitchd-dev.github.io/feature-flag/api-reference.html) · [Rust SDK](https://stitchd-dev.github.io/feature-flag/sdk/overview.html)

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
  stitchd-core/     # Domain models, rule engine, hashing, ID types
  stitchd-db/       # PostgreSQL repositories (sqlx)
  stitchd-events/   # ClickHouse event ingestion (WIP)
  stitchd-proto/    # Protobuf definitions for gRPC services
  stitchd-sdk/      # Rust server-side SDK
  stitchd-server/   # HTTP + gRPC server binary
  xtask/            # Build tool (mdbook docs generation)
docs/               # mdbook source — published to GitHub Pages
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

# 2. Start PostgreSQL (or point .env at an existing instance)
docker run -d \
  -e POSTGRES_USER=stitchd \
  -e POSTGRES_PASSWORD=stitchd \
  -e POSTGRES_DB=stitchd \
  -p 5432:5432 postgres:16

# 3. Build and run the server
cargo run -p stitchd-server
```

The HTTP admin API is available at `http://localhost:8080` and the gRPC flag sync service at `localhost:50051`.

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
    let config = SdkConfig::builder()
        .sdk_key("sdk-key-here")
        .server_url("http://localhost:50051")
        .build();

    // Initializes definitions and starts background polling
    let client = SdkClient::init(config).await.expect("SDK init failed");

    // Evaluate a flag for a user context
    let context = EvaluationContext::new(
        Context::user("user-123")
            .attribute("country", "US")
            .build(),
    );

    let variant = client.evaluate("my-feature-flag", &context).await;
    println!("Flag value: {:?}", variant);
}
```

See the [SDK documentation](https://stitchd-dev.github.io/feature-flag/sdk/quickstart.html) for full configuration options, LFU cache tuning, and segment pre-warming.

---

## Architecture

```
┌─────────────────┐    gRPC flag sync    ┌──────────────────┐
│  stitchd-server │ ──────────────────── │  stitchd-sdk     │
│  (HTTP + gRPC)  │                      │  (in-process     │
│                 │    REST (segments)   │   evaluation)    │
│  ┌───────────┐  │ ──────────────────── │                  │
│  │ PostgreSQL│  │                      └──────────────────┘
│  └───────────┘  │
│  ┌────────────┐ │
│  │ ClickHouse │ │  ← event metrics (WIP)
│  └────────────┘ │
└─────────────────┘
```

- Flag definitions are stored in PostgreSQL and streamed to SDKs via gRPC.
- Rule-based segments are evaluated entirely in-process; list-based segments are resolved via REST with an LFU cache.
- The admin REST API exposes full CRUD operations and an OpenAPI schema at `/api-docs/openapi.json`.

For a deeper dive see [Architecture Overview](https://stitchd-dev.github.io/feature-flag/architecture/overview.html).

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

| Variable | Default | Description |
|---|---|---|
| `HTTP_PORT` | `8080` | Admin REST API port |
| `GRPC_PORT` | `50051` | gRPC flag sync port |
| `DATABASE_URL` | — | PostgreSQL connection string |
| `CLICKHOUSE_URL` | — | ClickHouse connection string |
| `JWT_SECRET` | — | Secret for JWT signing |
| `RUST_LOG` | `info` | Log filter (e.g. `debug,sqlx=warn`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | — | OpenTelemetry collector endpoint |

See `.env.example` for a complete list.

---

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) at your option.