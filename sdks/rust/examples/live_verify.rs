//! Live verification example — connect to a running Stitchd gateway and evaluate flags.
//!
//! Usage:
//!   STITCHD_SDK_KEY=sdk_live_test_key_001 \
//!   STITCHD_GATEWAY_URL=http://localhost:8081 \
//!   cargo run --example live_verify -p stitchd-sdk

use std::time::Duration;

use stitchd_core::context::Context;
use stitchd_sdk::{EvalOutcome, EvalRequest, SdkClient, SdkConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let sdk_key = std::env::var("STITCHD_SDK_KEY")
        .unwrap_or_else(|_| "sdk_live_test_key_001".to_string());
    let gateway_url = std::env::var("STITCHD_GATEWAY_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string());

    println!("=== Stitchd SDK Live Verification ===");
    println!("Gateway : {gateway_url}");
    println!("SDK Key : {sdk_key}");
    println!();

    let mut config = SdkConfig::new(gateway_url.clone(), sdk_key.clone());
    config.definition_poll_interval = Duration::from_secs(30);
    config.event_flush_interval = Duration::from_secs(5);

    println!("Initialising SDK client (will poll definitions from gateway)…");
    let client = SdkClient::init(config).await?;
    println!("SDK client initialised.");
    println!();

    // ── Test 1: evaluate a known flag ────────────────────────────────────────
    println!("--- Test 1: evaluate 'test-flag' for user alice ---");
    let ctx = Context::new("user", "alice");
    let results = client
        .evaluate(&[EvalRequest {
            flag_key: "test-flag".to_string(),
            context: ctx.clone(),
        }])
        .await;

    let r = &results[0];
    println!("  flag_key     : {}", r.flag_key);
    println!("  variant_key  : {}", r.variant_key);
    println!("  variant_value: {}", r.variant_value);
    println!("  outcome      : {:?}", r.outcome);
    assert!(
        !matches!(r.outcome, EvalOutcome::FlagNotFound),
        "flag 'test-flag' should exist in the gateway snapshot"
    );
    println!("  PASS: flag found and evaluated");
    println!();

    // ── Test 2: evaluate a flag that does not exist ──────────────────────────
    println!("--- Test 2: evaluate non-existent flag ---");
    let results2 = client
        .evaluate(&[EvalRequest {
            flag_key: "does-not-exist-xyz".to_string(),
            context: ctx.clone(),
        }])
        .await;
    let r2 = &results2[0];
    println!("  outcome: {:?}", r2.outcome);
    assert!(
        matches!(r2.outcome, EvalOutcome::FlagNotFound),
        "unknown flag should return FlagNotFound"
    );
    println!("  PASS: FlagNotFound returned for unknown flag");
    println!();

    // ── Test 3: evaluate with reasoning ─────────────────────────────────────
    println!("--- Test 3: evaluate_with_reasoning for 'test-flag' ---");
    let results3 = client
        .evaluate_with_reasoning(&[EvalRequest {
            flag_key: "test-flag".to_string(),
            context: ctx,
        }])
        .await;
    let r3 = &results3[0];
    println!("  variant_key  : {}", r3.variant_key);
    println!("  outcome      : {:?}", r3.outcome);
    println!("  reasoning    : {:?}", r3.reasoning);
    assert!(
        !matches!(r3.outcome, EvalOutcome::FlagNotFound),
        "flag should be found"
    );
    println!("  PASS: reasoning trace returned");
    println!();

    // ── Test 4: batch evaluation ─────────────────────────────────────────────
    println!("--- Test 4: batch evaluation of 3 flags ---");
    let batch_requests: Vec<EvalRequest> = (1..=3)
        .map(|i| EvalRequest {
            flag_key: format!("pagination-test-{i}"),
            context: Context::new("user", "batch-user"),
        })
        .collect();
    let batch_results = client.evaluate(&batch_requests).await;
    assert_eq!(batch_results.len(), 3, "should return one result per request");
    for (i, br) in batch_results.iter().enumerate() {
        println!("  [{}] {} -> {:?}", i + 1, br.flag_key, br.outcome);
    }
    println!("  PASS: batch evaluation returned {} results", batch_results.len());
    println!();

    // ── Flush events and shut down cleanly ───────────────────────────────────
    println!("Flushing events and shutting down…");
    client.shutdown().await;

    println!();
    println!("=== All live verification tests PASSED ===");
    Ok(())
}
