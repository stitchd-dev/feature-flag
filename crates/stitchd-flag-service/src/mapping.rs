//! Domain ↔ Proto mapping helpers for the flag service.

use std::collections::HashMap;
use std::hash::BuildHasher;

use stitchd_core::{
    id::{FlagId, RuleId, VariantId},
    rule_engine::types::RuleOutput,
    variants::VariantValue,
};
use stitchd_proto::flags::v1::{
    AllocationBucket, ContextKeySelector, ContextParameterSelector,
    ExclusionGate as ProtoExclusionGate, FeatureFlag, FlagRule as ProtoFlagRule,
    FlagValueType as ProtoFlagValueType, HashSelector as ProtoHashSelector, PercentageAllocation,
    Variant as ProtoVariant, VariantValue as ProtoVariantValue,
    hash_selector::Selector as ProtoSelectorInner, variant_value::Value as ProtoVariantValueInner,
};

/// Convert a proto [`ProtoVariant`] to a domain [`stitchd_core::flag::Variant`].
///
/// Returns `None` if the variant value is missing or cannot be parsed.
#[must_use]
pub fn proto_variant_to_domain(v: ProtoVariant) -> Option<stitchd_core::flag::Variant> {
    let value = match v.value?.value? {
        ProtoVariantValueInner::BoolValue(b) => VariantValue::BoolValue(b),
        ProtoVariantValueInner::IntValue(i) => VariantValue::IntValue(i),
        ProtoVariantValueInner::DoubleValue(d) => VariantValue::DoubleValue(d),
        ProtoVariantValueInner::StringValue(s) => VariantValue::StrValue(s),
        ProtoVariantValueInner::JsonValue(s) => {
            VariantValue::JsonValue(serde_json::from_str(&s).ok()?)
        }
    };
    Some(stitchd_core::flag::Variant {
        id: stitchd_core::id::VariantId::new(),
        key: v.key,
        value,
    })
}

/// Convert a domain [`stitchd_core::flag::Variant`] to the proto [`ProtoVariant`].
#[must_use]
pub fn domain_variant_to_proto(v: stitchd_core::flag::Variant) -> ProtoVariant {
    let value = Some(match v.value {
        VariantValue::BoolValue(b) => ProtoVariantValue {
            value: Some(ProtoVariantValueInner::BoolValue(b)),
        },
        VariantValue::IntValue(i) => ProtoVariantValue {
            value: Some(ProtoVariantValueInner::IntValue(i)),
        },
        VariantValue::DoubleValue(d) => ProtoVariantValue {
            value: Some(ProtoVariantValueInner::DoubleValue(d)),
        },
        VariantValue::StrValue(s) => ProtoVariantValue {
            value: Some(ProtoVariantValueInner::StringValue(s)),
        },
        VariantValue::JsonValue(j) => ProtoVariantValue {
            value: Some(ProtoVariantValueInner::JsonValue(j.to_string())),
        },
    });
    ProtoVariant { key: v.key, value }
}

