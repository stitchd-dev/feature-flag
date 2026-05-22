//! Experiment-results assembly — splits per-metric variant results into
//! `primaries` and `guardrails`, and computes the `direction_violation`
//! flag for each guardrail row against the metric's `goal_direction`.
//!
//! Per spec §3, guardrails are computed with the same pipeline as primary
//! metrics (Frequentist, Bayesian, CUPED). The only differences in the
//! response are the bucket they land in (`primaries` vs `guardrails`) and
//! the `direction_violation: bool` flag on guardrails.
//!
//! ## Direction-violation rule
//!
//! Given the variant's observed `lift` (`treatment − control`) and the
//! metric's `goal_direction`:
//!
//! | `goal_direction` | `lift` sign | `direction_violation` |
//! |------------------|-------------|-----------------------|
//! | `Increase`       | `< 0`       | `true`                |
//! | `Increase`       | `≥ 0`       | `false`               |
//! | `Decrease`       | `> 0`       | `true`                |
//! | `Decrease`       | `≤ 0`       | `false`               |
//! | `Neutral`        | any         | `false`               |
//!
//! A NaN lift (e.g. percentile metrics whose lift comes from bootstrap CI
//! midpoints) is treated as "no direction" → `false`.

use serde::{Deserialize, Serialize};

use stitchd_core::metric::GoalDirection;

// ── Wire types ───────────────────────────────────────────────────────────────

/// One per-variant result row, used by both `primaries` and `guardrails` in
/// the experiment-results JSON.
///
/// This is the stats-service-internal wire shape; the gateway re-projects to
/// its admin-facing `VariantResultJson` (Phase 7) but the field set is
/// identical. The `direction_violation` field is always set, defaulting to
/// `false` for primaries (where it has no spec meaning) — and is the load-
/// bearing field for guardrails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VariantResultJson {
    /// Variant identifier (e.g. `"control"`, `"variant_b"`).
    pub variant_key: String,
    /// Number of assigned contexts in this variant for the iteration.
    pub participant_count: u64,
    /// Observed `treatment − control` lift on the metric. `0.0` for the
    /// control row.
    pub lift: f64,
    /// Whether the observed `lift` violates the metric's `goal_direction`.
    /// Always `false` for primary metrics (the field is reserved for
    /// guardrails); a guardrail with a sign-mismatched lift surfaces a
    /// warning in the UI.
    pub direction_violation: bool,
}

/// The two buckets of per-variant rows in an experiment's results JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResultBuckets {
    /// Primary metrics — drive the experiment's recommendation.
    pub primaries: Vec<VariantResultJson>,
    /// Guardrail metrics — computed identically but surfaced separately,
    /// with `direction_violation` set per the rule in this module's docs.
    pub guardrails: Vec<VariantResultJson>,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Compute the `direction_violation` flag for a guardrail metric's variant
/// row given its observed `lift` and the metric's `goal_direction`.
///
/// See the module docs for the full rule table.
#[must_use]
pub fn is_direction_violation(lift: f64, goal_direction: GoalDirection) -> bool {
    if lift.is_nan() {
        return false;
    }
    match goal_direction {
        GoalDirection::Increase => lift < 0.0,
        GoalDirection::Decrease => lift > 0.0,
        GoalDirection::Neutral => false,
    }
}

/// Tag a set of per-variant rows as guardrails: set `direction_violation`
/// based on each row's `lift` and the metric's `goal_direction`. The input
/// rows are expected to come out of the same Frequentist + Bayesian + CUPED
/// pipeline used for primaries — this function only applies the
/// guardrail-specific direction-violation tag.
///
/// Returns a new `Vec` so the caller's primary-results vector remains
/// untouched. The control row (lift = 0) is never flagged as a violation
/// since `0.0` is "neither increase nor decrease".
#[must_use]
pub fn tag_guardrail_rows(
    rows: Vec<VariantResultJson>,
    goal_direction: GoalDirection,
) -> Vec<VariantResultJson> {
    rows.into_iter()
        .map(|mut r| {
            r.direction_violation = is_direction_violation(r.lift, goal_direction);
            r
        })
        .collect()
}

