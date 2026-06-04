//! Metric kinds — the composable primitives that experiments target.
//!
//! Three kinds are supported in this initial implementation:
//!
//! - [`MetricKind::Aggregation`] — a SQL-style aggregator over one event
//!   stream (count / sum / avg / percentiles / uniq), optionally scoped
//!   to a field within the event payload and filtered with a JsonLogic
//!   where-clause.
//! - [`MetricKind::Ratio`] — a derived metric that divides one
//!   metric's numerator by another's denominator (e.g.
//!   `checkout_completed / checkout_started = completion_rate`). The
//!   referenced metrics must be `Aggregation` metrics; the stats service
//!   resolves them at compute time.
//! - [`MetricKind::Funnel`] — an ordered sequence of event steps with a
//!   conversion window. Backed by ClickHouse `windowFunnel` at the
//!   stats-service layer.
//!
//! All variants carry their own typed config struct and a `validate()`
//! method that enforces shape invariants (e.g. funnel has ≥2 steps,
//! ratio numerator and denominator are distinct).

use serde::{Deserialize, Serialize};

use crate::id::MetricId;

use super::{MetricValidationError, validate_field_name};

/// Recursively validates every JsonLogic `{"var": "<field>"}` reference inside
/// `expr`, rejecting any field name that is unsafe to interpolate into a
/// ClickHouse `properties['<field>']` map-key literal (see
/// [`validate_field_name`]).
///
/// The walker is deliberately structure-agnostic: it descends into every object
/// value and array element looking for `var` nodes, so it stays correct even if
/// the stats-service grows new JsonLogic operators. A `{"var": …}` node whose
/// value is a (non-string) is ignored here — the stats-service rejects those
/// structurally at query-build time.
fn validate_jsonlogic_vars(expr: &serde_json::Value) -> Result<(), MetricValidationError> {
    match expr {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "var"
                    && let Some(field) = value.as_str()
                {
                    validate_field_name(field)?;
                }
                validate_jsonlogic_vars(value)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for item in items {
                validate_jsonlogic_vars(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// ── MetricKind (discriminated union) ──────────────────────────────────────────

/// Discriminated union over the three supported metric primitives.
///
/// Serde tag: `kind` — e.g. `{"kind":"aggregation", ...}`. This matches
/// the wire format used in the public REST API and the protobuf oneof
/// representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetricKind {
    /// A SQL-style aggregator over a single event stream.
    Aggregation(AggregationConfig),
    /// A derived metric: numerator metric / denominator metric.
    Ratio(RatioConfig),
    /// An ordered sequence of event steps with a conversion window.
    Funnel(FunnelConfig),
}

impl MetricKind {
    /// Validate kind-specific shape invariants. See variant-config
    /// `validate()` methods for the per-kind rules.
    pub fn validate(&self) -> Result<(), MetricValidationError> {
        match self {
            Self::Aggregation(c) => c.validate(),
            Self::Ratio(c) => c.validate(),
            Self::Funnel(c) => c.validate(),
        }
    }

    /// Canonical short tag for telemetry / logging.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Aggregation(_) => "aggregation",
            Self::Ratio(_) => "ratio",
            Self::Funnel(_) => "funnel",
        }
    }
}

// ── Aggregation ───────────────────────────────────────────────────────────────

/// The SQL-style aggregator applied to event rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AggregationOperator {
    /// Number of events. Ignores `on_field`.
    Count,
    /// Sum of a numeric field across all events.
    Sum,
    /// Arithmetic mean of a numeric field.
    Avg,
    /// 50th percentile of a numeric field.
    P50,
    /// 90th percentile of a numeric field.
    P90,
    /// 99th percentile of a numeric field.
    P99,
    /// Cardinality (`uniq`) over a field — number of distinct values.
    Uniq,
}

impl AggregationOperator {
    /// Whether this aggregator needs an `on_field` to operate on.
    ///
    /// All aggregators now treat `on_field` as **optional**. When absent (or
    /// set to a canonical column alias like `"value"` / `"value_double"` /
    /// `"value_int"`), the stats-service uses the event's built-in numeric
    /// value columns (`value_double` / `value_int`).  When a non-canonical
    /// name is supplied it is treated as a `properties[<name>]` map key.
    ///
    /// `count` is the only aggregator that truly ignores `on_field`
    /// (it counts rows regardless). Keeping this method as a stable API
    /// surface; it now always returns `false`.
    #[must_use]
    pub const fn requires_field(self) -> bool {
        false
    }
}

