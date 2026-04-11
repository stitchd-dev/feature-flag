//! Stitchd server binary entry point.
use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Observability is bootstrapped in Phase 6; this stub ensures the binary compiles.
    tracing_subscriber::fmt().init();
    info!("stitchd-server starting");
    Ok(())
}