/// Compose the final `ResultBuckets` from already-computed primary and
/// guardrail rows. The primary rows are passed through unchanged; the
/// guardrail rows have `direction_violation` re-computed against the
/// supplied `goal_direction` so the field is authoritative regardless of
/// what the caller already populated.
#[must_use]
pub fn build_result_buckets(
    primaries: Vec<VariantResultJson>,
    guardrail_rows: Vec<VariantResultJson>,
    guardrail_goal_direction: GoalDirection,
) -> ResultBuckets {
    ResultBuckets {
        primaries,
        guardrails: tag_guardrail_rows(guardrail_rows, guardrail_goal_direction),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn row(variant_key: &str, lift: f64) -> VariantResultJson {
        VariantResultJson {
            variant_key: variant_key.to_string(),
            participant_count: 1000,
            lift,
            direction_violation: false,
        }
    }

    // ── is_direction_violation ──────────────────────────────────────────────

    #[test]
    fn increase_metric_positive_lift_is_not_violation() {
        assert!(!is_direction_violation(0.10, GoalDirection::Increase));
        assert!(!is_direction_violation(0.0, GoalDirection::Increase));
    }

    #[test]
    fn increase_metric_negative_lift_is_violation() {
        assert!(is_direction_violation(-0.05, GoalDirection::Increase));
    }

    #[test]
    fn decrease_metric_negative_lift_is_not_violation() {
        // For a "decrease" goal (e.g. latency, error rate), negative lift
        // means the treatment moved the metric in the desired direction.
        assert!(!is_direction_violation(-0.10, GoalDirection::Decrease));
        assert!(!is_direction_violation(0.0, GoalDirection::Decrease));
    }

    #[test]
    fn decrease_metric_positive_lift_is_violation() {
        // For a "decrease" goal, positive lift moves the metric the wrong way.
        assert!(is_direction_violation(0.10, GoalDirection::Decrease));
    }

    #[test]
    fn neutral_metric_never_violates() {
        // "Neutral" guardrails track only — no direction expectation.
        assert!(!is_direction_violation(0.10, GoalDirection::Neutral));
        assert!(!is_direction_violation(-0.10, GoalDirection::Neutral));
        assert!(!is_direction_violation(0.0, GoalDirection::Neutral));
    }

    #[test]
    fn nan_lift_is_never_a_violation() {
        // Bootstrap-based percentile metrics can produce NaN lifts; we never
        // flag those as direction violations (no signal to flag).
        assert!(!is_direction_violation(f64::NAN, GoalDirection::Increase));
        assert!(!is_direction_violation(f64::NAN, GoalDirection::Decrease));
        assert!(!is_direction_violation(f64::NAN, GoalDirection::Neutral));
    }

    // ── tag_guardrail_rows ──────────────────────────────────────────────────

    /// Spec sub-task 5.2: guardrail with positive lift + `goal_direction =
    /// "increase"` → no violation.
    #[test]
    fn guardrail_positive_lift_increase_no_violation() {
        let rows = vec![row("control", 0.0), row("variant_b", 0.15)];
        let tagged = tag_guardrail_rows(rows, GoalDirection::Increase);
        assert!(!tagged[0].direction_violation);
        assert!(!tagged[1].direction_violation);
    }

    /// Spec sub-task 5.2: guardrail with negative lift + `goal_direction =
    /// "increase"` → violation flagged.
    #[test]
    fn guardrail_negative_lift_increase_violation_flagged() {
        let rows = vec![row("control", 0.0), row("variant_b", -0.08)];
        let tagged = tag_guardrail_rows(rows, GoalDirection::Increase);
        assert!(
            !tagged[0].direction_violation,
            "control row is never flagged"
        );
        assert!(
            tagged[1].direction_violation,
            "treatment with negative lift on increase metric must flag"
        );
    }

    #[test]
    fn guardrail_decrease_metric_violation_flagged_for_positive_lift() {
        // A latency guardrail: treatment increased latency → violation.
        let rows = vec![row("control", 0.0), row("variant_b", 25.0)];
        let tagged = tag_guardrail_rows(rows, GoalDirection::Decrease);
        assert!(tagged[1].direction_violation);
    }

    #[test]
    fn guardrail_preserves_other_fields() {
        let rows = vec![VariantResultJson {
            variant_key: "v".to_string(),
            participant_count: 12345,
            lift: -0.5,
            direction_violation: false,
        }];
        let tagged = tag_guardrail_rows(rows, GoalDirection::Increase);
        assert_eq!(tagged[0].variant_key, "v");
        assert_eq!(tagged[0].participant_count, 12345);
        assert_eq!(tagged[0].lift, -0.5);
        assert!(tagged[0].direction_violation);
    }

    #[test]
    fn guardrail_recomputes_violation_overriding_input_field() {
        // Caller pre-populates direction_violation = false but the lift +
        // direction say it should be true. Output must reflect reality.
        let rows = vec![VariantResultJson {
            variant_key: "v".to_string(),
            participant_count: 100,
            lift: -0.2,
            direction_violation: false, // wrong; tag_guardrail_rows must override
        }];
        let tagged = tag_guardrail_rows(rows, GoalDirection::Increase);
        assert!(tagged[0].direction_violation);
    }

    #[test]
    fn guardrail_recomputes_violation_overriding_true_to_false() {
        // Caller-supplied direction_violation = true on a row whose lift +
        // direction don't actually violate — must be cleared.
        let rows = vec![VariantResultJson {
            variant_key: "v".to_string(),
            participant_count: 100,
            lift: 0.2,
            direction_violation: true, // wrong; tag_guardrail_rows must override
        }];
        let tagged = tag_guardrail_rows(rows, GoalDirection::Increase);
        assert!(!tagged[0].direction_violation);
    }

    // ── build_result_buckets ────────────────────────────────────────────────

    #[test]
    fn build_result_buckets_keeps_primaries_untouched() {
        // Primaries should pass through with their direction_violation
        // unchanged (they're not subject to the guardrail tag rule).
        let primaries = vec![VariantResultJson {
            variant_key: "control".to_string(),
            participant_count: 1000,
            lift: 0.0,
            direction_violation: false,
        }];
        let buckets = build_result_buckets(primaries.clone(), vec![], GoalDirection::Increase);
        assert_eq!(buckets.primaries, primaries);
        assert!(buckets.guardrails.is_empty());
    }

    #[test]
    fn build_result_buckets_tags_guardrails() {
        let primaries = vec![row("c", 0.0), row("v", 0.05)];
        let guardrails = vec![row("c", 0.0), row("v", -0.10)];
        let buckets = build_result_buckets(primaries, guardrails, GoalDirection::Increase);
        // Primaries: untouched (no spec violations on primaries).
        assert_eq!(buckets.primaries.len(), 2);
        assert!(!buckets.primaries[0].direction_violation);
        assert!(!buckets.primaries[1].direction_violation);
        // Guardrails: control unflagged; treatment (negative lift on
        // increase metric) flagged.
        assert_eq!(buckets.guardrails.len(), 2);
        assert!(!buckets.guardrails[0].direction_violation);
        assert!(buckets.guardrails[1].direction_violation);
    }

    // ── Serialisation ───────────────────────────────────────────────────────

    #[test]
    fn result_buckets_serializes_to_snake_case() {
        let buckets = ResultBuckets {
            primaries: vec![row("c", 0.0)],
            guardrails: vec![row("c", 0.0)],
        };
        let v = serde_json::to_value(&buckets).unwrap();
        assert!(v.get("primaries").is_some());
        assert!(v.get("guardrails").is_some());

        let row_v = &v["primaries"][0];
        assert!(row_v.get("variant_key").is_some());
        assert!(row_v.get("participant_count").is_some());
        assert!(row_v.get("lift").is_some());
        assert!(row_v.get("direction_violation").is_some());
    }
}
