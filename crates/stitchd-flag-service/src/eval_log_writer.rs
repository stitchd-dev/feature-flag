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

/// Spawns a background task that writes evaluation rows to `flag_evaluation_log`.
///
/// Each element of `contexts_with_variants` pairs an `EvaluationContext` with the
/// variant key that was served to it. Errors are logged via `tracing::error!` and
/// discarded — they never surface to the caller.
pub fn spawn_eval_log_write(
    client: Arc<Client>,
    env_id: Uuid,
    flag_id: Uuid,
    flag_key: String,
    is_disabled: bool,
    evaluated_at: DateTime<Utc>,
    contexts_with_variants: Vec<(EvaluationContext, String)>,
) {
    let rows = build_eval_log_rows(
        env_id,
        flag_id,
        &flag_key,
        is_disabled,
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

/// Build `EvalLogRow`s from context+variant pairs.
///
/// One row per `Context` in each `EvaluationContext`. Private parameter values
/// are replaced with `"********"`; keys are preserved.
pub fn build_eval_log_rows(
    env_id: Uuid,
    flag_id: Uuid,
    flag_key: &str,
    is_disabled: bool,
    evaluated_at: DateTime<Utc>,
    contexts_with_variants: &[(EvaluationContext, String)],
) -> Vec<EvalLogRow> {
    let mut rows = Vec::new();
    for (eval_ctx, variant_key) in contexts_with_variants {
        for ctx in &eval_ctx.contexts {
            let params_json = build_masked_params_json(ctx);
            rows.push(EvalLogRow {
                env_id,
                flag_id,
                flag_key: flag_key.to_string(),
                variant_key: variant_key.clone(),
                is_disabled,
                evaluated_at,
                context_type: ctx.context_type.clone(),
                context_key: ctx.key.clone(),
                params_json,
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
            false,
            Utc::now(),
            &[(ec, "on".to_string())],
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
            false,
            Utc::now(),
            &[(ec1, "on".to_string()), (ec2, "off".to_string())],
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
            false,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "on".to_string())],
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
            .with_parameter("d", ParameterValue::Double(3.14))
            .with_parameter("s", ParameterValue::Str("hello".into()))
            .with_parameter(
                "v",
                ParameterValue::SemVer(Version::parse("1.2.3").unwrap()),
            );
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            false,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "on".to_string())],
        );
        let params: serde_json::Value = serde_json::from_str(&rows[0].params_json).unwrap();
        assert_eq!(params["b"], true);
        assert_eq!(params["i"], 42);
        assert!(params["d"].as_f64().unwrap() - 3.14 < 0.001);
        assert_eq!(params["s"], "hello");
        assert_eq!(params["v"], "1.2.3");
    }

    #[test]
    fn empty_contexts_produces_no_rows() {
        let (env_id, flag_id) = nil_ids();
        let rows = build_eval_log_rows(env_id, flag_id, "flag", false, Utc::now(), &[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn disabled_flag_recorded_correctly() {
        let (env_id, flag_id) = nil_ids();
        let ctx = Context::new("user", "alice");
        let rows = build_eval_log_rows(
            env_id,
            flag_id,
            "flag",
            true,
            Utc::now(),
            &[(eval_ctx(vec![ctx]), "__disabled__".to_string())],
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_disabled);
        assert_eq!(rows[0].variant_key, "__disabled__");
    }
}
