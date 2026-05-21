//! Fire-and-forget evaluation log writer for flag evaluations.
//!
//! Converts `EvaluationContext`s into `EvalLogRow`s, masking private parameter
//! values to `"********"` before writing to ClickHouse. The write is spawned
//! as a background task so it never adds latency to the evaluation response.

use chrono::{DateTime, Utc};
use clickhouse::Client;
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

use stitchd_core::context::{EvaluationContext, ParameterValue};
use stitchd_db::clickhouse::{EvalLogRow, insert_eval_log_rows};

/// One (`EvaluationContext`, served `variant_key`, optional `matched_rule_id`) tuple.
///
/// `matched_rule_id` is the UUID of the custom rule that matched, or `None`
/// when the flag fell through to the default-rule path. Callers MUST set
/// `matched_rule_id = None` when `targeting_on = false` — no rule evaluation
/// runs when targeting is off. The writer enforces this invariant defensively
/// (any `Some(rule_id)` paired with `targeting_on = false` is rewritten to
/// `None` at row-build time to keep the Phase 4
/// `experiment_assignments_mv` invariant safe).
pub type EvalContextRow = (EvaluationContext, String, Option<Uuid>);

/// Spawns a background task that writes evaluation rows to `flag_evaluation_log`.
///
/// Each element of `contexts_with_variants` pairs an `EvaluationContext` with the
/// variant key that was served and the UUID of the rule that matched (or `None`
/// for default-rule fall-through / disabled flag). Errors are logged via
/// `tracing::error!` and discarded — they never surface to the caller.
pub fn spawn_eval_log_write(
    client: Arc<Client>,
    env_id: Uuid,
    flag_id: Uuid,
    flag_key: String,
    targeting_on: bool,
    evaluated_at: DateTime<Utc>,
    contexts_with_variants: Vec<EvalContextRow>,
) {
    let rows = build_eval_log_rows(
        env_id,
        flag_id,
        &flag_key,
        targeting_on,
        evaluated_at,
        &contexts_with_variants,
    );
    if rows.is_empty() {
        return;
    }
    tokio::spawn(async move {
        if let Err(e) = insert_eval_log_rows(&client, &rows).await {
            error!(flag_key = %flag_key, "eval_log write failed: {e}");
        }
    });
}

/// Build `EvalLogRow`s from `(context, variant_key, matched_rule_id)` tuples.
///
/// One row per `Context` in each `EvaluationContext`. Private parameter values
/// are replaced with `"********"`; keys are preserved.
///
/// `matched_rule_id` is taken from the input tuple. When `targeting_on = false`
/// the value is forced to `None` — no rule evaluation runs when the flag's
/// targeting toggle is off, so any incoming `Some(rule_id)` indicates a caller
/// bug and is dropped to keep the Phase 4 `experiment_assignments_mv`
/// invariants intact (it filters on `WHERE targeting_on` + `matched_rule_id`).
pub fn build_eval_log_rows(
    env_id: Uuid,
    flag_id: Uuid,
    flag_key: &str,
    targeting_on: bool,
    evaluated_at: DateTime<Utc>,
    contexts_with_variants: &[EvalContextRow],
) -> Vec<EvalLogRow> {
    let mut rows = Vec::new();
    for (eval_ctx, variant_key, matched_rule_id) in contexts_with_variants {
        // Defensive: enforce the invariant — no rule evaluation runs when
        // targeting is off, so `matched_rule_id` MUST be None in that case.
        let matched_rule_id = if targeting_on { *matched_rule_id } else { None };
        for ctx in &eval_ctx.contexts {
            let params_json = build_masked_params_json(ctx);
            rows.push(EvalLogRow {
                env_id,
                flag_id,
                flag_key: flag_key.to_string(),
                variant_key: variant_key.clone(),
                targeting_on,
                evaluated_at,
                context_type: ctx.context_type.clone(),
                context_key: ctx.key.clone(),
                params_json,
                matched_rule_id,
            });
        }
    }
    rows
}