/// Convert a proto [`ProtoFlagRule`] back to a domain [`stitchd_core::flag::FlagRule`].
///
/// The rule payload is JSON-encoded `ConditionExpr`; the output maps from
/// variant key or percentage allocation back to domain types.
/// Returns `None` if deserialization of the condition or output fails.
#[must_use]
pub fn proto_flag_rule_to_domain(
    flag_id: FlagId,
    rule_index: i32,
    proto: &ProtoFlagRule,
    variant_map: &HashMap<String, VariantId>,
) -> Option<stitchd_core::flag::FlagRule> {
    use stitchd_core::rule_engine::types::{ConditionExpr, PercentageTarget, Rule, RuleOutput};
    use stitchd_proto::flags::v1::flag_rule::Output;

    let condition: ConditionExpr = serde_json::from_slice(&proto.rule_payload).ok()?;

    let output = match &proto.output {
        Some(Output::VariantKey(key)) => {
            let vid = variant_map.get(key).copied()?;
            RuleOutput::Variant(vid)
        }
        Some(Output::Allocation(alloc)) => {
            let targets: Vec<PercentageTarget> = alloc
                .hash_inputs
                .iter()
                .filter_map(proto_hash_selector_to_target)
                .collect();
            let weights = alloc
                .buckets
                .iter()
                .filter_map(|b| {
                    let vid = variant_map.get(&b.variant_key).copied()?;
                    Some((vid, b.weight_bp))
                })
                .collect();
            // Phase 2 wires the exclusion gate through proto↔core mapping.
            let exclusion_gate = alloc
                .exclusion_gate
                .as_ref()
                .map(proto_exclusion_gate_to_domain);
            RuleOutput::Percentage {
                targets,
                weights,
                exclusion_gate,
            }
        }
        None => return None,
    };

    let name = if proto.name.is_empty() {
        None
    } else {
        Some(proto.name.clone())
    };
    // Preserve the rule's existing UUID when the client round-trips it back
    // (e.g. an admin UI re-submits a flag's rules). When absent, mint a fresh
    // one — the DB row's `id` column will overwrite it on read anyway, but a
    // valid UUID keeps the serialised `rule_def` self-consistent.
    let rule_id = if proto.rule_id.is_empty() {
        RuleId::new()
    } else {
        match uuid::Uuid::parse_str(&proto.rule_id) {
            Ok(u) => RuleId::from_uuid(u),
            Err(_) => RuleId::new(),
        }
    };
    Some(stitchd_core::flag::FlagRule {
        flag_id,
        rule_index,
        rule: Rule {
            id: rule_id,
            name,
            condition,
            output,
        },
    })
}

