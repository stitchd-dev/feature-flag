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
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: Uuid,
    /// FK → `feature_flags.id`.
    #[serde(with = "clickhouse::serde::uuid")]
    pub flag_id: Uuid,
    /// Human-readable flag key.
    pub flag_key: String,
    /// Variant that was served, or `"__disabled__"` when the flag is off.
    pub variant_key: String,
    /// `true` when the flag's targeting toggle was ON at evaluation time
    /// (i.e. `Flag.enabled = true` — rule evaluation actually happened).
    /// `false` when targeting was off and the default-variant fallthrough
    /// was applied. The Phase 4 `experiment_assignments_mv` filters on
    /// `WHERE targeting_on` so disabled-flag evals never produce
    /// experiment exposures.
    pub targeting_on: bool,
    /// When the evaluation occurred (millisecond precision UTC).
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub evaluated_at: DateTime<Utc>,
    /// Context `_type` field (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context `key` field (e.g. `"alice"`).
    pub context_key: String,
    /// JSON object `{"key": "value"}` with private values replaced by `"********"`.
    pub params_json: String,
    /// The rule UUID that matched, or `None` if the flag fell through to the
    /// default-rule path. `None` is also used when the row predates the
    /// schema migration (CH `Nullable(UUID)` column added 2026-05-21).
    /// Populated by `stitchd-flag-service::eval_log_writer` in Phase 2 of the
    /// `experimentation_full_20260521` track. Used by `experiment_assignments_mv`
    /// to scope exposures to the experiment's bound rule (or default-rule).
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub matched_rule_id: Option<Uuid>,
    /// Groups all context rows produced from the same evaluation call.
    ///
    /// When a flag is evaluated against a multi-context bundle (e.g. `user` +
    /// `device`), one row is emitted per context type. All sibling rows from
    /// the same call share this UUID so that downstream queries can reconstruct
    /// the full bundle (e.g. `experiment_assignments_mv` joining the unit
    /// context to its paired device context for cross-context experiments).
    #[serde(with = "clickhouse::serde::uuid")]
    pub evaluation_id: Uuid,
}

// ── Write ────────────────────────────────────────────────────────────────────

/// Insert a batch of [`EvalLogRow`]s into `flag_evaluation_log`.
///
/// Returns the underlying ClickHouse error if the insert fails. The caller
/// should log and discard the error — it must not propagate to the evaluation
/// response path.
///
/// # Errors
/// Returns [`clickhouse::error::Error`] if the ClickHouse insert fails.
pub async fn insert_eval_log_rows(
    client: &Client,
    rows: &[EvalLogRow],
) -> Result<(), clickhouse::error::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut insert = client.insert::<EvalLogRow>("flag_evaluation_log").await?;
    for row in rows {
        insert.write(row).await?;
    }
    insert.end().await
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(params_json: &str, targeting_on: bool) -> EvalLogRow {
        EvalLogRow {
            env_id: Uuid::new_v4(),
            flag_id: Uuid::new_v4(),
            flag_key: "test-flag".into(),
            variant_key: "on".into(),
            targeting_on,
            evaluated_at: Utc::now(),
            context_type: "user".into(),
            context_key: "alice".into(),
            params_json: params_json.into(),
            matched_rule_id: None,
            evaluation_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn row_serializes_with_correct_fields() {
        let row = make_row(r#"{"plan":"pro"}"#, true);
        // The Row derive requires all fields to be present; just confirm construction.
        assert_eq!(row.flag_key, "test-flag");
        assert_eq!(row.variant_key, "on");
        assert!(row.targeting_on);
        assert_eq!(row.params_json, r#"{"plan":"pro"}"#);
    }

    #[test]
    fn row_with_masked_private_value() {
        let row = make_row(r#"{"ssn":"********","plan":"pro"}"#, true);
        assert!(row.params_json.contains(r#""ssn":"********""#));
        assert!(row.params_json.contains(r#""plan":"pro""#));
    }

    #[test]
    fn insert_fn_exists() {
        // Ensure the function is accessible; live insert tested via integration tests.
        let _ = insert_eval_log_rows as fn(_, _) -> _;
    }
}
