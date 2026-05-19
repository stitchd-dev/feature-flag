# stitchd-sdk-rust — Rust Server-Side Feature Flag SDK

`stitchd-sdk-rust` is a server-side Rust SDK for the Stitchd Feature Flag platform. It evaluates flags entirely in-process using a locally-cached definition snapshot that is continuously synchronized from the Stitchd gateway via gRPC polling. Optional LFU list-segment membership is maintained in a bounded LRU cache, and evaluation events are submitted to the gateway asynchronously via a fire-and-forget batch flush — keeping the hot evaluation path free of network I/O. Conforms to the language-agnostic contract in [`sdks/spec/`](../spec/).

## Installation

```toml
[dependencies]
stitchd-sdk-rust = "0.1"
stitchd-core     = { path = "crates/stitchd-core" }   # for Context, ParameterValue
tokio            = { version = "1", features = ["full"] }
```

## Quickstart

```rust
use stitchd_sdk_rust::{SdkClient, SdkConfig, EvalRequest};
use stitchd_core::context::Context;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = SdkConfig {
        gateway_url: "http://localhost:8081".to_string(),
        sdk_key:     std::env::var("STITCHD_SDK_KEY")?,
        ..Default::default()
    };

    let client = SdkClient::init(config).await?;

    let context = Context::new("user", "alice")
        .with_parameter("plan", "pro".into());

    let results = client
        .evaluate(&[EvalRequest::flag("checkout-flow", context)])
        .await;

    println!("variant = {}", results[0].variant_key);

    client.shutdown().await;
    Ok(())
}
```

## Configuration (`SdkConfig`)

| Field | Default | Description |
|---|---|---|
| `gateway_url` | — | Base URL of the Stitchd gateway REST API (e.g. `http://localhost:8081`) |
| `sdk_key` | — | SDK key provisioned in the Stitchd admin UI |
| `gateway_grpc_port` | `50050` | Port for the gRPC `SdkService` endpoint on the gateway |
| `definition_poll_interval` | `30s` | How often the background task re-syncs flag definitions |
| `list_segment_refresh_interval` | `60s` | How often the LRU membership cache is proactively refreshed |
| `lru_max_entries` | `10 000` | Maximum `(context_type, context_key)` entries in the membership LRU |
| `event_flush_interval` | `5s` | How often queued evaluation events are flushed to the gateway |
| `event_batch_size` | `100` | Maximum events per flush batch |
| `event_buffer_capacity` | `1 000` | Bounded event queue size; events are dropped on overflow |
| `request_timeout` | `5s` | Timeout for individual HTTP/gRPC calls |

## Evaluation

`SdkClient::evaluate` accepts a slice of `EvalRequest` values and returns a
`Vec<EvalResult>` in the same order. Each result contains:

- `flag_key` — echoed from the request
- `variant_key` — the assigned variant key (empty string when `FlagNotFound`)
- `variant_value` — the variant's JSON value (`serde_json::Value`)
- `outcome` — `EvalOutcome::{Matched, DefaultRule, Disabled, FlagNotFound}`

To include rule traces:

```rust
let results = client.evaluate_with_reasoning(&requests).await;
let trace = &results[0].reasoning;
// trace.matched_rule_index, trace.matched_rule_name
```

## Background tasks

`SdkClient::init` spawns three background tasks automatically:

1. **Definition polling** — calls `SdkService::SyncDefinitions` (gRPC) every
   `definition_poll_interval`. On failure, backs off exponentially (×1, ×2,
   ×4, capped at ×5 the interval) and continues serving the last-known snapshot.
2. **LRU refresh** — periodically re-fetches list-segment membership for all
   context keys currently in the LRU via `POST /v1/sdk/segments/list:batch`.
3. **Event flush** — drains the in-process event queue to
   `POST /v1/sdk/events:batch` at `event_flush_interval`.

Call `client.shutdown().await` before process exit to drain the event buffer
before stopping the flush task.

## Behavioural spec

For the canonical algorithm, caching rules, polling lifecycle, and event
delivery semantics, read:

- [`sdks/spec/docs/01-overview.md`](../spec/docs/01-overview.md)
- [`sdks/spec/docs/02-evaluation-semantics.md`](../spec/docs/02-evaluation-semantics.md)
- [`sdks/spec/docs/03-caching.md`](../spec/docs/03-caching.md)
- [`sdks/spec/docs/04-polling.md`](../spec/docs/04-polling.md)
- [`sdks/spec/docs/05-events.md`](../spec/docs/05-events.md)
- [`sdks/spec/docs/06-errors.md`](../spec/docs/06-errors.md)

## Testing

### Unit tests

```bash
cargo test -p stitchd-sdk-rust
```

### Conformance tests

Conformance fixtures live in `sdks/spec/fixtures/evaluation/`. Run with the
`test-util` feature which enables the in-memory test helpers:

```bash
cargo test --features test-util -p stitchd-sdk-rust --test conformance
```

All 8 scenarios covering bool flags, string rules, percentage rollout,
rule-based segments, list segments, reasoning traces, and flag-not-found
are verified automatically.
