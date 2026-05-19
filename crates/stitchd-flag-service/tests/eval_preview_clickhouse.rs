//! Smoke test: evaluate_preview → rows appear in ClickHouse.
//!
//! Requires a running flag-service on STITCHD_FLAG_SERVICE_ADDR (default localhost:50052)
//! and ClickHouse on STITCHD_CLICKHOUSE_URL. Skipped when either is absent.

use stitchd_proto::flags::v1::EvaluatePreviewRequest;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;

fn ch_client() -> Option<clickhouse::Client> {
    let url = std::env::var("STITCHD_CLICKHOUSE_URL").ok()?;
    let db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let user = std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_string());
    let password = std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_default();
    Some(
        clickhouse::Client::default()
            .with_url(url)
            .with_database(db)
            .with_user(user)
            .with_password(password),
    )
}

fn flag_service_addr() -> Option<String> {
    std::env::var("STITCHD_FLAG_SERVICE_ADDR").ok()
}

#[tokio::test]
async fn evaluate_preview_writes_rows_to_clickhouse() {
    let Some(ch) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(addr) = flag_service_addr() else {
        eprintln!("STITCHD_FLAG_SERVICE_ADDR not set — skipping");
        return;
    };

    let project_id = std::env::var("TEST_PROJECT_ID")
        .unwrap_or_else(|_| "b787ef8e-a429-4df8-86fe-973fec41bc77".to_string());
    let flag_key = std::env::var("TEST_FLAG_KEY").unwrap_or_else(|_| "test-flag".to_string());

    // Count rows before the call.
    let before: u64 = ch
        .query("SELECT count() FROM flag_evaluation_log WHERE flag_key = ?")
        .bind(&flag_key)
        .fetch_one()
        .await
        .expect("count before");

    let mut client = FlagServiceClient::connect(addr)
        .await
        .expect("connect to flag-service");

    // Two evaluation contexts → expect 2 new rows.
    let contexts_json = serde_json::json!([
        {
            "contexts": [{
                "context_type": "user",
                "key": "alice",
                "parameters": {"plan": "pro", "email": "alice@example.com"},
                "private_parameters": ["email"]
            }]
        },
        {
            "contexts": [{
                "context_type": "user",
                "key": "bob",
                "parameters": {"plan": "free"},
                "private_parameters": []
            }]
        }
    ])
    .to_string();

    let resp = client
        .evaluate_preview(EvaluatePreviewRequest {
            project_id,
            flag_key: flag_key.clone(),
            contexts_json,
            environment_id: String::new(),
        })
        .await
        .expect("evaluate_preview RPC")
        .into_inner();

    println!(
        "flag_enabled={}, results_len={}",
        resp.flag_enabled,
        serde_json::from_str::<serde_json::Value>(&resp.results_json)
            .map(|v| v.as_array().map(|a| a.len()).unwrap_or(0))
            .unwrap_or(0)
    );

    // Give ClickHouse a moment to make the inserted rows visible.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    let after: u64 = ch
        .query("SELECT count() FROM flag_evaluation_log WHERE flag_key = ?")
        .bind(&flag_key)
        .fetch_one()
        .await
        .expect("count after");

    assert_eq!(after - before, 2, "expected 2 new rows (one per context)");

    // Verify masking on the alice row.
    #[derive(clickhouse::Row, serde::Deserialize)]
    struct Row {
        params_json: String,
        context_key: String,
    }
    let rows: Vec<Row> = ch
        .query("SELECT params_json, context_key FROM flag_evaluation_log WHERE flag_key = ? AND context_key IN ('alice','bob') ORDER BY context_key")
        .bind(&flag_key)
        .fetch_all()
        .await
        .expect("fetch alice/bob rows");

    let alice = rows
        .iter()
        .find(|r| r.context_key == "alice")
        .expect("alice row");
    let params: serde_json::Value = serde_json::from_str(&alice.params_json).unwrap();
    assert_eq!(params["email"], "********", "email must be masked");
    assert_eq!(params["plan"], "pro", "plan must be plain");

    println!("✓ 2 rows written, masking correct");
}
