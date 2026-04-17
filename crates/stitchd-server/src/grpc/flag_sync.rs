//! gRPC implementation of `FlagSyncService`.
//!
//! Serves flag and segment definitions to SDK clients. The SDK authenticates
//! by passing its raw key in the `x-sdk-key` gRPC metadata header. The server
//! hashes the key (SHA-256) and resolves the environment from the stored record.

use std::collections::HashMap;

use tonic::{Request, Response, Status};

use stitchd_core::{
    id::EnvironmentId,
    rule_engine::types::{RuleOutput, TargetField},
    segment::SegmentType,
    variants::VariantValue,
};
use stitchd_proto::{
    flags::v1::{
        AllocationBucket, ContextHashSpec, FeatureFlag, FlagRule, FlagValueType,
        PercentageAllocation, SyncRequest, SyncResponse, Variant as ProtoVariant,
        VariantValue as ProtoVariantValue, flag_sync_service_server::FlagSyncService,
        variant_value::Value as ProtoVariantValueInner,
    },
    segments::v1::{ListSegmentMeta, RuleSegment},
};

use crate::{AppState, api::sdk_auth::hash_sdk_key};

/// gRPC implementation of `FlagSyncService`.
pub struct FlagSyncServiceImpl {
    pub(crate) state: AppState,
}

