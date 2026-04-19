# SDK Quickstart

> Auto-extracted from `stitchd-sdk/src/lib.rs` module docs.
> Run `cargo xtask docs` to regenerate.

Add the SDK to your `Cargo.toml`:

```toml
[dependencies]
stitchd-sdk = "0.1"
```

Initialize the client once at application startup:

```rust,no_run
use stitchd_sdk::{SdkClient, SdkConfig};
use stitchd_sdk::{Context, EvaluationContext};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let config = SdkConfig::new(
        "http://localhost:9090",   // gRPC endpoint
        "http://localhost:8080",   // REST endpoint for list-checks
        "sk_live_...",             // SDK key
    );

    let client: Arc<SdkClient> = SdkClient::init(config).await.expect("SDK init failed");

    // Evaluate a flag for a specific user context
    let ctx = EvaluationContext {
        contexts: vec![
            Context::new("user", "user-123"),
        ],
    };

    let variant = client.evaluate("my-feature-flag", &ctx).await.expect("evaluation failed");
    if let Some(v) = variant {
        println!("Flag value: {:?}", v);
    }
}
```

See [`SdkClient`], [`SdkConfig`], and [`EvaluationContext`] for full API details.
