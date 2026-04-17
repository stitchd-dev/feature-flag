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

        Ok(Self { flags, rule_segments, list_segments, environment_id })
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
        let uuid = Uuid::parse_str(&rs.id)
            .map_err(|e| SdkError::Deserialization(format!("invalid segment id '{}': {e}", rs.id)))?;
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
        let uuid = Uuid::parse_str(&ls.id)
            .map_err(|e| SdkError::Deserialization(format!("invalid segment id '{}': {e}", ls.id)))?;
        let seg_id = SegmentId::from_uuid(uuid);
        map.insert(seg_id, SdkListSegmentMeta {
            key: ls.key.clone(),
            id: seg_id,
            context_type: ls.context_type.clone(),
        });
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
                            vec![PercentageTarget { context_type: ctx_type.clone(), field: TargetField::Key }]
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
                        key_to_vid.get(&b.variant_key).map(|&vid| (vid, b.weight_milli))
                    })
                    .collect();

                RuleOutput::Percentage { targets, weights }
            }
            None => continue,
        };

        rules.push(Rule { id: RuleId::new(), condition, output });
    }

    Ok(SdkFlagDef { key: flag.key, enabled: flag.enabled, rules, variant_map })
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
                value: Some(stitchd_proto::flags::v1::variant_value::Value::BoolValue(val)),
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
            condition: ConditionExpr::And(vec![
                ConditionExpr::Leaf(Condition::InSegment(sid)),
            ]),
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
            variants: vec![bool_variant_proto("on", true), bool_variant_proto("off", false)],
            rules: vec![stitchd_proto::flags::v1::FlagRule {
                rule_payload: serde_json::to_vec(&always_true_condition()).unwrap(),
                output: Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey("on".to_string())),
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
        assert_eq!(
            cache.environment_id.unwrap().as_uuid(),
            env_uuid
        );
    }
}