/// Config for an [`MetricKind::Aggregation`] metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AggregationConfig {
    /// Source event key (e.g. `"checkout_completed"`).
    pub event_key: String,
    /// How to aggregate matching rows.
    pub aggregator: AggregationOperator,
    /// Optional field reference for numeric aggregators.
    ///
    /// - `None` / `""` / `"value"` / `"value_double"` / `"value_int"` →
    ///   the stats-service uses the canonical numeric columns
    ///   (`value_double` then `value_int`).
    /// - Any other string → treated as a `properties[<name>]` map key
    ///   (coerced to `Float64` via `toFloat64OrNull`).
    ///
    /// `Count` ignores this field entirely.
    pub on_field: Option<String>,
    /// Optional JsonLogic where-clause to filter rows before aggregation.
    /// Stored as raw JSON; the stats-service transforms it to ClickHouse
    /// SQL at query-build time.
    pub where_clause: Option<serde_json::Value>,
}

impl AggregationConfig {
    /// Returns `Ok` if the config is well-formed, else a typed error.
    ///
    /// `on_field` is optional for all aggregators — validation no longer
    /// rejects a missing field. When present, both `on_field` and every
    /// JsonLogic `{"var": …}` reference in `where_clause` are checked for
    /// characters that are unsafe to splice into a ClickHouse
    /// `properties['<field>']` map-key literal (SQL-injection / placeholder
    /// collision hardening — see [`validate_field_name`]).
    pub fn validate(&self) -> Result<(), MetricValidationError> {
        if let Some(field) = &self.on_field {
            validate_field_name(field)?;
        }
        if let Some(where_clause) = &self.where_clause {
            validate_jsonlogic_vars(where_clause)?;
        }
        Ok(())
    }
}

// ── Ratio ─────────────────────────────────────────────────────────────────────

/// Config for a [`MetricKind::Ratio`] metric.
///
/// The ratio is `numerator_metric / denominator_metric`. Both must be
/// `Aggregation` metrics in the same environment. The stats service
/// resolves the references at compute time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RatioConfig {
    /// Metric whose value forms the numerator.
    pub numerator_metric_id: MetricId,
    /// Metric whose value forms the denominator.
    pub denominator_metric_id: MetricId,
    /// Minimum denominator count before the ratio is considered
    /// statistically valid. Below this threshold the metric is reported
    /// as `Insufficient data` rather than a noisy ratio.
    pub min_denominator: i64,
}

impl RatioConfig {
    /// Returns `Ok` if the config is well-formed, else a typed error.
    pub fn validate(&self) -> Result<(), MetricValidationError> {
        if self.numerator_metric_id == self.denominator_metric_id {
            return Err(MetricValidationError::RatioSameMetric);
        }
        if self.min_denominator < 0 {
            return Err(MetricValidationError::RatioNegativeMinDenominator(
                self.min_denominator,
            ));
        }
        Ok(())
    }
}

// ── Funnel ────────────────────────────────────────────────────────────────────

/// One step in a [`MetricKind::Funnel`] metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunnelStep {
    /// Event key for this step (e.g. `"checkout_started"`).
    pub event_key: String,
    /// Optional JsonLogic where-clause to filter which events qualify
    /// as this step.
    pub where_clause: Option<serde_json::Value>,
}

/// Config for a [`MetricKind::Funnel`] metric.
///
/// Funnels are evaluated at the stats-service layer via ClickHouse's
/// `windowFunnel` aggregate function. The conversion rate for step *N*
/// is computed as `countIf(level >= N) / countIf(level >= 1)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunnelConfig {
    /// Ordered list of funnel steps. Must contain at least 2 entries.
    pub steps: Vec<FunnelStep>,
    /// Maximum time window (seconds) within which the funnel must
    /// complete for a context to count as converted.
    pub window_seconds: i64,
    /// Whether repeated occurrences of a step event count as
    /// progression. When `false`, `windowFunnel` uses `strict_order`
    /// mode (the default for product funnels).
    pub count_repeats: bool,
}

