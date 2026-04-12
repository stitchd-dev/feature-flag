use crate::context::ParameterValue;
use crate::rule_engine::condition::Condition;
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::types::EvaluationInput;
use semver::{Version, VersionReq};

/// Evaluate a single leaf [`Condition`] against the provided [`EvaluationInput`].
pub fn evaluate_leaf(
    cond: &Condition,
    input: &EvaluationInput<'_>,
) -> Result<bool, RuleEngineError> {
    match cond {
        // ── Equality / Inequality ─────────────────────────────────────────
        Condition::Eq {
            context_type,
            param,
            value,
        } => {
            let actual = lookup_param(input, context_type, param)?;
            Ok(actual == value)
        }
        Condition::Ne {
            context_type,
            param,
            value,
        } => {
            let actual = lookup_param(input, context_type, param)?;
            Ok(actual != value)
        }

        // ── Numeric Comparisons ───────────────────────────────────────────
        Condition::Lt {
            context_type,
            param,
            value,
        } => {
            let actual = lookup_param(input, context_type, param)?;
            numeric_cmp(actual, value, param, |o| o.is_lt())
        }
        Condition::Lte {
            context_type,
            param,
            value,
        } => {
            let actual = lookup_param(input, context_type, param)?;
            numeric_cmp(actual, value, param, |o| o.is_le())
        }
        Condition::Gt {
            context_type,
            param,
            value,
        } => {
            let actual = lookup_param(input, context_type, param)?;
            numeric_cmp(actual, value, param, |o| o.is_gt())
        }
        Condition::Gte {
            context_type,
            param,
            value,
        } => {
            let actual = lookup_param(input, context_type, param)?;
            numeric_cmp(actual, value, param, |o| o.is_ge())
        }

        // ── String Operators ──────────────────────────────────────────────
        Condition::Contains {
            context_type,
            param,
            substr,
        } => {
            let s = lookup_str(input, context_type, param)?;
            Ok(s.contains(substr.as_str()))
        }
        Condition::StartsWith {
            context_type,
            param,
            prefix,
        } => {
            let s = lookup_str(input, context_type, param)?;
            Ok(s.starts_with(prefix.as_str()))
        }
        Condition::EndsWith {
            context_type,
            param,
            suffix,
        } => {
            let s = lookup_str(input, context_type, param)?;
            Ok(s.ends_with(suffix.as_str()))
        }

        // ── SemVer Comparisons ────────────────────────────────────────────
        Condition::SemverGte {
            context_type,
            param,
            version,
        } => {
            let actual = lookup_semver(input, context_type, param)?;
            let req = parse_semver_req(&format!(">={version}"), param)?;
            Ok(req.matches(actual))
        }
        Condition::SemverTilde {
            context_type,
            param,
            version,
        } => {
            let actual = lookup_semver(input, context_type, param)?;
            let req = parse_semver_req(&format!("~{version}"), param)?;
            Ok(req.matches(actual))
        }
        Condition::SemverCaret {
            context_type,
            param,
            version,
        } => {
            let actual = lookup_semver(input, context_type, param)?;
            let req = parse_semver_req(&format!("^{version}"), param)?;
            Ok(req.matches(actual))
        }

        // ── Segment Membership ────────────────────────────────────────────
        Condition::InSegment(segment_id) => Ok(input.resolved_segments.contains(segment_id)),
        Condition::NotInSegment(segment_id) => Ok(!input.resolved_segments.contains(segment_id)),

        // ── Cross-Flag Condition ──────────────────────────────────────────
        Condition::FlagEvaluatedAs {
            flag_id,
            variant_id,
        } => Ok(input.evaluated_flags.get(flag_id) == Some(variant_id)),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Look up a parameter value from the named context.
fn lookup_param<'a>(
    input: &'a EvaluationInput<'_>,
    context_type: &str,
    param: &str,
) -> Result<&'a ParameterValue, RuleEngineError> {
    let ctx = input
        .find_context(context_type)
        .ok_or_else(|| RuleEngineError::MissingContext {
            context_type: context_type.to_owned(),
        })?;
    ctx.parameters
        .get(param)
        .ok_or_else(|| RuleEngineError::MissingParameter {
            param: param.to_owned(),
        })
}

