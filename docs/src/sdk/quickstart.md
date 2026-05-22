# SDK Quickstart

> Auto-extracted from `sdks/rust/src/lib.rs` module docs.
> Run `cargo xtask docs` to regenerate.

```no_run
use std::time::Duration;
use stitchd_sdk_rust::{EvalRequest, SdkClient, SdkConfig};
use stitchd_core::context::Context;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Required: gateway base URL + SDK key (provisioned in the admin UI).
    // All other fields fall back to spec-defined defaults — see `SdkConfig`.
    let config = SdkConfig::new(
        std::env::var("STITCHD_GATEWAY_URL")
            .unwrap_or_else(|_| "http://localhost:8081".to_string()),
        std::env::var("STITCHD_SDK_KEY")?,
    );

    // `init` validates config, performs the first definition sync, and
    // spawns the three background tasks (poll, LRU refresh, event flush).
    let client = SdkClient::init(config).await?;

    // Evaluate a flag for a `(context_type, key)` tuple.
    let context = Context::new("user", "alice");
    let results = client
        .evaluate(&[EvalRequest {
            flag_key: "checkout-flow".to_string(),
            context,
        }])
        .await;
    println!("variant = {}", results[0].variant_key);

    // Drain the track-event buffer + stop the background tasks.
    client.shutdown(Duration::from_secs(5)).await?;
    Ok(())
}
```

