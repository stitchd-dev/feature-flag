# stitchd-sdk

Rust server-side SDK for Stitchd. Evaluates feature flags **in-process** with zero network hops per evaluation after startup.

## How it works

1. `SdkClient::init` opens a long-lived gRPC stream to `FlagSyncService` on the gateway (`localhost:50050` by default) and downloads the full flag/segment definition snapshot.
2. Definitions are stored in a `DefinitionCache` behind an `Arc<RwLock>` — reads are lock-free under `tokio`.
3. `evaluate(flag_key, ctx)` walks the `ConditionExpr` rule tree entirely in-process.
4. List-based segment membership falls back to a REST call to the gateway (`localhost:8080`) with results cached in an LFU cache to minimise round-trips.

## Quickstart

```toml
[dependencies]
stitchd-sdk = { git = "https://github.com/stitchd-dev/feature-flag" }
```

```rust
use stitchd_sdk::{SdkClient, SdkConfig, Context, EvaluationContext};

#[tokio::main]
async fn main() {
    let config = SdkConfig::new(
        "http://localhost:50050",  // gRPC FlagSync endpoint (gateway)
        "http://localhost:8080",   // REST endpoint for list-segment checks (gateway)
        "sdk_live_...",            // SDK key from the admin API
    );

    let client = SdkClient::init(config).await.expect("SDK init failed");

    let ctx = EvaluationContext {
        contexts: vec![Context::new("user", "user-123")],
    };

    if let Some(variant) = client.evaluate("my-feature-flag", &ctx).await.expect("eval failed") {
        println!("Variant: {:?}", variant);
    }
}
```

## Key Types

| Type | Purpose |
|------|---------|
| `SdkClient` | Main handle — initialise once, share via `Arc` |
| `SdkConfig` | gRPC endpoint, REST endpoint, SDK key, LFU config |
| `EvaluationContext` | Set of `Context` instances passed to `evaluate` |
| `Context` | A single `(context_type, context_key)` with optional attributes |
| `ParameterValue` | Typed attribute value: `String`, `Int`, `Float`, `Bool`, `List` |
| `LfuConfig` | LFU cache capacity and TTL for list-segment membership |
| `SdkError` | Error variants for init and evaluation failures |

## LFU Cache

List-segment membership results are cached using Least Frequently Used eviction.
Pre-warm high-traffic contexts at startup to avoid REST round-trips for known users.
Configure via `SdkConfig` with `LfuConfig { capacity, ttl }`.

## Dependencies

- `stitchd-core` — domain types and rule engine (re-exported for consumers)
