//! Integration test: eval log rows reach ClickHouse after insert.
//!
//! Skipped when `STITCHD_CLICKHOUSE_URL` is not set (CI without ClickHouse).

use chrono::Utc;
use clickhouse::Row;
use serde::Deserialize;
use stitchd_db::clickhouse::{EvalLogRow, insert_eval_log_rows};
use uuid::Uuid;

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

#[allow(dead_code)]
#[derive(Debug, Deserialize, Row)]
struct EvalLogRowRead {
    #[serde(with = "clickhouse::serde::uuid")]
    env_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    flag_id: Uuid,
    flag_key: String,
    variant_key: String,
    targeting_on: bool,
    context_type: String,
    context_key: String,
    params_json: String,
}

#[tokio::test]
async fn eval_log_rows_appear_in_clickhouse() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping ClickHouse integration test");
        return;
    };

    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let flag_key = format!("test-flag-{}", &env_id.to_string()[..8]);
    let now = Utc::now();

    let bundle_eval_id = Uuid::new_v4();
    let rows = vec![
        EvalLogRow {
            env_id,
            flag_id,
            flag_key: flag_key.clone(),
            variant_key: "on".to_string(),
            targeting_on: true,
            evaluated_at: now,
            context_type: "user".to_string(),
            context_key: "alice".to_string(),
            params_json: r#"{"plan":"pro","email":"********"}"#.to_string(),
            matched_rule_id: None,
            evaluation_id: bundle_eval_id,
        },
        EvalLogRow {
            env_id,
            flag_id,
            flag_key: flag_key.clone(),
            variant_key: "off".to_string(),
            targeting_on: true,
            evaluated_at: now,
            context_type: "org".to_string(),
            context_key: "acme".to_string(),
            params_json: r#"{"tier":"enterprise"}"#.to_string(),
            matched_rule_id: None,
            evaluation_id: bundle_eval_id,
        },
    ];

    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert_eval_log_rows should succeed");

    // Wait briefly for MergeTree to make rows visible.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let fetched: Vec<EvalLogRowRead> = client
        .query("SELECT env_id, flag_id, flag_key, variant_key, targeting_on, context_type, context_key, params_json FROM flag_evaluation_log WHERE flag_key = ? ORDER BY context_type")
        .bind(&flag_key)
        .fetch_all()
        .await
        .expect("SELECT from flag_evaluation_log should succeed");

    assert_eq!(fetched.len(), 2, "expected 2 rows (one per context)");

    // Row 0: org/acme (ordered by context_type ASC)
    assert_eq!(fetched[0].context_type, "org");
    assert_eq!(fetched[0].context_key, "acme");
    assert_eq!(fetched[0].variant_key, "off");
    assert!(fetched[0].targeting_on);
    assert_eq!(fetched[0].env_id, env_id);

    // Row 1: user/alice
    assert_eq!(fetched[1].context_type, "user");
    assert_eq!(fetched[1].context_key, "alice");
    assert_eq!(fetched[1].variant_key, "on");
    // Masking: email value must be "********", key preserved
    let params: serde_json::Value =
        serde_json::from_str(&fetched[1].params_json).expect("params_json is valid JSON");
    assert_eq!(params["email"], "********", "private value must be masked");
    assert_eq!(params["plan"], "pro", "non-private value must be preserved");
}

#[tokio::test]
async fn eval_log_n_contexts_produces_n_rows() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping ClickHouse integration test");
        return;
    };

    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let flag_key = format!("test-flag-multi-{}", &env_id.to_string()[..8]);
    let now = Utc::now();

    // Insert 5 rows (simulating 5 contexts evaluated in one call).
    let rows: Vec<EvalLogRow> = (0..5)
        .map(|i| EvalLogRow {
            env_id,
            flag_id,
            flag_key: flag_key.clone(),
            variant_key: "on".to_string(),
            targeting_on: true,
            evaluated_at: now,
            context_type: "user".to_string(),
            context_key: format!("user-{i}"),
            params_json: r#"{}"#.to_string(),
            matched_rule_id: None,
            evaluation_id: Uuid::new_v4(),
        })
        .collect();

    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let count: u64 = client
        .query("SELECT count() FROM flag_evaluation_log WHERE flag_key = ?")
        .bind(&flag_key)
        .fetch_one()
        .await
        .expect("count query should succeed");

    assert_eq!(count, 5, "5 contexts → 5 rows");
}
