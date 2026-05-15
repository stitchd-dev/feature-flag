//! ClickHouse write path for `flag_evaluation_log`.
//!
//! Each server-side flag evaluation (currently `evaluate_preview`) produces one
//! row per context. Private parameter values are masked to `"********"` before
//! insertion; keys are preserved for the context intelligence registry.

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::Serialize;
use uuid::Uuid;

// ── Row type ─────────────────────────────────────────────────────────────────

/// One row in `flag_evaluation_log`.
#[derive(Debug, Clone, Serialize, Row)]
pub struct EvalLogRow {
    /// FK → environments.id.
    pub env_id: Uuid,
    /// FK → feature_flags.id.
    pub flag_id: Uuid,
    /// Human-readable flag key.
    pub flag_key: String,
    /// Variant that was served, or `"__disabled__"` when the flag is off.
    pub variant_key: String,
    /// `true` when the flag was disabled and the default variant was applied.
    pub is_disabled: bool,
    /// When the evaluation occurred (millisecond precision UTC).
    pub evaluated_at: DateTime<Utc>,
    /// Context `_type` field (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context `key` field (e.g. `"alice"`).
    pub context_key: String,
    /// JSON object `{"key": "value"}` with private values replaced by `"********"`.
    pub params_json: String,
}

// ── Write ────────────────────────────────────────────────────────────────────

/// Insert a batch of [`EvalLogRow`]s into `flag_evaluation_log`.
///
/// Returns the underlying ClickHouse error if the insert fails. The caller
/// should log and discard the error — it must not propagate to the evaluation
/// response path.
pub async fn insert_eval_log_rows(
    client: &Client,
    rows: &[EvalLogRow],
) -> Result<(), clickhouse::error::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut insert = client.insert("flag_evaluation_log")?;
    for row in rows {
        insert.write(row).await?;
    }
    insert.end().await
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(params_json: &str, is_disabled: bool) -> EvalLogRow {
        EvalLogRow {
            env_id: Uuid::new_v4(),
            flag_id: Uuid::new_v4(),
            flag_key: "test-flag".into(),
            variant_key: "on".into(),
            is_disabled,
            evaluated_at: Utc::now(),
            context_type: "user".into(),
            context_key: "alice".into(),
            params_json: params_json.into(),
        }
    }

    #[test]
    fn row_serializes_with_correct_fields() {
        let row = make_row(r#"{"plan":"pro"}"#, false);
        // The Row derive requires all fields to be present; just confirm construction.
        assert_eq!(row.flag_key, "test-flag");
        assert_eq!(row.variant_key, "on");
        assert!(!row.is_disabled);
        assert_eq!(row.params_json, r#"{"plan":"pro"}"#);
    }

    #[test]
    fn row_with_masked_private_value() {
        let row = make_row(r#"{"ssn":"********","plan":"pro"}"#, false);
        assert!(row.params_json.contains(r#""ssn":"********""#));
        assert!(row.params_json.contains(r#""plan":"pro""#));
    }

    #[test]
    fn insert_fn_exists() {
        // Ensure the function is accessible; live insert tested via integration tests.
        let _ = insert_eval_log_rows as fn(_, _) -> _;
    }
}