/// Look up a parameter and assert it is `ParameterValue::Str`.
fn lookup_str<'a>(
    input: &'a EvaluationInput<'_>,
    context_type: &str,
    param: &str,
) -> Result<&'a str, RuleEngineError> {
    match lookup_param(input, context_type, param)? {
        ParameterValue::Str(s) => Ok(s.as_str()),
        other => Err(RuleEngineError::TypeMismatch {
            param: param.to_owned(),
            expected: "Str",
            actual: param_value_type_name(other),
        }),
    }
}

/// Look up a parameter and assert it is `ParameterValue::SemVer`.
fn lookup_semver<'a>(
    input: &'a EvaluationInput<'_>,
    context_type: &str,
    param: &str,
) -> Result<&'a Version, RuleEngineError> {
    match lookup_param(input, context_type, param)? {
        ParameterValue::SemVer(v) => Ok(v),
        other => Err(RuleEngineError::TypeMismatch {
            param: param.to_owned(),
            expected: "SemVer",
            actual: param_value_type_name(other),
        }),
    }
}

/// Parse a semver `VersionReq`; map parse failure to `TypeMismatch`.
fn parse_semver_req(req_str: &str, param: &str) -> Result<VersionReq, RuleEngineError> {
    VersionReq::parse(req_str).map_err(|_| RuleEngineError::TypeMismatch {
        param: param.to_owned(),
        expected: "valid semver requirement",
        actual: "unparseable version string",
    })
}

/// Compare two `ParameterValue`s using `f` applied to their `Ordering`.
///
/// Supported types: `Int`, `Double`, `SemVer`. All others → `TypeMismatch`.
fn numeric_cmp(
    actual: &ParameterValue,
    value: &ParameterValue,
    param: &str,
    f: impl Fn(std::cmp::Ordering) -> bool,
) -> Result<bool, RuleEngineError> {
    match (actual, value) {
        (ParameterValue::Int(a), ParameterValue::Int(b)) => Ok(f(a.cmp(b))),
        (ParameterValue::Double(a), ParameterValue::Double(b)) => {
            Ok(f(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)))
        }
        (ParameterValue::SemVer(a), ParameterValue::SemVer(b)) => Ok(f(a.cmp(b))),
        (actual, _) => Err(RuleEngineError::TypeMismatch {
            param: param.to_owned(),
            expected: "Int, Double, or SemVer",
            actual: param_value_type_name(actual),
        }),
    }
}

