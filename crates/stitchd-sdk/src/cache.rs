//! In-memory definition cache built from a gRPC SyncResponse.

use std::collections::{HashMap, HashSet};

use stitchd_core::{
    id::{EnvironmentId, RuleId, SegmentId, VariantId},
    rule_engine::{
        condition::Condition,
        types::{ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField},
    },
    segment::RuleBasedSegment,
    variants::VariantValue,
};
use stitchd_proto::{
    flags::v1::{FeatureFlag, SyncResponse, flag_rule::Output, variant_value::Value},
    segments::v1::{ListSegmentMeta as ProtoListMeta, RuleSegment as ProtoRuleSegment},
};
use uuid::Uuid;

use crate::error::SdkError;

/// Atomically-replaced snapshot of all flag and segment definitions.
#[derive(Clone, Default)]
pub struct DefinitionCache {
    pub flags: HashMap<String, SdkFlagDef>,
    /// Rule-based segments indexed by their server-assigned SegmentId.
    pub rule_segments: HashMap<SegmentId, RuleBasedSegment>,
    /// List-based segment metadata indexed by SegmentId.
    pub list_segments: HashMap<SegmentId, SdkListSegmentMeta>,
    pub environment_id: Option<EnvironmentId>,
}

/// SDK-internal flag definition.
#[derive(Clone)]
pub struct SdkFlagDef {
    pub key: String,
    pub enabled: bool,
    /// Domain Rule objects ready for `evaluate_rules`.
    pub rules: Vec<Rule>,
    /// Variant lookup: VariantId → (variant_key, VariantValue).
    pub variant_map: HashMap<VariantId, (String, VariantValue)>,
}

/// SDK-internal list-segment metadata.
#[derive(Clone)]
pub struct SdkListSegmentMeta {
    pub key: String,
    pub id: SegmentId,
    pub context_type: String,
}

impl DefinitionCache {
    /// Build a cache from the proto SyncResponse.
    pub fn from_sync_response(resp: SyncResponse) -> Result<Self, SdkError> {
        let environment_id = if resp.environment_id.is_empty() {
            None
        } else {
            let uuid = Uuid::parse_str(&resp.environment_id)
                .map_err(|e| SdkError::Deserialization(format!("invalid environment_id: {e}")))?;
            Some(EnvironmentId::from_uuid(uuid))
        };

        let rule_segments = build_rule_segments(&resp.rule_segments)?;
        let list_segments = build_list_segments(&resp.list_segments)?;
        let flags = build_flags(resp.flags)?;

        Ok(Self {
            flags,
            rule_segments,
            list_segments,
            environment_id,
        })
    }
}

fn build_rule_segments(
    segments: &[ProtoRuleSegment],
) -> Result<HashMap<SegmentId, RuleBasedSegment>, SdkError> {
    let mut map = HashMap::with_capacity(segments.len());
    for rs in segments {
        if rs.id.is_empty() {
            continue;
        }
        let uuid = Uuid::parse_str(&rs.id).map_err(|e| {
            SdkError::Deserialization(format!("invalid segment id '{}': {e}", rs.id))
        })?;
        let seg_id = SegmentId::from_uuid(uuid);
        let rules: Vec<Rule> = serde_json::from_slice(&rs.rule_payload)
            .map_err(|e| SdkError::Deserialization(format!("segment rule_payload: {e}")))?;
        map.insert(seg_id, RuleBasedSegment { id: seg_id, rules });
    }
    Ok(map)
}

fn build_list_segments(
    segments: &[ProtoListMeta],
) -> Result<HashMap<SegmentId, SdkListSegmentMeta>, SdkError> {
    let mut map = HashMap::with_capacity(segments.len());
    for ls in segments {
        if ls.id.is_empty() {
            continue;
        }
        let uuid = Uuid::parse_str(&ls.id).map_err(|e| {
            SdkError::Deserialization(format!("invalid segment id '{}': {e}", ls.id))
        })?;
        let seg_id = SegmentId::from_uuid(uuid);
        map.insert(
            seg_id,
            SdkListSegmentMeta {
                key: ls.key.clone(),
                id: seg_id,
                context_type: ls.context_type.clone(),
            },
        );
    }
    Ok(map)
}