/// Serialize context parameters to a JSON object, masking private values.
fn build_masked_params_json(ctx: &stitchd_core::context::Context) -> String {
    let mut map = serde_json::Map::new();
    for (k, v) in &ctx.parameters {
        let json_val = if ctx.is_private(k) {
            serde_json::Value::String("********".into())
        } else {
            param_value_to_json(v)
        };
        map.insert(k.clone(), json_val);
    }
    serde_json::Value::Object(map).to_string()
}

fn param_value_to_json(v: &ParameterValue) -> serde_json::Value {
    match v {
        ParameterValue::Bool(b) => serde_json::Value::Bool(*b),
        ParameterValue::Int(i) => serde_json::json!(i),
        ParameterValue::Double(d) => serde_json::json!(d),
        ParameterValue::SemVer(s) => serde_json::Value::String(s.to_string()),
        ParameterValue::Str(s) => serde_json::Value::String(s.clone()),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::context::{Context, ParameterValue};

    fn eval_ctx(contexts: Vec<Context>) -> EvaluationContext {
        let mut ec = EvaluationContext::new();
        for c in contexts {
            ec = ec.with_context(c);
        }
        ec
    }

    fn nil_ids() -> (Uuid, Uuid) {
        (Uuid::nil(), Uuid::nil())
    }

    #[test]
    fn one_row_per_context() {
        let (env_id, flag_id) = nil_ids();
        let ec = eval_ctx(vec![
            Context::new("user", "alice"),
            Context::new("org", "acme"),
        ]);
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "my-flag",
            true,
            Utc::now(),
            &[(ec, "on".to_string(), None)],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].context_type, "user");
        assert_eq!(rows[0].context_key, "alice");
        assert_eq!(rows[1].context_type, "org");
        assert_eq!(rows[1].context_key, "acme");
    }

    #[test]
    fn per_context_variant_key_recorded() {
        let (env_id, flag_id) = nil_ids();
        let ec1 = eval_ctx(vec![Context::new("user", "alice")]);
        let ec2 = eval_ctx(vec![Context::new("user", "bob")]);
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            true,
            Utc::now(),
            &[
                (ec1, "on".to_string(), None),
                (ec2, "off".to_string(), None),
            ],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].variant_key, "on");
        assert_eq!(rows[1].variant_key, "off");
    }

    #[test]
    fn private_param_value_masked_key_preserved() {
        let (env_id, flag_id) = nil_ids();
        let ctx = Context::new("user", "bob")
            .with_parameter("email", ParameterValue::Str("bob@example.com".into()))
            .with_parameter("plan", ParameterValue::Str("pro".into()))
            .with_private_parameter("email");
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            true,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "on".to_string(), None)],
        );
        assert_eq!(rows.len(), 1);
        let params: serde_json::Value = serde_json::from_str(&rows[0].params_json).unwrap();
        assert_eq!(params["email"], "********");
        assert_eq!(params["plan"], "pro");
    }

    #[test]
    fn all_param_types_serialize() {
        use semver::Version;
        let (env_id, flag_id) = nil_ids();
        let ctx = Context::new("user", "u")
            .with_parameter("b", ParameterValue::Bool(true))
            .with_parameter("i", ParameterValue::Int(42))
            .with_parameter("d", ParameterValue::Double(std::f64::consts::PI))
            .with_parameter("s", ParameterValue::Str("hello".into()))
            .with_parameter(
                "v",
                ParameterValue::SemVer(Version::parse("1.2.3").unwrap()),
            );
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            true,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "on".to_string(), None)],
        );
        let params: serde_json::Value = serde_json::from_str(&rows[0].params_json).unwrap();
        assert_eq!(params["b"], true);
        assert_eq!(params["i"], 42);
        assert!((params["d"].as_f64().unwrap() - std::f64::consts::PI).abs() < 0.001);
        assert_eq!(params["s"], "hello");
        assert_eq!(params["v"], "1.2.3");
    }

    #[test]
    fn empty_contexts_produces_no_rows() {
        let (env_id, flag_id) = nil_ids();
        let rows = build_eval_log_rows(env_id, flag_id, "flag", true, Utc::now(), &[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn disabled_flag_recorded_correctly() {
        let (env_id, flag_id) = nil_ids();
        let ctx = Context::new("user", "alice");
        // targeting_on=false ⇔ flag was disabled (targeting toggle OFF).
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            false,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "__disabled__".to_string(), None)],
        );
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].targeting_on);
        assert!(
            rows[0].matched_rule_id.is_none(),
            "no rule evaluation runs when targeting is off, so matched_rule_id MUST be None"
        );
        assert_eq!(rows[0].variant_key, "__disabled__");
    }

    // ── Phase 2 Task 2.1: matched_rule_id wiring ─────────────────────────────

    #[test]
    fn matched_rule_id_propagates_when_custom_rule_matched() {
        // Custom rule matched → row carries the rule UUID.
        let (env_id, flag_id) = nil_ids();
        let rule_id = Uuid::new_v4();
        let ctx = Context::new("user", "alice");
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            true,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "treatment".to_string(), Some(rule_id))],
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].targeting_on);
        assert_eq!(
            rows[0].matched_rule_id,
            Some(rule_id),
            "matched rule UUID must round-trip into the eval-log row"
        );
        assert_eq!(rows[0].variant_key, "treatment");
    }

    #[test]
    fn default_rule_fallthrough_has_none_matched_rule_id() {
        // Targeting on but no custom rule matched (default-rule fall-through) →
        // matched_rule_id is None.
        let (env_id, flag_id) = nil_ids();
        let ctx = Context::new("user", "alice");
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            true,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "control".to_string(), None)],
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].targeting_on);
        assert!(
            rows[0].matched_rule_id.is_none(),
            "default-rule fall-through must produce matched_rule_id = None"
        );
    }

    #[test]
    fn matched_rule_id_forced_to_none_when_targeting_off() {
        // Defensive invariant: even if a caller incorrectly passes a Some(rule_id)
        // when targeting_on = false, the writer drops it. The Phase 4
        // experiment_assignments_mv joins on (targeting_on, matched_rule_id) and
        // we must never produce an exposure row from a disabled-flag eval.
        let (env_id, flag_id) = nil_ids();
        let rogue_rule_id = Uuid::new_v4();
        let ctx = Context::new("user", "alice");
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            false,
            Utc::now(),
            &[(
                eval_ctx(vec![ctx]),
                "__disabled__".to_string(),
                Some(rogue_rule_id),
            )],
        );
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].targeting_on);
        assert!(
            rows[0].matched_rule_id.is_none(),
            "targeting_on = false MUST always coerce matched_rule_id to None"
        );
    }

    #[test]
    fn matched_rule_id_per_context_varies() {
        // Verify the per-row association: two evaluations, distinct
        // matched_rule_ids, both rows reflect their own ID.
        let (env_id, flag_id) = nil_ids();
        let rule_a = Uuid::new_v4();
        let ec_alice = eval_ctx(vec![Context::new("user", "alice")]);
        let ec_bob = eval_ctx(vec![Context::new("user", "bob")]);
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            true,
            Utc::now(),
            &[
                (ec_alice, "treatment".to_string(), Some(rule_a)),
                (ec_bob, "control".to_string(), None),
            ],
        );
        assert_eq!(rows.len(), 2);
        // Row 0 = alice's evaluation
        assert_eq!(rows[0].context_key, "alice");
        assert_eq!(rows[0].matched_rule_id, Some(rule_a));
        // Row 1 = bob's evaluation (fell through to default-rule)
        assert_eq!(rows[1].context_key, "bob");
        assert!(rows[1].matched_rule_id.is_none());
    }
}