/// Return a static type name string for a `ParameterValue` variant.
fn param_value_type_name(v: &ParameterValue) -> &'static str {
    match v {
        ParameterValue::Bool(_) => "Bool",
        ParameterValue::Int(_) => "Int",
        ParameterValue::Double(_) => "Double",
        ParameterValue::SemVer(_) => "SemVer",
        ParameterValue::Str(_) => "Str",
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, ParameterValue};
    use crate::id::{FlagId, SegmentId, VariantId};
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::EvaluationInput;
    use semver::Version;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn user_ctx(params: &[(&str, ParameterValue)]) -> Context {
        let mut ctx = Context::new("user", "u-1");
        for (k, v) in params {
            ctx = ctx.with_parameter(*k, v.clone());
        }
        ctx
    }

    fn input_with(ctx: &[Context]) -> EvaluationInput<'_> {
        EvaluationInput::new(ctx)
    }

    // ── Equality ─────────────────────────────────────────────────────────────

    #[test]
    fn eq_str_match() {
        let ctx = [user_ctx(&[("plan", ParameterValue::Str("pro".into()))])];
        let cond = Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn eq_str_no_match() {
        let ctx = [user_ctx(&[("plan", ParameterValue::Str("free".into()))])];
        let cond = Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn ne_match() {
        let ctx = [user_ctx(&[("plan", ParameterValue::Str("free".into()))])];
        let cond = Condition::Ne {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    // ── Numeric: Int ─────────────────────────────────────────────────────────

    #[test]
    fn lt_int_true() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(17))])];
        let cond = Condition::Lt {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn lt_int_false_when_equal() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(18))])];
        let cond = Condition::Lt {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn lte_int_true_when_equal() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(18))])];
        let cond = Condition::Lte {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn gt_int_true() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(20))])];
        let cond = Condition::Gt {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn gte_int_true_when_equal() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(18))])];
        let cond = Condition::Gte {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    // ── Numeric: Double ───────────────────────────────────────────────────────

    #[test]
    fn gt_double_true() {
        let ctx = [user_ctx(&[("score", ParameterValue::Double(0.9))])];
        let cond = Condition::Gt {
            context_type: "user".into(),
            param: "score".into(),
            value: ParameterValue::Double(0.5),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    // ── Type Mismatch ────────────────────────────────────────────────────────

    #[test]
    fn numeric_cmp_type_mismatch_returns_error() {
        let ctx = [user_ctx(&[("age", ParameterValue::Str("old".into()))])];
        let cond = Condition::Lt {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        let result = evaluate_leaf(&cond, &input_with(&ctx));
        assert!(matches!(result, Err(RuleEngineError::TypeMismatch { .. })));
    }

    // ── Missing param / context ───────────────────────────────────────────────

    #[test]
    fn missing_param_returns_error() {
        let ctx = [user_ctx(&[])];
        let cond = Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        };
        assert_eq!(
            evaluate_leaf(&cond, &input_with(&ctx)),
            Err(RuleEngineError::MissingParameter {
                param: "plan".into()
            })
        );
    }

    #[test]
    fn missing_context_returns_error() {
        let ctx: [Context; 0] = [];
        let cond = Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        };
        assert_eq!(
            evaluate_leaf(&cond, &input_with(&ctx)),
            Err(RuleEngineError::MissingContext {
                context_type: "user".into()
            })
        );
    }

    // ── String Operators ──────────────────────────────────────────────────────

    #[test]
    fn contains_match() {
        let ctx = [user_ctx(&[(
            "email",
            ParameterValue::Str("user@acme.com".into()),
        )])];
        let cond = Condition::Contains {
            context_type: "user".into(),
            param: "email".into(),
            substr: "acme".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn contains_no_match() {
        let ctx = [user_ctx(&[(
            "email",
            ParameterValue::Str("user@other.com".into()),
        )])];
        let cond = Condition::Contains {
            context_type: "user".into(),
            param: "email".into(),
            substr: "acme".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn contains_type_mismatch() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(30))])];
        let cond = Condition::Contains {
            context_type: "user".into(),
            param: "age".into(),
            substr: "3".into(),
        };
        assert!(matches!(
            evaluate_leaf(&cond, &input_with(&ctx)),
            Err(RuleEngineError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn starts_with_match() {
        let ctx = [user_ctx(&[("name", ParameterValue::Str("Alice".into()))])];
        let cond = Condition::StartsWith {
            context_type: "user".into(),
            param: "name".into(),
            prefix: "Al".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn starts_with_no_match() {
        let ctx = [user_ctx(&[("name", ParameterValue::Str("Bob".into()))])];
        let cond = Condition::StartsWith {
            context_type: "user".into(),
            param: "name".into(),
            prefix: "Al".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn starts_with_type_mismatch() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(30))])];
        let cond = Condition::StartsWith {
            context_type: "user".into(),
            param: "age".into(),
            prefix: "3".into(),
        };
        assert!(matches!(
            evaluate_leaf(&cond, &input_with(&ctx)),
            Err(RuleEngineError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn ends_with_match() {
        let ctx = [user_ctx(&[(
            "domain",
            ParameterValue::Str("api.acme.com".into()),
        )])];
        let cond = Condition::EndsWith {
            context_type: "user".into(),
            param: "domain".into(),
            suffix: ".com".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn ends_with_no_match() {
        let ctx = [user_ctx(&[(
            "domain",
            ParameterValue::Str("api.acme.io".into()),
        )])];
        let cond = Condition::EndsWith {
            context_type: "user".into(),
            param: "domain".into(),
            suffix: ".com".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn ends_with_type_mismatch() {
        let ctx = [user_ctx(&[("age", ParameterValue::Int(30))])];
        let cond = Condition::EndsWith {
            context_type: "user".into(),
            param: "age".into(),
            suffix: "0".into(),
        };
        assert!(matches!(
            evaluate_leaf(&cond, &input_with(&ctx)),
            Err(RuleEngineError::TypeMismatch { .. })
        ));
    }

    // ── SemVer ────────────────────────────────────────────────────────────────

    fn semver_ctx(v: &str) -> [Context; 1] {
        [user_ctx(&[(
            "version",
            ParameterValue::SemVer(Version::parse(v).unwrap()),
        )])]
    }

    #[test]
    fn semver_gte_true() {
        let ctx = semver_ctx("1.2.3");
        let cond = Condition::SemverGte {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.0".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn semver_gte_false() {
        let ctx = semver_ctx("1.1.9");
        let cond = Condition::SemverGte {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.0".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn semver_tilde_matches_patch_increment() {
        // ~1.2.3 → >=1.2.3, <1.3.0
        let ctx = semver_ctx("1.2.4");
        let cond = Condition::SemverTilde {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.3".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn semver_tilde_rejects_minor_increment() {
        // ~1.2.3 must not match 1.3.0
        let ctx = semver_ctx("1.3.0");
        let cond = Condition::SemverTilde {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.3".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn semver_caret_matches_minor_increment() {
        // ^1.2.3 → >=1.2.3, <2.0.0
        let ctx = semver_ctx("1.3.0");
        let cond = Condition::SemverCaret {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.3".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn semver_caret_rejects_major_increment() {
        // ^1.2.3 must not match 2.0.0
        let ctx = semver_ctx("2.0.0");
        let cond = Condition::SemverCaret {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.3".into(),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    #[test]
    fn semver_type_mismatch() {
        let ctx = [user_ctx(&[(
            "version",
            ParameterValue::Str("1.2.3".into()),
        )])];
        let cond = Condition::SemverGte {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.0.0".into(),
        };
        assert!(matches!(
            evaluate_leaf(&cond, &input_with(&ctx)),
            Err(RuleEngineError::TypeMismatch { .. })
        ));
    }

    // ── Segment Membership ────────────────────────────────────────────────────

    #[test]
    fn in_segment_when_present() {
        let ctx: [Context; 0] = [];
        let seg = SegmentId::new();
        let mut input = input_with(&ctx);
        input.resolved_segments.insert(seg);
        let cond = Condition::InSegment(seg);
        assert_eq!(evaluate_leaf(&cond, &input), Ok(true));
    }

    #[test]
    fn in_segment_when_absent() {
        let ctx: [Context; 0] = [];
        let seg = SegmentId::new();
        let input = input_with(&ctx);
        let cond = Condition::InSegment(seg);
        assert_eq!(evaluate_leaf(&cond, &input), Ok(false));
    }

    #[test]
    fn not_in_segment_when_absent() {
        let ctx: [Context; 0] = [];
        let seg = SegmentId::new();
        let input = input_with(&ctx);
        let cond = Condition::NotInSegment(seg);
        assert_eq!(evaluate_leaf(&cond, &input), Ok(true));
    }

    #[test]
    fn not_in_segment_when_present() {
        let ctx: [Context; 0] = [];
        let seg = SegmentId::new();
        let mut input = input_with(&ctx);
        input.resolved_segments.insert(seg);
        let cond = Condition::NotInSegment(seg);
        assert_eq!(evaluate_leaf(&cond, &input), Ok(false));
    }

    // ── Cross-Flag ────────────────────────────────────────────────────────────

    #[test]
    fn flag_evaluated_as_match() {
        let ctx: [Context; 0] = [];
        let flag = FlagId::new();
        let variant = VariantId::new();
        let mut input = input_with(&ctx);
        input.evaluated_flags.insert(flag, variant);
        let cond = Condition::FlagEvaluatedAs {
            flag_id: flag,
            variant_id: variant,
        };
        assert_eq!(evaluate_leaf(&cond, &input), Ok(true));
    }

    #[test]
    fn flag_evaluated_as_mismatch_variant() {
        let ctx: [Context; 0] = [];
        let flag = FlagId::new();
        let variant_a = VariantId::new();
        let variant_b = VariantId::new();
        let mut input = input_with(&ctx);
        input.evaluated_flags.insert(flag, variant_a);
        let cond = Condition::FlagEvaluatedAs {
            flag_id: flag,
            variant_id: variant_b,
        };
        assert_eq!(evaluate_leaf(&cond, &input), Ok(false));
    }

    #[test]
    fn flag_evaluated_as_absent_flag() {
        let ctx: [Context; 0] = [];
        let flag = FlagId::new();
        let variant = VariantId::new();
        let input = input_with(&ctx);
        let cond = Condition::FlagEvaluatedAs {
            flag_id: flag,
            variant_id: variant,
        };
        assert_eq!(evaluate_leaf(&cond, &input), Ok(false));
    }

    // ── SemVer direct numeric comparison (Lt/Gt via numeric_cmp) ─────────────

    #[test]
    fn lt_semver_true() {
        let ctx = semver_ctx("1.1.0");
        let cond = Condition::Lt {
            context_type: "user".into(),
            param: "version".into(),
            value: ParameterValue::SemVer(semver::Version::parse("1.2.0").unwrap()),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(true));
    }

    #[test]
    fn gt_semver_false() {
        let ctx = semver_ctx("1.1.0");
        let cond = Condition::Gt {
            context_type: "user".into(),
            param: "version".into(),
            value: ParameterValue::SemVer(semver::Version::parse("1.2.0").unwrap()),
        };
        assert_eq!(evaluate_leaf(&cond, &input_with(&ctx)), Ok(false));
    }

    // ── parse_semver_req error path (bad version string) ─────────────────────

    #[test]
    fn semver_gte_invalid_version_returns_type_mismatch() {
        let ctx = semver_ctx("1.0.0");
        let cond = Condition::SemverGte {
            context_type: "user".into(),
            param: "version".into(),
            version: "not-a-version".into(),
        };
        assert!(matches!(
            evaluate_leaf(&cond, &input_with(&ctx)),
            Err(RuleEngineError::TypeMismatch { .. })
        ));
    }

    // ── param_value_type_name: Bool and Double mismatch paths ─────────────────

    #[test]
    fn numeric_cmp_type_mismatch_bool() {
        let ctx = [user_ctx(&[("flag", ParameterValue::Bool(true))])];
        let cond = Condition::Lt {
            context_type: "user".into(),
            param: "flag".into(),
            value: ParameterValue::Int(1),
        };
        let result = evaluate_leaf(&cond, &input_with(&ctx));
        assert!(
            matches!(result, Err(RuleEngineError::TypeMismatch { actual, .. }) if actual == "Bool")
        );
    }

    #[test]
    fn string_op_type_mismatch_double() {
        let ctx = [user_ctx(&[("score", ParameterValue::Double(3.1))])];
        let cond = Condition::Contains {
            context_type: "user".into(),
            param: "score".into(),
            substr: "3".into(),
        };
        let result = evaluate_leaf(&cond, &input_with(&ctx));
        assert!(
            matches!(result, Err(RuleEngineError::TypeMismatch { actual, .. }) if actual == "Double")
        );
    }

    #[test]
    fn string_op_type_mismatch_semver() {
        let ctx = semver_ctx("1.0.0");
        let cond = Condition::Contains {
            context_type: "user".into(),
            param: "version".into(),
            substr: "1".into(),
        };
        let result = evaluate_leaf(&cond, &input_with(&ctx));
        assert!(
            matches!(result, Err(RuleEngineError::TypeMismatch { actual, .. }) if actual == "SemVer")
        );
    }
}