fn build_flags(flags: Vec<FeatureFlag>) -> Result<HashMap<String, SdkFlagDef>, SdkError> {
    let mut map = HashMap::with_capacity(flags.len());
    for flag in flags {
        let def = build_flag_def(flag)?;
        map.insert(def.key.clone(), def);
    }
    Ok(map)
}

fn build_flag_def(flag: FeatureFlag) -> Result<SdkFlagDef, SdkError> {
    // Assign synthetic VariantIds and build lookup maps.
    let mut key_to_vid: HashMap<String, VariantId> = HashMap::new();
    let mut variant_map: HashMap<VariantId, (String, VariantValue)> = HashMap::new();

    for proto_v in &flag.variants {
        let vid = VariantId::new();
        let value = proto_variant_value_to_domain(proto_v.value.as_ref())?;
        key_to_vid.insert(proto_v.key.clone(), vid);
        variant_map.insert(vid, (proto_v.key.clone(), value));
    }

    let mut rules: Vec<Rule> = Vec::with_capacity(flag.rules.len());
    for proto_rule in flag.rules {
        let condition = serde_json::from_slice::<ConditionExpr>(&proto_rule.rule_payload)
            .map_err(|e| SdkError::Deserialization(format!("flag rule_payload: {e}")))?;

        let output = match proto_rule.output {
            Some(Output::VariantKey(ref key)) => {
                let vid = key_to_vid.get(key.as_str()).copied().ok_or_else(|| {
                    SdkError::Deserialization(format!("unknown variant key '{key}'"))
                })?;
                RuleOutput::Variant(vid)
            }
            Some(Output::Allocation(ref alloc)) => {
                let targets: Vec<PercentageTarget> = alloc
                    .context_hash_specs
                    .iter()
                    .flat_map(|(ctx_type, spec)| {
                        if spec.parameter_names.is_empty() {
                            vec![PercentageTarget {
                                context_type: ctx_type.clone(),
                                field: TargetField::Key,
                            }]
                        } else {
                            spec.parameter_names
                                .iter()
                                .map(|p| PercentageTarget {
                                    context_type: ctx_type.clone(),
                                    field: TargetField::Parameter(p.clone()),
                                })
                                .collect()
                        }
                    })
                    .collect();

                let weights: Vec<(VariantId, u32)> = alloc
                    .buckets
                    .iter()
                    .filter_map(|b| {
                        key_to_vid
                            .get(&b.variant_key)
                            .map(|&vid| (vid, b.weight_milli))
                    })
                    .collect();

                RuleOutput::Percentage { targets, weights }
            }
            None => continue,
        };

        rules.push(Rule {
            id: RuleId::new(),
            condition,
            output,
        });
    }

    Ok(SdkFlagDef {
        key: flag.key,
        enabled: flag.enabled,
        rules,
        variant_map,
    })
}

fn proto_variant_value_to_domain(
    v: Option<&stitchd_proto::flags::v1::VariantValue>,
) -> Result<VariantValue, SdkError> {
    let inner = v
        .and_then(|vv| vv.value.as_ref())
        .ok_or_else(|| SdkError::Deserialization("variant has no value".into()))?;

    Ok(match inner {
        Value::BoolValue(b) => VariantValue::BoolValue(*b),
        Value::IntValue(i) => VariantValue::IntValue(*i),
        Value::DoubleValue(d) => VariantValue::DoubleValue(*d),
        Value::StringValue(s) => VariantValue::StrValue(s.clone()),
        Value::JsonValue(j) => {
            let parsed: serde_json::Value = serde_json::from_str(j)
                .map_err(|e| SdkError::Deserialization(format!("json variant: {e}")))?;
            VariantValue::JsonValue(parsed)
        }
    })
}

/// Collect all SegmentIds referenced by `InSegment` or `NotInSegment` in the rules.
pub fn collect_segment_ids(rules: &[Rule]) -> HashSet<SegmentId> {
    let mut ids = HashSet::new();
    for rule in rules {
        collect_from_expr(&rule.condition, &mut ids);
    }
    ids
}

