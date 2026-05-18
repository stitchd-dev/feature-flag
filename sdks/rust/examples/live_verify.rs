//! Live verification example — connect to a running Stitchd gateway and evaluate flags.
//!
//! Usage:
//!   STITCHD_SDK_KEY=sdk_live_test_key_001 \
//!   STITCHD_GATEWAY_URL=http://localhost:8081 \
//!   cargo run --example live_verify -p stitchd-sdk

use std::time::Duration;

use serde_json::json;
use stitchd_core::context::Context;
use stitchd_sdk::{EvalOutcome, EvalRequest, SdkClient, SdkConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let sdk_key =
        std::env::var("STITCHD_SDK_KEY").unwrap_or_else(|_| "sdk_live_test_key_001".to_string());
    let gateway_url = std::env::var("STITCHD_GATEWAY_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string());

    println!("=== Stitchd SDK Live Verification ===");
    println!("Gateway : {gateway_url}");
    println!("SDK Key : {sdk_key}");
    println!();

    // ── Test 1–4: flag evaluation via SDK ────────────────────────────────────
    let mut config = SdkConfig::new(gateway_url.clone(), sdk_key.clone());
    config.definition_poll_interval = Duration::from_secs(30);
    config.event_flush_interval = Duration::from_millis(500); // fast flush for Test 6

    println!("Initialising SDK client…");
    let client = SdkClient::init(config).await?;
    println!("SDK client initialised.");
    println!();

    // ── Test 1: evaluate a known flag ────────────────────────────────────────
    println!("--- Test 1: evaluate 'test-flag' ---");
    let ctx = Context::new("user", "alice");
    let results = client
        .evaluate(&[EvalRequest {
            flag_key: "test-flag".to_string(),
            context: ctx.clone(),
        }])
        .await;

    let r = &results[0];
    println!("  variant_key  : {}", r.variant_key);
    println!("  outcome      : {:?}", r.outcome);
    assert!(
        !matches!(r.outcome, EvalOutcome::FlagNotFound),
        "flag 'test-flag' should exist in the gateway snapshot"
    );
    println!("  PASS");
    println!();

    // ── Test 2: unknown flag returns FlagNotFound ────────────────────────────
    println!("--- Test 2: unknown flag → FlagNotFound ---");
    let r2 = &client
        .evaluate(&[EvalRequest {
            flag_key: "does-not-exist-xyz".to_string(),
            context: ctx.clone(),
        }])
        .await[0];
    assert!(matches!(r2.outcome, EvalOutcome::FlagNotFound));
    println!("  outcome: {:?}  PASS", r2.outcome);
    println!();

    // ── Test 3: evaluate_with_reasoning ─────────────────────────────────────
    println!("--- Test 3: evaluate_with_reasoning ---");
    let r3 = &client
        .evaluate_with_reasoning(&[EvalRequest {
            flag_key: "test-flag".to_string(),
            context: ctx,
        }])
        .await[0];
    println!("  variant_key : {}", r3.variant_key);
    println!("  outcome     : {:?}", r3.outcome);
    println!("  reasoning   : {:?}", r3.reasoning);
    assert!(!matches!(r3.outcome, EvalOutcome::FlagNotFound));
    println!("  PASS");
    println!();

    // ── Test 4: batch evaluation ─────────────────────────────────────────────
    println!("--- Test 4: batch evaluation of 3 flags ---");
    let batch = client
        .evaluate(
            &(1..=3)
                .map(|i| EvalRequest {
                    flag_key: format!("pagination-test-{i}"),
                    context: Context::new("user", "batch-user"),
                })
                .collect::<Vec<_>>(),
        )
        .await;
    assert_eq!(batch.len(), 3);
    for (i, br) in batch.iter().enumerate() {
        println!("  [{}] {} → {:?}", i + 1, br.flag_key, br.outcome);
    }
    println!("  PASS");
    println!();

    // ── Test 5: segment membership batch (POST /v1/sdk/segments/list:batch) ──
    // Verify the REST → gRPC → segmentation-service pipeline using a known
    // list segment (beta-orgs, id: bd5af427-0d22-4531-9a5a-07b22d8e0c8f).
    // DB state: globex-inc is include-only → member; acme-corp is in both
    // include and exclude lists → NOT a member (exclude wins).
    println!("--- Test 5: segment membership batch endpoint ---");
    let http = reqwest::Client::new();
    let seg_resp = http
        .post(format!("{gateway_url}/v1/sdk/segments/list:batch"))
        .header("Content-Type", "application/json")
        .header("x-sdk-key", &sdk_key)
        .json(&json!({
            "queries": [
                {
                    "context_type": "org",
                    "context_key": "globex-inc",
                    "segment_ids": ["bd5af427-0d22-4531-9a5a-07b22d8e0c8f"]
                },
                {
                    "context_type": "org",
                    "context_key": "acme-corp",
                    "segment_ids": ["bd5af427-0d22-4531-9a5a-07b22d8e0c8f"]
                }
            ]
        }))
        .send()
        .await?;

    assert_eq!(
        seg_resp.status().as_u16(),
        200,
        "segments/list:batch should return 200"
    );
    let seg_body: serde_json::Value = seg_resp.json().await?;
    println!("  response: {}", serde_json::to_string_pretty(&seg_body)?);

    let results_arr = seg_body["results"].as_array().expect("results array");
    assert_eq!(results_arr.len(), 2, "two results expected");

    // globex-inc: include-only → member
    let globex = results_arr
        .iter()
        .find(|r| r["context_key"] == "globex-inc")
        .expect("globex-inc result");
    let globex_memberships = &globex["memberships"];
    let globex_member = globex_memberships["bd5af427-0d22-4531-9a5a-07b22d8e0c8f"]
        .as_bool()
        .expect("bool membership value");
    assert!(globex_member, "globex-inc should be a member of beta-orgs");

    // acme-corp: exclude overrides include → not a member
    let acme = results_arr
        .iter()
        .find(|r| r["context_key"] == "acme-corp")
        .expect("acme-corp result");
    let acme_member = acme["memberships"]["bd5af427-0d22-4531-9a5a-07b22d8e0c8f"]
        .as_bool()
        .expect("bool membership value");
    assert!(
        !acme_member,
        "acme-corp (excluded) should not be a member of beta-orgs"
    );

    println!("  globex-inc member: {globex_member}  (expected true)  ✓");
    println!("  acme-corp  member: {acme_member}  (expected false) ✓");
    println!("  PASS");
    println!();

    // ── Test 6: event flush (POST /v1/sdk/events:batch) ──────────────────────
    // Evaluate a flag to enqueue an event, wait for the 500ms flush, then
    // verify the flush endpoint directly (202 Accepted, no 502).
    println!("--- Test 6: event flush endpoint ---");
    // Trigger an evaluation so the SDK enqueues an event.
    client
        .evaluate(&[EvalRequest {
            flag_key: "test-flag".to_string(),
            context: Context::new("user", "event-test-user"),
        }])
        .await;

    // Direct probe: send a minimal valid payload to the events:batch endpoint.
    let ev_resp = http
        .post(format!("{gateway_url}/v1/sdk/events:batch"))
        .header("Content-Type", "application/json")
        .header("x-sdk-key", &sdk_key)
        .json(&json!({ "events": [] }))
        .send()
        .await?;

    let ev_status = ev_resp.status().as_u16();
    assert_eq!(ev_status, 202, "events:batch should return 202 Accepted");
    println!("  events:batch → HTTP {ev_status}  PASS");

    // Let the background flush task drain the queued event (flush_interval=500ms).
    tokio::time::sleep(Duration::from_secs(1)).await;
    println!("  background flush completed (no 502 warnings expected above)");
    println!("  PASS");
    println!();

    // ── Shutdown ─────────────────────────────────────────────────────────────
    println!("Shutting down…");
    client.shutdown().await;

    println!();
    println!("=== All 6 live verification tests PASSED ===");
    Ok(())
}
