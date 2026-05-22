//! Domain ↔ Proto mapping helpers for the flag service.

use std::collections::HashMap;
use std::hash::BuildHasher;

use stitchd_core::{
    id::{FlagId, RuleId, VariantId},
    rule_engine::types::{RuleOutput, TargetField},
    variants::VariantValue,
};
use stitchd_proto::flags::v1::{
    AllocationBucket, ContextHashSpec, FeatureFlag, FlagRule as ProtoFlagRule,
    FlagValueType as ProtoFlagValueType, PercentageAllocation, Variant as ProtoVariant,
    VariantValue as ProtoVariantValue, variant_value::Value as ProtoVariantValueInner,
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
    use stitchd_core::rule_engine::types::{
        ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField,
    };
    use stitchd_proto::flags::v1::flag_rule::Output;

    let condition: ConditionExpr = serde_json::from_slice(&proto.rule_payload).ok()?;

    let output = match &proto.output {
        Some(Output::VariantKey(key)) => {
            let vid = variant_map.get(key).copied()?;
            RuleOutput::Variant(vid)
        }
        Some(Output::Allocation(alloc)) => {
            let mut targets: Vec<PercentageTarget> = Vec::new();
            for (ctx_type, spec) in &alloc.context_hash_specs {
                if spec.parameter_names.is_empty() {
                    targets.push(PercentageTarget {
                        context_type: ctx_type.clone(),
                        field: TargetField::Key,
                    });
                } else {
                    for param in &spec.parameter_names {
                        targets.push(PercentageTarget {
                            context_type: ctx_type.clone(),
                            field: TargetField::Parameter(param.clone()),
                        });
                    }
                }
            }
            let weights = alloc
                .buckets
                .iter()
                .filter_map(|b| {
                    let vid = variant_map.get(&b.variant_key).copied()?;
                    Some((vid, b.weight_milli))
                })
                .collect();
            RuleOutput::Percentage { targets, weights }
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
        RuleOutput::Percentage { targets, weights } => {
            let mut context_hash_specs: HashMap<String, ContextHashSpec> = HashMap::new();
            for target in targets {
                let spec = context_hash_specs
                    .entry(target.context_type.clone())
                    .or_insert_with(|| ContextHashSpec {
                        parameter_names: Vec::new(),
                    });
                if let TargetField::Parameter(name) = &target.field {
                    spec.parameter_names.push(name.clone());
                }
            }

            let buckets = weights
                .iter()
                .map(|(vid, weight)| AllocationBucket {
                    variant_key: variant_key_map.get(vid).cloned().unwrap_or_default(),
                    weight_milli: *weight,
                })
                .collect();

            Some(Output::Allocation(PercentageAllocation {
                context_hash_specs,
                buckets,
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

    let proto_rules = flag_rules
        .iter()
        .map(|fr| domain_flag_rule_to_proto(fr, &variant_key_map))
        .collect::<Vec<_>>();

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
                    weights: vec![(vid, 1000)],
                },
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        if let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) = proto.output {
            assert!(alloc.context_hash_specs.contains_key("user"));
            assert_eq!(alloc.buckets.len(), 1);
            assert_eq!(alloc.buckets[0].variant_key, "treatment");
            assert_eq!(alloc.buckets[0].weight_milli, 1000);
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
                    weights: vec![(vid, 500)],
                },
            },
        };

        let proto = domain_flag_rule_to_proto(&flag_rule, &key_map);
        if let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) = proto.output {
            let spec = alloc.context_hash_specs.get("user").unwrap();
            assert!(spec.parameter_names.contains(&"user_id".to_string()));
        } else {
            panic!("expected Allocation output");
        }
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
}