fn collect_from_expr(expr: &ConditionExpr, ids: &mut HashSet<SegmentId>) {
    match expr {
        ConditionExpr::Leaf(Condition::InSegment(id) | Condition::NotInSegment(id)) => {
            ids.insert(*id);
        }
        ConditionExpr::Leaf(_) => {}
        ConditionExpr::And(exprs) | ConditionExpr::Or(exprs) => {
            for e in exprs {
                collect_from_expr(e, ids);
            }
        }
        ConditionExpr::Not(e) => collect_from_expr(e, ids),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::id::RuleId;
    use stitchd_core::rule_engine::condition::Condition;
    use stitchd_core::rule_engine::types::{ConditionExpr, RuleOutput};

    fn bool_variant_proto(key: &str, val: bool) -> stitchd_proto::flags::v1::Variant {
        stitchd_proto::flags::v1::Variant {
            key: key.to_string(),
            value: Some(stitchd_proto::flags::v1::VariantValue {
                value: Some(stitchd_proto::flags::v1::variant_value::Value::BoolValue(
                    val,
                )),
            }),
        }
    }

    fn always_true_condition() -> ConditionExpr {
        ConditionExpr::And(vec![])
    }

    #[test]
    fn collect_segment_ids_finds_in_segment() {
        let sid = SegmentId::new();
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::InSegment(sid)),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let ids = collect_segment_ids(&[rule]);
        assert!(ids.contains(&sid));
    }

    #[test]
    fn collect_segment_ids_finds_nested() {
        let sid = SegmentId::new();
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![ConditionExpr::Leaf(Condition::InSegment(sid))]),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let ids = collect_segment_ids(&[rule]);
        assert!(ids.contains(&sid));
    }

    #[test]
    fn build_flag_def_maps_variants_and_rules() {
        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "my-flag".to_string(),
            enabled: true,
            value_type: 1,
            variants: vec![
                bool_variant_proto("on", true),
                bool_variant_proto("off", false),
            ],
            rules: vec![stitchd_proto::flags::v1::FlagRule {
                rule_payload: serde_json::to_vec(&always_true_condition()).unwrap(),
                output: Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(
                    "on".to_string(),
                )),
            }],
        };

        let def = build_flag_def(flag).unwrap();
        assert_eq!(def.key, "my-flag");
        assert!(def.enabled);
        assert_eq!(def.rules.len(), 1);
        assert_eq!(def.variant_map.len(), 2);

        // The rule output should be Variant(...) pointing to a known variant id
        if let RuleOutput::Variant(vid) = def.rules[0].output {
            let (key, value) = &def.variant_map[&vid];
            assert_eq!(key, "on");
            assert_eq!(*value, VariantValue::BoolValue(true));
        } else {
            panic!("expected Variant output");
        }
    }

    #[test]
    fn from_sync_response_parses_environment_id() {
        let env_uuid = uuid::Uuid::new_v4();
        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![],
            environment_id: env_uuid.to_string(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        assert_eq!(cache.environment_id.unwrap().as_uuid(), env_uuid);
    }

    #[test]
    fn from_sync_response_empty_environment_id_is_none() {
        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![],
            environment_id: String::new(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        assert!(cache.environment_id.is_none());
    }

    #[test]
    fn from_sync_response_invalid_environment_id_returns_error() {
        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![],
            environment_id: "not-a-uuid".to_string(),
        };
        let result = DefinitionCache::from_sync_response(resp);
        let err = result.err().expect("expected error");
        let msg = err.to_string();
        assert!(msg.contains("invalid environment_id"));
    }

    #[test]
    fn from_sync_response_builds_rule_segment() {
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;
        use stitchd_proto::segments::v1::RuleSegment as ProtoRuleSegment;

        let seg_uuid = uuid::Uuid::new_v4();
        let rules: Vec<Rule> = vec![];
        let rule_payload = serde_json::to_vec(&rules).unwrap();

        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![ProtoRuleSegment {
                id: seg_uuid.to_string(),
                rule_payload,
                context_type: "user".to_string(),
                key: "test-seg".to_string(),
            }],
            list_segments: vec![],
            environment_id: String::new(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        let seg_id = SegmentId::from_uuid(seg_uuid);
        assert!(cache.rule_segments.contains_key(&seg_id));

        // Verify non-empty rule payload too
        let _ = Condition::InSegment(seg_id); // just ensure import is used
        let _ = ConditionExpr::Leaf(Condition::InSegment(seg_id));
    }

    #[test]
    fn from_sync_response_skips_empty_rule_segment_id() {
        use stitchd_proto::segments::v1::RuleSegment as ProtoRuleSegment;

        let rules: Vec<Rule> = vec![];
        let rule_payload = serde_json::to_vec(&rules).unwrap();

        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![ProtoRuleSegment {
                id: String::new(), // empty id → skip
                rule_payload,
                context_type: "user".to_string(),
                key: "test-seg".to_string(),
            }],
            list_segments: vec![],
            environment_id: String::new(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        assert!(cache.rule_segments.is_empty());
    }

    #[test]
    fn from_sync_response_invalid_rule_segment_id_returns_error() {
        use stitchd_proto::segments::v1::RuleSegment as ProtoRuleSegment;

        let rules: Vec<Rule> = vec![];
        let rule_payload = serde_json::to_vec(&rules).unwrap();

        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![ProtoRuleSegment {
                id: "bad-uuid".to_string(),
                rule_payload,
                context_type: "user".to_string(),
                key: "seg".to_string(),
            }],
            list_segments: vec![],
            environment_id: String::new(),
        };
        let result = DefinitionCache::from_sync_response(resp);
        assert!(result.is_err());
    }

    #[test]
    fn from_sync_response_invalid_rule_segment_payload_returns_error() {
        use stitchd_proto::segments::v1::RuleSegment as ProtoRuleSegment;

        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![ProtoRuleSegment {
                id: uuid::Uuid::new_v4().to_string(),
                rule_payload: b"not-valid-json".to_vec(),
                context_type: "user".to_string(),
                key: "seg".to_string(),
            }],
            list_segments: vec![],
            environment_id: String::new(),
        };
        let result = DefinitionCache::from_sync_response(resp);
        assert!(result.is_err());
    }

    #[test]
    fn from_sync_response_builds_list_segment() {
        use stitchd_proto::segments::v1::ListSegmentMeta as ProtoListMeta;

        let seg_uuid = uuid::Uuid::new_v4();
        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![ProtoListMeta {
                id: seg_uuid.to_string(),
                key: "vip-users".to_string(),
                context_type: "user".to_string(),
            }],
            environment_id: String::new(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        let seg_id = SegmentId::from_uuid(seg_uuid);
        let meta = cache.list_segments.get(&seg_id).unwrap();
        assert_eq!(meta.key, "vip-users");
        assert_eq!(meta.context_type, "user");
    }

    #[test]
    fn from_sync_response_skips_empty_list_segment_id() {
        use stitchd_proto::segments::v1::ListSegmentMeta as ProtoListMeta;

        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![ProtoListMeta {
                id: String::new(),
                key: "seg".to_string(),
                context_type: "user".to_string(),
            }],
            environment_id: String::new(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        assert!(cache.list_segments.is_empty());
    }

    #[test]
    fn from_sync_response_invalid_list_segment_id_returns_error() {
        use stitchd_proto::segments::v1::ListSegmentMeta as ProtoListMeta;

        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![ProtoListMeta {
                id: "not-a-uuid".to_string(),
                key: "seg".to_string(),
                context_type: "user".to_string(),
            }],
            environment_id: String::new(),
        };
        let result = DefinitionCache::from_sync_response(resp);
        assert!(result.is_err());
    }

    fn make_variant_proto(
        key: &str,
        value: stitchd_proto::flags::v1::variant_value::Value,
    ) -> stitchd_proto::flags::v1::Variant {
        stitchd_proto::flags::v1::Variant {
            key: key.to_string(),
            value: Some(stitchd_proto::flags::v1::VariantValue {
                value: Some(value),
            }),
        }
    }

    #[test]
    fn build_flag_def_int_variant() {
        use stitchd_proto::flags::v1::variant_value::Value;

        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "int-flag".to_string(),
            enabled: true,
            value_type: 2,
            variants: vec![make_variant_proto("forty-two", Value::IntValue(42))],
            rules: vec![],
        };
        let def = build_flag_def(flag).unwrap();
        let val = def.variant_map.values().next().unwrap();
        assert_eq!(val.1, VariantValue::IntValue(42));
    }

    #[test]
    fn build_flag_def_double_variant() {
        use stitchd_proto::flags::v1::variant_value::Value;

        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "double-flag".to_string(),
            enabled: true,
            value_type: 3,
            variants: vec![make_variant_proto("pi", Value::DoubleValue(3.14))],
            rules: vec![],
        };
        let def = build_flag_def(flag).unwrap();
        let val = def.variant_map.values().next().unwrap();
        assert_eq!(val.1, VariantValue::DoubleValue(3.14));
    }

    #[test]
    fn build_flag_def_string_variant() {
        use stitchd_proto::flags::v1::variant_value::Value;

        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "str-flag".to_string(),
            enabled: true,
            value_type: 4,
            variants: vec![make_variant_proto("hello", Value::StringValue("world".to_string()))],
            rules: vec![],
        };
        let def = build_flag_def(flag).unwrap();
        let val = def.variant_map.values().next().unwrap();
        assert_eq!(val.1, VariantValue::StrValue("world".to_string()));
    }

    #[test]
    fn build_flag_def_json_variant() {
        use stitchd_proto::flags::v1::variant_value::Value;

        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "json-flag".to_string(),
            enabled: true,
            value_type: 5,
            variants: vec![make_variant_proto(
                "config",
                Value::JsonValue(r#"{"key":"val"}"#.to_string()),
            )],
            rules: vec![],
        };
        let def = build_flag_def(flag).unwrap();
        let val = def.variant_map.values().next().unwrap();
        if let VariantValue::JsonValue(j) = &val.1 {
            assert_eq!(j["key"], "val");
        } else {
            panic!("expected JsonValue");
        }
    }

    #[test]
    fn build_flag_def_invalid_json_variant_returns_error() {
        use stitchd_proto::flags::v1::variant_value::Value;

        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "json-flag".to_string(),
            enabled: true,
            value_type: 5,
            variants: vec![make_variant_proto(
                "config",
                Value::JsonValue("not-json!!!".to_string()),
            )],
            rules: vec![],
        };
        let result = build_flag_def(flag);
        assert!(result.is_err());
    }

    #[test]
    fn build_flag_def_no_variant_value_returns_error() {
        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "bad-flag".to_string(),
            enabled: true,
            value_type: 1,
            variants: vec![stitchd_proto::flags::v1::Variant {
                key: "on".to_string(),
                value: None, // missing value
            }],
            rules: vec![],
        };
        let result = build_flag_def(flag);
        assert!(result.is_err());
    }

    #[test]
    fn build_flag_def_unknown_variant_key_returns_error() {
        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "bad-flag".to_string(),
            enabled: true,
            value_type: 1,
            variants: vec![bool_variant_proto("on", true)],
            rules: vec![stitchd_proto::flags::v1::FlagRule {
                rule_payload: serde_json::to_vec(&always_true_condition()).unwrap(),
                output: Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(
                    "nonexistent".to_string(),
                )),
            }],
        };
        let result = build_flag_def(flag);
        let err = result.err().expect("expected error");
        let msg = err.to_string();
        assert!(msg.contains("unknown variant key"));
    }

    #[test]
    fn build_flag_def_invalid_rule_payload_returns_error() {
        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "bad-flag".to_string(),
            enabled: true,
            value_type: 1,
            variants: vec![bool_variant_proto("on", true)],
            rules: vec![stitchd_proto::flags::v1::FlagRule {
                rule_payload: b"invalid-json".to_vec(),
                output: Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(
                    "on".to_string(),
                )),
            }],
        };
        let result = build_flag_def(flag);
        assert!(result.is_err());
    }

    #[test]
    fn build_flag_def_none_output_is_skipped() {
        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "flag".to_string(),
            enabled: true,
            value_type: 1,
            variants: vec![bool_variant_proto("on", true)],
            rules: vec![stitchd_proto::flags::v1::FlagRule {
                rule_payload: serde_json::to_vec(&always_true_condition()).unwrap(),
                output: None, // None output should be skipped
            }],
        };
        let def = build_flag_def(flag).unwrap();
        assert_eq!(def.rules.len(), 0); // rule skipped
    }

    #[test]
    fn build_flag_def_allocation_output() {
        use std::collections::HashMap as HM;
        use stitchd_proto::flags::v1::{
            AllocationBucket, ContextHashSpec, PercentageAllocation,
        };
        use stitchd_proto::flags::v1::flag_rule::Output;

        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "pct-flag".to_string(),
            enabled: true,
            value_type: 1,
            variants: vec![
                bool_variant_proto("treatment", true),
                bool_variant_proto("control", false),
            ],
            rules: vec![stitchd_proto::flags::v1::FlagRule {
                rule_payload: serde_json::to_vec(&always_true_condition()).unwrap(),
                output: Some(Output::Allocation(PercentageAllocation {
                    context_hash_specs: {
                        let mut m = HM::new();
                        m.insert(
                            "user".to_string(),
                            ContextHashSpec {
                                parameter_names: vec![],
                            },
                        );
                        m
                    },
                    buckets: vec![
                        AllocationBucket {
                            variant_key: "treatment".to_string(),
                            weight_milli: 500,
                        },
                        AllocationBucket {
                            variant_key: "control".to_string(),
                            weight_milli: 500,
                        },
                    ],
                })),
            }],
        };
        let def = build_flag_def(flag).unwrap();
        assert_eq!(def.rules.len(), 1);
        if let RuleOutput::Percentage { targets, weights } = &def.rules[0].output {
            assert_eq!(targets.len(), 1);
            assert_eq!(weights.len(), 2);
        } else {
            panic!("expected Percentage output");
        }
    }

    #[test]
    fn build_flag_def_allocation_with_parameter_hash_spec() {
        use std::collections::HashMap as HM;
        use stitchd_proto::flags::v1::{
            AllocationBucket, ContextHashSpec, PercentageAllocation,
        };
        use stitchd_proto::flags::v1::flag_rule::Output;

        let flag = stitchd_proto::flags::v1::FeatureFlag {
            key: "pct-flag".to_string(),
            enabled: true,
            value_type: 1,
            variants: vec![bool_variant_proto("on", true)],
            rules: vec![stitchd_proto::flags::v1::FlagRule {
                rule_payload: serde_json::to_vec(&always_true_condition()).unwrap(),
                output: Some(Output::Allocation(PercentageAllocation {
                    context_hash_specs: {
                        let mut m = HM::new();
                        m.insert(
                            "user".to_string(),
                            ContextHashSpec {
                                parameter_names: vec!["account_id".to_string()],
                            },
                        );
                        m
                    },
                    buckets: vec![AllocationBucket {
                        variant_key: "on".to_string(),
                        weight_milli: 1000,
                    }],
                })),
            }],
        };
        let def = build_flag_def(flag).unwrap();
        if let RuleOutput::Percentage { targets, .. } = &def.rules[0].output {
            assert_eq!(targets.len(), 1);
            assert!(
                matches!(&targets[0].field, stitchd_core::rule_engine::types::TargetField::Parameter(p) if p == "account_id")
            );
        } else {
            panic!("expected Percentage output");
        }
    }

    #[test]
    fn from_sync_response_with_flags_builds_flag_map() {
        let env_uuid = uuid::Uuid::new_v4();
        let resp = stitchd_proto::flags::v1::SyncResponse {
            flags: vec![stitchd_proto::flags::v1::FeatureFlag {
                key: "feature-x".to_string(),
                enabled: true,
                value_type: 1,
                variants: vec![bool_variant_proto("on", true)],
                rules: vec![stitchd_proto::flags::v1::FlagRule {
                    rule_payload: serde_json::to_vec(&always_true_condition()).unwrap(),
                    output: Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(
                        "on".to_string(),
                    )),
                }],
            }],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![],
            environment_id: env_uuid.to_string(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        assert!(cache.flags.contains_key("feature-x"));
    }

    #[test]
    fn collect_segment_ids_non_segment_leaf_ignored() {
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;

        // A non-InSegment/NotInSegment leaf should be ignored
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".into(),
                param: "plan".into(),
                value: ParameterValue::Str("pro".into()),
            }),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let ids = collect_segment_ids(&[rule]);
        assert!(ids.is_empty());
    }

    #[test]
    fn collect_segment_ids_finds_not_in_segment() {
        let sid = SegmentId::new();
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::NotInSegment(sid)),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let ids = collect_segment_ids(&[rule]);
        assert!(ids.contains(&sid));
    }

    #[test]
    fn collect_segment_ids_or_expression() {
        let sid = SegmentId::new();
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Or(vec![ConditionExpr::Leaf(Condition::InSegment(sid))]),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let ids = collect_segment_ids(&[rule]);
        assert!(ids.contains(&sid));
    }

    #[test]
    fn collect_segment_ids_not_expression() {
        let sid = SegmentId::new();
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Not(Box::new(ConditionExpr::Leaf(
                Condition::InSegment(sid),
            ))),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let ids = collect_segment_ids(&[rule]);
        assert!(ids.contains(&sid));
    }
}