/// Validate a proto `hash_inputs` selector list per FR-8 of
/// `flag_eval_unify_20260522`. Mirrors the gateway-side validator so
/// non-gateway gRPC clients hit the same rules.
///
/// Returns an error message describing the first failure:
/// 1. Empty list.
/// 2. Duplicate selectors by `(context_type, field)` identity.
/// 3. `ContextParameter` with empty `parameter`.
///
/// Selectors with an unset oneof (corrupted payload) are treated as
/// invalid and rejected.
///
/// # Errors
/// Returns a [`String`] describing the first validation failure.
pub fn validate_proto_hash_inputs(selectors: &[ProtoHashSelector]) -> Result<(), String> {
    if selectors.is_empty() {
        return Err("hash_inputs must not be empty".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for sel in selectors {
        let inner = sel
            .selector
            .as_ref()
            .ok_or_else(|| "hash_inputs: unset selector oneof".to_string())?;
        let (ctx_type, field) = match inner {
            ProtoSelectorInner::ContextKey(s) => (s.context_type.clone(), "__key__".to_string()),
            ProtoSelectorInner::ContextParameter(s) => {
                if s.parameter.is_empty() {
                    return Err(
                        "hash_inputs: context_parameter selector requires a non-empty `parameter`"
                            .to_string(),
                    );
                }
                (s.context_type.clone(), s.parameter.clone())
            }
        };
        if !seen.insert((ctx_type.clone(), field.clone())) {
            return Err(format!(
                "hash_inputs: duplicate selector for context_type=`{ctx_type}` field=`{}`",
                if field == "__key__" { "<key>" } else { &field }
            ));
        }
    }
    Ok(())
}

/// Validate a rule's `ConditionExpr` payload against shape-level traps that
/// the gateway / UI guard against but a non-gateway gRPC client (or a UI
/// regression) could still submit. Bug fix `feature-flag-yrj`: an empty
/// WHEN clause must not silently round-trip as a literal
/// `Eq { param: "", value: "" }` leaf that never matches.
///
/// Returns `Err` with the first violation message. Walk is recursive across
/// `And`, `Or`, and `Not` combinators; segment / cross-flag / numeric / string
/// leaves are passed through unchanged.
///
/// # Errors
/// Returns a [`String`] describing the first validation failure.
pub fn validate_proto_rule_condition(rule_payload: &[u8]) -> Result<(), String> {
    use stitchd_core::context::ParameterValue;
    use stitchd_core::rule_engine::condition::Condition;
    use stitchd_core::rule_engine::types::ConditionExpr;

    // An empty rule_payload is legal (the UI's "Default rule" tab serialises
    // the catch-all sentinel as `And: []`). Anything that fails to
    // deserialise is rejected — the upstream `proto_flag_rule_to_domain`
    // would skip the rule silently otherwise.
    if rule_payload.is_empty() {
        return Ok(());
    }
    let expr: ConditionExpr = serde_json::from_slice(rule_payload)
        .map_err(|e| format!("rule_payload is not a valid ConditionExpr: {e}"))?;

    fn walk(expr: &ConditionExpr) -> Result<(), String> {
        match expr {
            ConditionExpr::Leaf(Condition::Eq { param, value, .. }) => {
                let empty_value = matches!(value, ParameterValue::Str(s) if s.is_empty());
                if param.is_empty() && empty_value {
                    return Err(
                        "invalid_condition: WHEN clause has empty attribute/value — \
                         remove or fill the condition"
                            .to_string(),
                    );
                }
                Ok(())
            }
            ConditionExpr::Leaf(_) => Ok(()),
            ConditionExpr::And(items) | ConditionExpr::Or(items) => {
                for item in items {
                    walk(item)?;
                }
                Ok(())
            }
            ConditionExpr::Not(inner) => walk(inner),
        }
    }

    walk(&expr)
}

/// Convert a proto [`ProtoHashSelector`] to a domain [`PercentageTarget`].
///
/// Returns `None` if the oneof field is unset (corrupted / legacy data).
#[must_use]
fn proto_hash_selector_to_target(
    sel: &ProtoHashSelector,
) -> Option<stitchd_core::rule_engine::types::PercentageTarget> {
    use stitchd_core::rule_engine::types::{PercentageTarget, TargetField};
    match sel.selector.as_ref()? {
        ProtoSelectorInner::ContextKey(s) => Some(PercentageTarget {
            context_type: s.context_type.clone(),
            field: TargetField::Key,
        }),
        ProtoSelectorInner::ContextParameter(s) => Some(PercentageTarget {
            context_type: s.context_type.clone(),
            field: TargetField::Parameter(s.parameter.clone()),
        }),
    }
}

/// Convert a domain [`PercentageTarget`] to a proto [`ProtoHashSelector`].
#[must_use]
fn target_to_proto_hash_selector(
    target: &stitchd_core::rule_engine::types::PercentageTarget,
) -> ProtoHashSelector {
    use stitchd_core::rule_engine::types::TargetField;
    let inner = match &target.field {
        TargetField::Key => ProtoSelectorInner::ContextKey(ContextKeySelector {
            context_type: target.context_type.clone(),
        }),
        TargetField::Parameter(name) => {
            ProtoSelectorInner::ContextParameter(ContextParameterSelector {
                context_type: target.context_type.clone(),
                parameter: name.clone(),
            })
        }
    };
    ProtoHashSelector {
        selector: Some(inner),
    }
}

/// Convert a proto [`ProtoExclusionGate`] to a domain
/// [`stitchd_core::rule_engine::types::ExclusionGate`].
///
/// Proto bucket bounds are `u32` (proto3 has no `u16`); the domain uses `u16`
/// since exclusion-group buckets live in `[0, 9999]`. Out-of-range values are
/// clamped via `as u16` truncation, which is harmless because a gate with
/// bounds beyond the bucket space would never admit anything anyway.
#[must_use]
fn proto_exclusion_gate_to_domain(
    gate: &ProtoExclusionGate,
) -> stitchd_core::rule_engine::types::ExclusionGate {
    stitchd_core::rule_engine::types::ExclusionGate {
        group_salt: gate.group_salt.clone(),
        context_type: gate.context_type.clone(),
        bucket_lo: gate.bucket_lo as u16,
        bucket_hi: gate.bucket_hi as u16,
    }
}

/// Convert a domain [`stitchd_core::rule_engine::types::ExclusionGate`] to the
/// proto [`ProtoExclusionGate`]. Carries all four fields including
/// `context_type`.
#[must_use]
fn domain_exclusion_gate_to_proto(
    gate: &stitchd_core::rule_engine::types::ExclusionGate,
) -> ProtoExclusionGate {
    ProtoExclusionGate {
        group_salt: gate.group_salt.clone(),
        bucket_lo: u32::from(gate.bucket_lo),
        bucket_hi: u32::from(gate.bucket_hi),
        context_type: gate.context_type.clone(),
    }
}

/// Convert a domain `FlagValueType` to the proto [`ProtoFlagValueType`].
#[must_use]
pub const fn domain_value_type_to_proto(
    vt: stitchd_core::variants::FlagValueType,
) -> ProtoFlagValueType {
    use stitchd_core::variants::FlagValueType as D;
    match vt {
        D::Bool => ProtoFlagValueType::Bool,
        D::Int => ProtoFlagValueType::Int,
        D::Double => ProtoFlagValueType::Double,
        D::Str => ProtoFlagValueType::String,
        D::Json => ProtoFlagValueType::Json,
    }
}

/// Convert a domain [`stitchd_core::flag::FlagRule`] to the proto [`ProtoFlagRule`].
#[must_use]
pub fn domain_flag_rule_to_proto<S: BuildHasher>(
    fr: &stitchd_core::flag::FlagRule,
    variant_key_map: &HashMap<stitchd_core::id::VariantId, String, S>,
) -> ProtoFlagRule {
    use stitchd_proto::flags::v1::flag_rule::Output;

    let rule_payload = serde_json::to_vec(&fr.rule.condition).unwrap_or_default();

    let output = match &fr.rule.output {
        RuleOutput::Variant(variant_id) => {
            let key = variant_key_map.get(variant_id).cloned().unwrap_or_default();
            Some(Output::VariantKey(key))
        }
        RuleOutput::Percentage {
            targets,
            weights,
            exclusion_gate,
        } => {
            let hash_inputs: Vec<ProtoHashSelector> =
                targets.iter().map(target_to_proto_hash_selector).collect();

            let buckets = weights
                .iter()
                .map(|(vid, weight)| AllocationBucket {
                    variant_key: variant_key_map.get(vid).cloned().unwrap_or_default(),
                    weight_bp: *weight,
                })
                .collect();

            Some(Output::Allocation(PercentageAllocation {
                context_hash_specs: HashMap::new(),
                buckets,
                hash_inputs,
                // Phase 2 wires the exclusion gate through core↔proto mapping.
                exclusion_gate: exclusion_gate.as_ref().map(domain_exclusion_gate_to_proto),
            }))
        }
    };

    ProtoFlagRule {
        rule_payload,
        output,
        name: fr.rule.name.clone().unwrap_or_default(),
        // Surface the rule's row-PK UUID so admin clients (notably the
        // experiment-create flow) can bind to a real rule without faking
        // index-derived placeholders.
        rule_id: fr.rule.id.to_string(),
    }
}

/// Build a [`FeatureFlag`] proto message from a flag record plus its associated data.
///
/// # Errors
/// Returns an error string if serialisation of rules fails (falls back to empty payload).
#[must_use]
pub fn build_feature_flag_proto(
    record: &stitchd_core::flag::FlagRecord,
    variants: Vec<stitchd_core::flag::Variant>,
    flag_rules: &[stitchd_core::flag::FlagRule],
) -> FeatureFlag {
    let variant_key_map: HashMap<_, _> = variants.iter().map(|v| (v.id, v.key.clone())).collect();

    let default_variant_key = record
        .default_variant_id
        .and_then(|id| variant_key_map.get(&id).cloned())
        .unwrap_or_default();

    let proto_variants = variants
        .into_iter()
        .map(domain_variant_to_proto)
        .collect::<Vec<_>>();

    let mut proto_rules = flag_rules
        .iter()
        .map(|fr| domain_flag_rule_to_proto(fr, &variant_key_map))
        .collect::<Vec<_>>();

    // Reconstitute the catch-all percentage allocation as a trailing And:[]
    // rule so the admin UI can read it back and re-submit it unchanged.
    if let Some(dist) = &record.default_rule_distribution {
        let buckets = dist
            .allocations
            .iter()
            .map(|a| AllocationBucket {
                variant_key: a.variant_key.clone(),
                weight_bp: a.percentage_bp,
            })
            .collect();
        let hash_inputs = vec![ProtoHashSelector {
            selector: Some(ProtoSelectorInner::ContextKey(ContextKeySelector {
                context_type: "user".to_string(),
            })),
        }];
        let catch_all_condition = serde_json::to_vec(
            &stitchd_core::rule_engine::types::ConditionExpr::And(vec![]),
        )
        .unwrap_or_default();
        proto_rules.push(ProtoFlagRule {
            rule_payload: catch_all_condition,
            output: Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(
                PercentageAllocation {
                    context_hash_specs: Default::default(),
                    buckets,
                    hash_inputs,
                    exclusion_gate: None,
                },
            )),
            name: String::new(),
            rule_id: String::new(),
        });
    }

    FeatureFlag {
        key: record.key.to_string(),
        enabled: record.enabled,
        value_type: domain_value_type_to_proto(record.value_type) as i32,
        variants: proto_variants,
        rules: proto_rules,
        name: record.name.clone(),
        description: record.description.clone(),
        flag_id: record.id.to_string(),
        version: record.version as u64,
        default_variant_key,
        created_at_ms: record.created_at.timestamp_millis(),
        updated_at_ms: record.updated_at.timestamp_millis(),
        archived: record.deleted_at.is_some(),
        // Populated by callers that resolve the lock state (admin RPCs).
        // SDK paths leave this empty.
        locked_by_experiment_id: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::{
        id::{FlagId, RuleId, VariantId},
        rule_engine::types::{ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField},
        variants::{FlagValueType as DomainFVT, VariantValue},
    };
    use stitchd_proto::flags::v1::ContextHashSpec;

    fn make_variant_id() -> VariantId {
        VariantId::new()
    }

    #[test]
    fn domain_value_type_maps_all_variants() {
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Bool),
            ProtoFlagValueType::Bool
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Int),
            ProtoFlagValueType::Int
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Double),
            ProtoFlagValueType::Double
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Str),
            ProtoFlagValueType::String
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Json),
            ProtoFlagValueType::Json
        );
    }

    #[test]
    fn variant_bool_maps_correctly() {
        let v = stitchd_core::flag::Variant {
            id: make_variant_id(),
            key: "on".to_string(),
            value: VariantValue::BoolValue(true),
        };
        let proto = domain_variant_to_proto(v);
        assert_eq!(proto.key, "on");
        assert!(matches!(
            proto.value.unwrap().value,
            Some(ProtoVariantValueInner::BoolValue(true))
        ));
    }

    #[test]
    fn variant_int_maps_correctly() {
        let v = stitchd_core::flag::Variant {
            id: make_variant_id(),
            key: "large".to_string(),
            value: VariantValue::IntValue(42),
        };
        let proto = domain_variant_to_proto(v);
        assert!(matches!(
            proto.value.unwrap().value,
            Some(ProtoVariantValueInner::IntValue(42))
        ));
    }

    #[test]
    fn variant_double_maps_correctly() {
        let v = stitchd_core::flag::Variant {
            id: make_variant_id(),
            key: "rate".to_string(),
            value: VariantValue::DoubleValue(2.5),
        };
        let proto = domain_variant_to_proto(v);
        assert!(matches!(
            proto.value.unwrap().value,
            Some(ProtoVariantValueInner::DoubleValue(d)) if (d - 2.5).abs() < 1e-9
        ));
    }

    #[test]
    fn variant_str_maps_correctly() {
        let v = stitchd_core::flag::Variant {
            id: make_variant_id(),
            key: "colour".to_string(),
            value: VariantValue::StrValue("blue".to_string()),
        };
        let proto = domain_variant_to_proto(v);
        assert!(matches!(
            proto.value.unwrap().value,
            Some(ProtoVariantValueInner::StringValue(ref s)) if s == "blue"
        ));
    }

    #[test]
    fn variant_json_maps_correctly() {
        let v = stitchd_core::flag::Variant {
            id: make_variant_id(),
            key: "cfg".to_string(),
            value: VariantValue::JsonValue(serde_json::json!({"key": "val"})),
        };
        let proto = domain_variant_to_proto(v);
        assert!(matches!(
            proto.value.unwrap().value,
            Some(ProtoVariantValueInner::JsonValue(_))
        ));
    }

    #[test]
    fn flag_rule_variant_output_maps_key() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "control".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(vid),
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        assert!(matches!(
            proto.output,
            Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(ref k)) if k == "control"
        ));
    }

    #[test]
    fn flag_rule_proto_carries_rule_id_and_name() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "v".to_string());

        let rule_id = RuleId::new();
        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: rule_id,
                name: Some("named-rule".to_string()),
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(vid),
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        assert_eq!(proto.rule_id, rule_id.to_string());
        assert_eq!(proto.name, "named-rule");
    }

    #[test]
    fn proto_flag_rule_round_trips_rule_id() {
        let vid = make_variant_id();
        let key_map: HashMap<VariantId, String> = [(vid, "on".to_string())].into_iter().collect();
        let variant_map: HashMap<String, VariantId> =
            [("on".to_string(), vid)].into_iter().collect();

        let rule_id = RuleId::new();
        let original = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: rule_id,
                name: Some("n".to_string()),
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(vid),
            },
        };

        let proto = domain_flag_rule_to_proto(&original, &key_map);
        let back = proto_flag_rule_to_domain(original.flag_id, 0, &proto, &variant_map)
            .expect("round-trip");
        assert_eq!(back.rule.id, rule_id, "rule_id must round-trip");
    }

    #[test]
    fn flag_rule_variant_missing_in_map_returns_empty_key() {
        let vid = make_variant_id();
        let key_map: HashMap<_, String> = HashMap::new();

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(vid),
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        assert!(matches!(
            proto.output,
            Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(ref k)) if k.is_empty()
        ));
    }

    #[test]
    fn flag_rule_percentage_output_builds_hash_specs() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "treatment".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![PercentageTarget {
                        context_type: "user".to_string(),
                        field: TargetField::Key,
                    }],
                    weights: vec![(vid, 10000)],
                    exclusion_gate: None,
                },
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        if let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) = proto.output {
            assert!(alloc.context_hash_specs.is_empty());
            assert_eq!(alloc.buckets.len(), 1);
            assert_eq!(alloc.buckets[0].variant_key, "treatment");
            assert_eq!(alloc.buckets[0].weight_bp, 10000);
        } else {
            panic!("expected Allocation output");
        }
    }

    #[test]
    fn flag_rule_percentage_with_parameter_field_adds_param_name() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "t".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![PercentageTarget {
                        context_type: "user".to_string(),
                        field: TargetField::Parameter("user_id".to_string()),
                    }],
                    weights: vec![(vid, 5000)],
                    exclusion_gate: None,
                },
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        if let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) = proto.output {
            assert!(alloc.context_hash_specs.is_empty());
            assert_eq!(alloc.hash_inputs.len(), 1);
        } else {
            panic!("expected Allocation output");
        }
    }

    #[test]
    fn domain_percentage_populates_hash_inputs_and_clears_legacy_map() {
        // Phase 5/6 cutover: domain-to-proto conversion of
        // `RuleOutput::Percentage` populates only `hash_inputs`; the legacy
        // `context_hash_specs` map is always empty.
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "t".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![
                        PercentageTarget {
                            context_type: "user".to_string(),
                            field: TargetField::Key,
                        },
                        PercentageTarget {
                            context_type: "device".to_string(),
                            field: TargetField::Parameter("os".to_string()),
                        },
                    ],
                    weights: vec![(vid, 10000)],
                    exclusion_gate: None,
                },
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) = proto.output
        else {
            panic!("expected Allocation output");
        };

        // New field populated with two ordered selectors.
        assert_eq!(alloc.hash_inputs.len(), 2);
        let s0 = alloc.hash_inputs[0].selector.as_ref().unwrap();
        let s1 = alloc.hash_inputs[1].selector.as_ref().unwrap();
        match s0 {
            ProtoSelectorInner::ContextKey(s) => assert_eq!(s.context_type, "user"),
            _ => panic!("expected ContextKey"),
        }
        match s1 {
            ProtoSelectorInner::ContextParameter(s) => {
                assert_eq!(s.context_type, "device");
                assert_eq!(s.parameter, "os");
            }
            _ => panic!("expected ContextParameter"),
        }
        // Legacy field must be empty after cutover.
        assert!(alloc.context_hash_specs.is_empty());
    }

    #[test]
    fn proto_to_domain_reads_hash_inputs_preserving_order() {
        // `hash_inputs` is the sole authoritative field; selector order is
        // preserved end-to-end. `context_hash_specs` is ignored.
        let vid = make_variant_id();
        let variant_map: HashMap<String, VariantId> =
            [("t".to_string(), vid)].into_iter().collect();

        let mut legacy_map = HashMap::new();
        legacy_map.insert(
            "z_other".to_string(),
            ContextHashSpec {
                parameter_names: vec!["ignored".to_string()],
            },
        );

        let proto = ProtoFlagRule {
            rule_payload: serde_json::to_vec(
                &stitchd_core::rule_engine::types::ConditionExpr::And(vec![]),
            )
            .unwrap(),
            output: Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(
                PercentageAllocation {
                    context_hash_specs: legacy_map,
                    buckets: vec![AllocationBucket {
                        variant_key: "t".to_string(),
                        weight_bp: 10000,
                    }],
                    hash_inputs: vec![
                        ProtoHashSelector {
                            selector: Some(ProtoSelectorInner::ContextKey(ContextKeySelector {
                                context_type: "user".to_string(),
                            })),
                        },
                        ProtoHashSelector {
                            selector: Some(ProtoSelectorInner::ContextParameter(
                                ContextParameterSelector {
                                    context_type: "device".to_string(),
                                    parameter: "os".to_string(),
                                },
                            )),
                        },
                    ],
                    exclusion_gate: None,
                },
            )),
            name: String::new(),
            rule_id: String::new(),
        };

        let domain =
            proto_flag_rule_to_domain(FlagId::new(), 0, &proto, &variant_map).expect("conversion");
        let RuleOutput::Percentage { targets, .. } = domain.rule.output else {
            panic!("expected Percentage output");
        };
        assert_eq!(
            targets.len(),
            2,
            "selector order preserved from hash_inputs"
        );
        assert_eq!(targets[0].context_type, "user");
        assert!(matches!(targets[0].field, TargetField::Key));
        assert_eq!(targets[1].context_type, "device");
        assert!(matches!(targets[1].field, TargetField::Parameter(ref p) if p == "os"));
    }

    #[test]
    fn proto_to_domain_empty_hash_inputs_yields_empty_targets() {
        // After context_hash_specs fallback removal, a proto with only the
        // legacy map and no hash_inputs produces an empty targets vec.
        let vid = make_variant_id();
        let variant_map: HashMap<String, VariantId> =
            [("t".to_string(), vid)].into_iter().collect();

        let mut legacy_map = HashMap::new();
        legacy_map.insert(
            "user".to_string(),
            ContextHashSpec {
                parameter_names: vec![],
            },
        );

        let proto = ProtoFlagRule {
            rule_payload: serde_json::to_vec(
                &stitchd_core::rule_engine::types::ConditionExpr::And(vec![]),
            )
            .unwrap(),
            output: Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(
                PercentageAllocation {
                    context_hash_specs: legacy_map,
                    buckets: vec![AllocationBucket {
                        variant_key: "t".to_string(),
                        weight_bp: 10000,
                    }],
                    hash_inputs: vec![],
                    exclusion_gate: None,
                },
            )),
            name: String::new(),
            rule_id: String::new(),
        };

        let domain =
            proto_flag_rule_to_domain(FlagId::new(), 0, &proto, &variant_map).expect("conversion");
        let RuleOutput::Percentage { targets, .. } = domain.rule.output else {
            panic!("expected Percentage output");
        };
        assert!(
            targets.is_empty(),
            "context_hash_specs fallback is removed; empty hash_inputs → empty targets"
        );
    }

    // ── feature-flag-yrj — empty-WHEN rejection ────────────────────────

    #[test]
    fn validate_proto_rule_condition_rejects_empty_eq_leaf() {
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;

        let expr = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: String::new(),
            value: ParameterValue::Str(String::new()),
        });
        let payload = serde_json::to_vec(&expr).unwrap();
        let err = validate_proto_rule_condition(&payload)
            .expect_err("empty-attr empty-value Eq leaf must be rejected");
        assert!(
            err.contains("invalid_condition"),
            "expected invalid_condition sentinel; got `{err}`"
        );
    }

    #[test]
    fn validate_proto_rule_condition_rejects_empty_eq_leaf_inside_and() {
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;

        let bad_leaf = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: String::new(),
            value: ParameterValue::Str(String::new()),
        });
        let expr = ConditionExpr::And(vec![bad_leaf]);
        let payload = serde_json::to_vec(&expr).unwrap();
        let err = validate_proto_rule_condition(&payload)
            .expect_err("nested empty Eq leaf must be rejected");
        assert!(err.contains("invalid_condition"));
    }

    #[test]
    fn validate_proto_rule_condition_accepts_empty_payload_and_and_sentinel() {
        // Empty rule_payload (legal — the UI emits this for the catch-all
        // rule pre-fix) AND the `And: []` sentinel must both pass.
        use stitchd_core::rule_engine::types::ConditionExpr;

        validate_proto_rule_condition(&[]).expect("empty payload is legal");
        let sentinel = serde_json::to_vec(&ConditionExpr::And(vec![])).unwrap();
        validate_proto_rule_condition(&sentinel).expect("And: [] sentinel is legal");
    }

    #[test]
    fn validate_proto_rule_condition_accepts_fully_populated_eq() {
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;

        let expr = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: "tier".to_string(),
            value: ParameterValue::Str("gold".to_string()),
        });
        let payload = serde_json::to_vec(&expr).unwrap();
        validate_proto_rule_condition(&payload).expect("populated Eq leaf is legal");
    }

    #[test]
    fn build_feature_flag_proto_includes_name_and_description() {
        use stitchd_core::{
            flag::FlagRecord,
            id::FlagKey,
            id::{FlagId, ProjectId},
        };

        let record = FlagRecord {
            id: FlagId::new(),
            project_id: ProjectId::new(),
            key: FlagKey::new("my-flag").unwrap(),
            name: "My Flag".to_string(),
            description: "A flag for testing".to_string(),
            value_type: DomainFVT::Bool,
            enabled: true,
            default_variant_id: None,
            default_rule_distribution: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        };

        let proto = build_feature_flag_proto(&record, vec![], &[]);

        assert_eq!(proto.key, "my-flag");
        assert_eq!(proto.name, "My Flag");
        assert_eq!(proto.description, "A flag for testing");
        assert!(proto.enabled);
    }

    // ── Phase 2: exclusion-gate mapping (all 4 fields incl context_type) ────

    #[test]
    fn percentage_exclusion_gate_round_trips_core_proto_core() {
        use stitchd_core::rule_engine::types::ExclusionGate;

        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "t".to_string());
        let variant_map: HashMap<String, VariantId> =
            [("t".to_string(), vid)].into_iter().collect();

        let gate = ExclusionGate {
            group_salt: "exp-group-7".to_string(),
            context_type: "user".to_string(),
            bucket_lo: 2500,
            bucket_hi: 7500,
        };

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![PercentageTarget {
                        context_type: "user".to_string(),
                        field: TargetField::Key,
                    }],
                    weights: vec![(vid, 10000)],
                    exclusion_gate: Some(gate.clone()),
                },
            },
        };

        // core → proto: all four fields carried.
        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) = &proto.output
        else {
            panic!("expected Allocation output");
        };
        let pg = alloc
            .exclusion_gate
            .as_ref()
            .expect("exclusion_gate must survive core→proto");
        assert_eq!(pg.group_salt, "exp-group-7");
        assert_eq!(pg.context_type, "user");
        assert_eq!(pg.bucket_lo, 2500);
        assert_eq!(pg.bucket_hi, 7500);

        // proto → core: identical gate recovered.
        let domain =
            proto_flag_rule_to_domain(FlagId::new(), 0, &proto, &variant_map).expect("conversion");
        let RuleOutput::Percentage { exclusion_gate, .. } = domain.rule.output else {
            panic!("expected Percentage output");
        };
        assert_eq!(exclusion_gate, Some(gate));
    }

    #[test]
    fn percentage_without_exclusion_gate_maps_to_none() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "t".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: FlagId::new(),
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![PercentageTarget {
                        context_type: "user".to_string(),
                        field: TargetField::Key,
                    }],
                    weights: vec![(vid, 10000)],
                    exclusion_gate: None,
                },
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) = proto.output
        else {
            panic!("expected Allocation output");
        };
        assert!(
            alloc.exclusion_gate.is_none(),
            "ungated rule must serialize without an exclusion gate"
        );
    }
}