impl FlagSyncServiceImpl {
    /// Create a new instance backed by the given app state.
    #[must_use]
    pub const fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Extract and validate the SDK key from gRPC metadata.
    ///
    /// Returns the environment ID the key is scoped to.
    async fn authenticate(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<EnvironmentId, Status> {
        let raw_key = metadata
            .get("x-sdk-key")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing x-sdk-key metadata"))?;

        let hash = hash_sdk_key(raw_key);

        let sdk_key = self
            .state
            .sdk_key_repo
            .find_active_by_hash(&hash)
            .await
            .map_err(|_| Status::unauthenticated("invalid or revoked SDK key"))?;

        Ok(sdk_key.environment_id)
    }
}

#[tonic::async_trait]
impl FlagSyncService for FlagSyncServiceImpl {
    #[allow(clippy::too_many_lines)]
    async fn sync(&self, request: Request<SyncRequest>) -> Result<Response<SyncResponse>, Status> {
        let env_id = self.authenticate(request.metadata()).await?;

        // ── Load flags ──────────────────────────────────────────────────────
        let flag_records = self
            .state
            .flag_repo
            .list_by_environment(env_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut proto_flags = Vec::with_capacity(flag_records.len());
        for record in &flag_records {
            let variants = self
                .state
                .variant_repo
                .find_by_flag(record.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;

            let flag_rules = self
                .state
                .flag_repo
                .find_rules(record.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;

            // Build a variant_id → variant_key lookup for percentage outputs.
            let variant_key_map: HashMap<_, _> =
                variants.iter().map(|v| (v.id, v.key.clone())).collect();

            let proto_variants = variants
                .into_iter()
                .map(domain_variant_to_proto)
                .collect::<Vec<_>>();

            let proto_rules = flag_rules
                .iter()
                .map(|fr| domain_flag_rule_to_proto(fr, &variant_key_map))
                .collect::<Vec<_>>();

            proto_flags.push(FeatureFlag {
                key: record.key.to_string(),
                enabled: record.enabled,
                value_type: domain_value_type_to_proto(record.value_type) as i32,
                variants: proto_variants,
                rules: proto_rules,
            });
        }

        // ── Load segments ───────────────────────────────────────────────────
        let segment_records = self
            .state
            .segment_repo
            .list_by_environment(env_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut rule_segments: Vec<RuleSegment> = Vec::new();
        let mut list_segments: Vec<ListSegmentMeta> = Vec::new();

        for seg in &segment_records {
            match seg.segment_type {
                SegmentType::Rule => {
                    let rule_seg = self
                        .state
                        .segment_repo
                        .find_with_rules(seg.id)
                        .await
                        .map_err(|e| Status::internal(e.to_string()))?;

                    let rule_payload = serde_json::to_vec(&rule_seg.rules)
                        .map_err(|e| Status::internal(e.to_string()))?;

                    rule_segments.push(RuleSegment {
                        key: seg.key.clone(),
                        context_type: String::new(),
                        rule_payload,
                        id: seg.id.to_string(),
                    });
                }
                SegmentType::List => {
                    let list_seg = self
                        .state
                        .segment_repo
                        .find_with_list(seg.id)
                        .await
                        .map_err(|e| Status::internal(e.to_string()))?;

                    // Emit one ListSegmentMeta per context_type covered by this segment.
                    for context_type in list_seg.lists.keys() {
                        list_segments.push(ListSegmentMeta {
                            key: seg.key.clone(),
                            context_type: context_type.clone(),
                            id: seg.id.to_string(),
                        });
                    }
                }
            }
        }

        Ok(Response::new(SyncResponse {
            flags: proto_flags,
            server_timestamp_ms: chrono::Utc::now().timestamp_millis(),
            rule_segments,
            list_segments,
            environment_id: env_id.to_string(),
        }))
    }
}

// ── Domain → Proto mapping helpers ──────────────────────────────────────────

fn domain_variant_to_proto(v: stitchd_core::flag::Variant) -> ProtoVariant {
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

const fn domain_value_type_to_proto(vt: stitchd_core::variants::FlagValueType) -> FlagValueType {
    use stitchd_core::variants::FlagValueType as DomainFVT;
    match vt {
        DomainFVT::Bool => FlagValueType::Bool,
        DomainFVT::Int => FlagValueType::Int,
        DomainFVT::Double => FlagValueType::Double,
        DomainFVT::Str => FlagValueType::String,
        DomainFVT::Json => FlagValueType::Json,
    }
}

fn domain_flag_rule_to_proto(
    fr: &stitchd_core::flag::FlagRule,
    variant_key_map: &HashMap<stitchd_core::id::VariantId, String>,
) -> FlagRule {
    use stitchd_proto::flags::v1::flag_rule::Output;

    // Serialise the condition expression as opaque JSON bytes.
    let rule_payload = serde_json::to_vec(&fr.rule.condition).unwrap_or_default();

    let output = match &fr.rule.output {
        RuleOutput::Variant(variant_id) => {
            let key = variant_key_map.get(variant_id).cloned().unwrap_or_default();
            Some(Output::VariantKey(key))
        }
        RuleOutput::Percentage { targets, weights } => {
            // Group targets by context_type → ContextHashSpec.
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
                // TargetField::Key means use context.key — empty parameter_names
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

    FlagRule {
        rule_payload,
        output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::{
        id::{RuleId, VariantId},
        rule_engine::types::{ConditionExpr, PercentageTarget, RuleOutput, TargetField},
        variants::{FlagValueType as DomainFVT, VariantValue},
    };

    fn make_variant_id() -> VariantId {
        VariantId::new()
    }

    #[test]
    fn domain_value_type_maps_all_variants() {
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Bool),
            FlagValueType::Bool
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Int),
            FlagValueType::Int
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Double),
            FlagValueType::Double
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Str),
            FlagValueType::String
        );
        assert_eq!(
            domain_value_type_to_proto(DomainFVT::Json),
            FlagValueType::Json
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
    fn flag_rule_variant_output_maps_key() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "control".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: stitchd_core::id::FlagId::new(),
            rule_index: 0,
            rule: stitchd_core::rule_engine::types::Rule {
                id: RuleId::new(),
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
    fn flag_rule_percentage_output_builds_hash_specs() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "treatment".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: stitchd_core::id::FlagId::new(),
            rule_index: 0,
            rule: stitchd_core::rule_engine::types::Rule {
                id: RuleId::new(),
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
}