impl FunnelConfig {
    /// Returns `Ok` if the config is well-formed, else a typed error.
    ///
    /// In addition to the shape checks (≥2 steps, positive window), every
    /// step's JsonLogic `where_clause` `{"var": …}` references are validated
    /// for map-key-safe characters — the funnel `where_clause` flows through
    /// the same ClickHouse `properties['<field>']` interpolation as the
    /// aggregation one.
    pub fn validate(&self) -> Result<(), MetricValidationError> {
        if self.steps.len() < 2 {
            return Err(MetricValidationError::FunnelTooFewSteps(self.steps.len()));
        }
        if self.window_seconds <= 0 {
            return Err(MetricValidationError::FunnelNonPositiveWindow(
                self.window_seconds,
            ));
        }
        for step in &self.steps {
            if let Some(where_clause) = &step.where_clause {
                validate_jsonlogic_vars(where_clause)?;
            }
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::MetricId;
    use serde_json::json;

    // ── AggregationOperator::requires_field ──────────────────────────────────

    #[test]
    fn count_does_not_require_field() {
        assert!(!AggregationOperator::Count.requires_field());
    }

    #[test]
    fn non_count_aggregators_do_not_require_field_either() {
        // on_field is now optional for ALL aggregators — absent means
        // "use the canonical numeric value columns".
        for op in [
            AggregationOperator::Sum,
            AggregationOperator::Avg,
            AggregationOperator::P50,
            AggregationOperator::P90,
            AggregationOperator::P99,
            AggregationOperator::Uniq,
        ] {
            assert!(!op.requires_field(), "{op:?} should not require a field");
        }
    }

    // ── AggregationConfig::validate ──────────────────────────────────────────

    #[test]
    fn count_aggregation_validates_without_field() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn sum_without_field_validates() {
        // on_field is now optional — sum without it uses canonical value columns.
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: None,
            where_clause: None,
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn sum_with_field_validates() {
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("revenue".into()),
            where_clause: Some(json!({ "==": [{ "var": "currency" }, "USD"] })),
        };
        assert!(cfg.validate().is_ok());
    }

    // ── on_field / JsonLogic `var` charset hardening (SQL-injection /
    //    placeholder-collision) ─────────────────────────────────────────────

    /// Helper: an aggregation config with a given `on_field`, no where-clause.
    fn agg_with_on_field(on_field: Option<&str>) -> AggregationConfig {
        AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: on_field.map(Into::into),
            where_clause: None,
        }
    }

    #[test]
    fn on_field_with_single_quote_and_bracket_is_rejected() {
        // `x']` would break out of the `properties['x']` map-key literal.
        let cfg = agg_with_on_field(Some("x']"));
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::UnsafeFieldName("x']".into()),
        );
    }

