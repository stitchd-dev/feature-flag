# SDK Quickstart

> Auto-extracted from `sdks/rust/src/lib.rs` module docs.
> Run `cargo xtask docs` to regenerate.

```ignore
use stitchd_sdk_rust::{SdkClient, SdkConfig, EvalRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = SdkConfig {
        gateway_url: "http://localhost:8081".to_string(),
        sdk_key:     std::env::var("STITCHD_SDK_KEY")?,
        ..Default::default()
    };

    let client = SdkClient::init(config).await?;
    let ctx    = stitchd_core::context::Context::new("user", "alice");
    let results = client
        .evaluate(&[EvalRequest::flag("my-flag", ctx)])
        .await;
    println!("variant = {}", results[0].variant_key);
    client.shutdown(std::time::Duration::from_secs(5)).await?;
    Ok(())
}
```