    #[test]
    fn on_field_with_placeholder_substring_is_rejected() {
        // `a{p0}b` collides with the `{pN}` → `?` placeholder rewrite.
        let cfg = agg_with_on_field(Some("a{p0}b"));
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::UnsafeFieldName("a{p0}b".into()),
        );
    }

    #[test]
    fn on_field_with_backslash_is_rejected() {
        let cfg = agg_with_on_field(Some("foo\\bar"));
        assert!(matches!(
            cfg.validate().unwrap_err(),
            MetricValidationError::UnsafeFieldName(_)
        ));
    }

    #[test]
    fn normal_on_field_names_are_accepted() {
        // A plain identifier and a dotted path must both validate.
        assert!(agg_with_on_field(Some("revenue")).validate().is_ok());
        assert!(agg_with_on_field(Some("cart.total")).validate().is_ok());
        // Hyphen / underscore / space are allowed too.
        assert!(agg_with_on_field(Some("latency_ms")).validate().is_ok());
        assert!(agg_with_on_field(Some("a-b c")).validate().is_ok());
        // Absent on_field stays valid.
        assert!(agg_with_on_field(None).validate().is_ok());
    }

    #[test]
    fn where_clause_var_with_single_quote_is_rejected() {
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("revenue".into()),
            where_clause: Some(json!({ "==": [{ "var": "currency'--" }, "USD"] })),
        };
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::UnsafeFieldName("currency'--".into()),
        );
    }

    #[test]
    fn where_clause_var_inside_nested_combinator_is_rejected() {
        // Unsafe `var` buried under `and` → `or` must still be caught (the
        // walker descends the whole tree).
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: None,
            where_clause: Some(json!({
                "and": [
                    { "==": [{ "var": "country" }, "US"] },
                    { "or": [
                        { "==": [{ "var": "tier" }, "gold"] },
                        { "in": [{ "var": "bad{p0}" }, ["x", "y"]] },
                    ] },
                ]
            })),
        };
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::UnsafeFieldName("bad{p0}".into()),
        );
    }

    #[test]
    fn where_clause_with_only_safe_vars_validates() {
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: None,
            where_clause: Some(json!({
                "and": [
                    { "==": [{ "var": "country" }, "US"] },
                    { "in": [{ "var": "tier" }, ["gold", "silver"]] },
                ]
            })),
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn funnel_step_where_clause_unsafe_var_is_rejected() {
        let cfg = FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "start".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "complete".into(),
                    where_clause: Some(json!({ "==": [{ "var": "plan']" }, "pro"] })),
                },
            ],
            window_seconds: 3600,
            count_repeats: false,
        };
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::UnsafeFieldName("plan']".into()),
        );
    }

    // ── RatioConfig::validate ────────────────────────────────────────────────

    #[test]
    fn ratio_with_distinct_metrics_validates() {
        let cfg = RatioConfig {
            numerator_metric_id: MetricId::new(),
            denominator_metric_id: MetricId::new(),
            min_denominator: 50,
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ratio_with_identical_metrics_fails() {
        let same = MetricId::new();
        let cfg = RatioConfig {
            numerator_metric_id: same,
            denominator_metric_id: same,
            min_denominator: 50,
        };
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::RatioSameMetric,
        );
    }

    #[test]
    fn ratio_with_negative_min_denominator_fails() {
        let cfg = RatioConfig {
            numerator_metric_id: MetricId::new(),
            denominator_metric_id: MetricId::new(),
            min_denominator: -1,
        };
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::RatioNegativeMinDenominator(-1),
        );
    }

    // ── FunnelConfig::validate ───────────────────────────────────────────────

    #[test]
    fn funnel_with_two_steps_validates() {
        let cfg = FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "start".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "complete".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 3600,
            count_repeats: false,
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn funnel_with_single_step_fails() {
        let cfg = FunnelConfig {
            steps: vec![FunnelStep {
                event_key: "lonely".into(),
                where_clause: None,
            }],
            window_seconds: 3600,
            count_repeats: false,
        };
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::FunnelTooFewSteps(1),
        );
    }

    #[test]
    fn funnel_with_zero_window_fails() {
        let cfg = FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "a".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "b".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 0,
            count_repeats: false,
        };
        assert_eq!(
            cfg.validate().unwrap_err(),
            MetricValidationError::FunnelNonPositiveWindow(0),
        );
    }

    // ── MetricKind serde round-trip + tag ────────────────────────────────────

    #[test]
    fn aggregation_serde_round_trip() {
        let kind = MetricKind::Aggregation(AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        });
        let j = serde_json::to_value(&kind).unwrap();
        assert_eq!(j["kind"], "aggregation");
        assert_eq!(j["aggregator"], "count");
        let back: MetricKind = serde_json::from_value(j).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn ratio_serde_round_trip() {
        let num = MetricId::new();
        let den = MetricId::new();
        let kind = MetricKind::Ratio(RatioConfig {
            numerator_metric_id: num,
            denominator_metric_id: den,
            min_denominator: 100,
        });
        let j = serde_json::to_value(&kind).unwrap();
        assert_eq!(j["kind"], "ratio");
        let back: MetricKind = serde_json::from_value(j).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn funnel_serde_round_trip_strict_order() {
        let kind = MetricKind::Funnel(FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "view".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "add_to_cart".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "checkout".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 86_400,
            count_repeats: false,
        });
        let j = serde_json::to_value(&kind).unwrap();
        assert_eq!(j["kind"], "funnel");
        assert_eq!(j["window_seconds"], 86_400);
        let back: MetricKind = serde_json::from_value(j).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn metric_kind_tags() {
        let m_a = MetricKind::Aggregation(AggregationConfig {
            event_key: "k".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        });
        let m_r = MetricKind::Ratio(RatioConfig {
            numerator_metric_id: MetricId::new(),
            denominator_metric_id: MetricId::new(),
            min_denominator: 0,
        });
        let m_f = MetricKind::Funnel(FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "a".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "b".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 60,
            count_repeats: false,
        });
        assert_eq!(m_a.tag(), "aggregation");
        assert_eq!(m_r.tag(), "ratio");
        assert_eq!(m_f.tag(), "funnel");
    }

    #[test]
    fn metric_kind_validate_propagates_to_variant() {
        // Invalid funnel: only 1 step.
        let bad = MetricKind::Funnel(FunnelConfig {
            steps: vec![FunnelStep {
                event_key: "only".into(),
                where_clause: None,
            }],
            window_seconds: 60,
            count_repeats: false,
        });
        assert_eq!(
            bad.validate().unwrap_err(),
            MetricValidationError::FunnelTooFewSteps(1),
        );
    }
}
