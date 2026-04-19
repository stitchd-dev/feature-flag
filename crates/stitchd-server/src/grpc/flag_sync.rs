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
    use async_trait::async_trait;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::sync::{Arc, Mutex};
    use stitchd_core::{
        flag::Variant,
        id::{EnvironmentId, FlagId, ProjectId, RuleId, SdkKeyId, SegmentId, VariantId},
        rule_engine::types::{ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField},
        segment::{ListBasedSegment, RuleBasedSegment, Segment},
        tenant::SdkKey,
        variants::{FlagValueType as DomainFVT, VariantValue},
    };
    use stitchd_db::{
        ContextMembership, FlagRepository, RepositoryError, SdkKeyRepository, SegmentRepository,
        VariantRepository,
    };

    fn make_variant_id() -> VariantId {
        VariantId::new()
    }

    // ── Minimal stubs for gRPC sync tests ────────────────────────────────────

    struct StubFlagRepo {
        flags: Mutex<Vec<stitchd_core::flag::FlagRecord>>,
    }

    impl StubFlagRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                flags: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl FlagRepository for StubFlagRepo {
        async fn find_by_id(
            &self,
            id: FlagId,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &stitchd_core::id::FlagKey,
            _project_id: ProjectId,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key.to_string(),
            })
        }

        async fn list_by_project(
            &self,
            _project_id: ProjectId,
        ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> {
            Ok(self.flags.lock().unwrap().clone())
        }

        async fn create(
            &self,
            _flag: &stitchd_core::flag::FlagRecord,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(
            &self,
            flag: &stitchd_core::flag::FlagRecord,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Ok(flag.clone())
        }

        async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_hashing_config(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn upsert_hashing_config(
            &self,
            _flag_id: FlagId,
            _config: &[stitchd_core::flag::FlagHashingConfig],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn find_rules(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::FlagRule>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn upsert_rules(
            &self,
            _flag_id: FlagId,
            _rules: &[stitchd_core::flag::FlagRule],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    struct StubVariantRepo;

    #[async_trait]
    impl VariantRepository for StubVariantRepo {
        async fn find_by_flag(&self, _flag_id: FlagId) -> Result<Vec<Variant>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _flag_id: FlagId,
            _variant: &Variant,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, variant: &Variant) -> Result<Variant, RepositoryError> {
            Ok(variant.clone())
        }

        async fn delete(&self, _id: VariantId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    struct StubSegmentRepo;

    #[async_trait]
    impl SegmentRepository for StubSegmentRepo {
        async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<Segment, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key.to_string(),
            })
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<Segment>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(&self, _segment: &Segment) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, segment: &Segment) -> Result<Segment, RepositoryError> {
            Ok(segment.clone())
        }

        async fn find_with_rules(
            &self,
            id: SegmentId,
        ) -> Result<RuleBasedSegment, RepositoryError> {
            Ok(RuleBasedSegment {
                id,
                rules: Vec::new(),
            })
        }

        async fn find_with_list(&self, id: SegmentId) -> Result<ListBasedSegment, RepositoryError> {
            Ok(ListBasedSegment {
                id,
                lists: std::collections::HashMap::new(),
            })
        }

        async fn upsert_rules(
            &self,
            _id: SegmentId,
            _rules: &[Rule],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn set_list_entries(
            &self,
            _id: SegmentId,
            _context_type: &str,
            _include: &[String],
            _exclude: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn soft_delete(&self, _id: SegmentId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn check_list_membership(
            &self,
            _environment_id: EnvironmentId,
            _context_type: &str,
            _context_key: &str,
            segment_keys: &[String],
        ) -> Result<std::collections::HashMap<String, bool>, RepositoryError> {
            Ok(segment_keys.iter().map(|k| (k.clone(), false)).collect())
        }

        async fn batch_check_list_membership(
            &self,
            _environment_id: EnvironmentId,
            _contexts: &[(String, String)],
            _segment_keys: &[String],
        ) -> Result<Vec<ContextMembership>, RepositoryError> {
            Ok(Vec::new())
        }
    }

    struct StubSdkKeyRepo {
        active_keys: Vec<SdkKey>,
    }

    impl StubSdkKeyRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                active_keys: Vec::new(),
            })
        }

        fn with_hash(key_hash: String, env_id: EnvironmentId) -> Arc<Self> {
            let key = SdkKey {
                id: SdkKeyId::new(),
                environment_id: env_id,
                key_hash,
                is_active: true,
                created_at: chrono::Utc::now(),
                revoked_at: None,
            };
            Arc::new(Self {
                active_keys: vec![key],
            })
        }
    }

    #[async_trait]
    impl SdkKeyRepository for StubSdkKeyRepo {
        async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(&self, _key: &SdkKey) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_active_by_environment(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(self
                .active_keys
                .iter()
                .filter(|k| k.environment_id == env_id && k.is_active)
                .cloned()
                .collect())
        }

        async fn find_active_by_hash(&self, key_hash: &str) -> Result<SdkKey, RepositoryError> {
            self.active_keys
                .iter()
                .find(|k| k.key_hash == key_hash)
                .cloned()
                .ok_or(RepositoryError::NotFound {
                    id: key_hash.to_string(),
                })
        }
    }

    fn make_state(
        flag_repo: Arc<dyn FlagRepository>,
        sdk_key_repo: Arc<dyn SdkKeyRepository>,
    ) -> crate::AppState {
        struct StubEventDefinitionRepo;
        #[async_trait::async_trait]
        impl stitchd_db::EventDefinitionRepository for StubEventDefinitionRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::EventDefinitionId,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &str,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<Vec<stitchd_core::event::EventDefinition>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::event::EventDefinition,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                d: &stitchd_core::event::EventDefinition,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Ok(d.clone())
            }
            async fn soft_delete(
                &self,
                id: stitchd_core::id::EventDefinitionId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
        }
        struct StubExperimentRepo;
        #[async_trait::async_trait]
        impl stitchd_db::ExperimentRepository for StubExperimentRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::ExperimentId,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
                _: Option<stitchd_core::experimentation::ExperimentStatus>,
            ) -> Result<
                Vec<stitchd_core::experimentation::Experiment>,
                stitchd_db::RepositoryError,
            > {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::experimentation::Experiment,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                e: &stitchd_core::experimentation::Experiment,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Ok(e.clone())
            }
            async fn soft_delete(
                &self,
                id: stitchd_core::id::ExperimentId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_iterations(
                &self,
                _: stitchd_core::id::ExperimentId,
            ) -> Result<
                Vec<stitchd_core::experimentation::ExperimentIteration>,
                stitchd_db::RepositoryError,
            > {
                Ok(vec![])
            }
            async fn apply_transition(
                &self,
                id: stitchd_core::id::ExperimentId,
                _: stitchd_core::experimentation::ExperimentStatus,
                _: Option<stitchd_core::id::UserId>,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
        }
        let db =
            sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool");
        crate::AppState {
            db,
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            segment_repo: Arc::new(StubSegmentRepo),
            flag_repo,
            variant_repo: Arc::new(StubVariantRepo),
            sdk_key_repo,
            event_definition_repo: Arc::new(StubEventDefinitionRepo),
            experiment_repo: Arc::new(StubExperimentRepo),
            event_writer: None,
        }
    }

    // ── FlagSyncServiceImpl integration tests ─────────────────────────────────

    #[tokio::test]
    async fn sync_returns_unauthenticated_when_no_sdk_key() {
        let state = make_state(StubFlagRepo::empty(), StubSdkKeyRepo::empty());
        let svc = FlagSyncServiceImpl::new(state);

        let request = Request::new(SyncRequest { contexts: vec![] });
        let result = svc.sync(request).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn sync_returns_unauthenticated_when_key_not_found() {
        let state = make_state(StubFlagRepo::empty(), StubSdkKeyRepo::empty());
        let svc = FlagSyncServiceImpl::new(state);

        let mut request = Request::new(SyncRequest { contexts: vec![] });
        request
            .metadata_mut()
            .insert("x-sdk-key", "wrong-key".parse().unwrap());
        let result = svc.sync(request).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn sync_returns_ok_for_empty_environment() {
        let env_id = EnvironmentId::new();
        let raw_key = "test-grpc-key";
        let key_hash = crate::api::sdk_auth::hash_sdk_key(raw_key);
        let state = make_state(
            StubFlagRepo::empty(),
            StubSdkKeyRepo::with_hash(key_hash, env_id),
        );
        let svc = FlagSyncServiceImpl::new(state);

        let mut request = Request::new(SyncRequest { contexts: vec![] });
        request
            .metadata_mut()
            .insert("x-sdk-key", raw_key.parse().unwrap());
        let result = svc.sync(request).await;
        assert!(result.is_ok());
        let response = result.unwrap().into_inner();
        assert!(response.flags.is_empty());
        assert!(response.rule_segments.is_empty());
        assert!(response.list_segments.is_empty());
    }

    #[tokio::test]
    async fn flag_sync_service_impl_new_creates_instance() {
        let state = make_state(StubFlagRepo::empty(), StubSdkKeyRepo::empty());
        let svc = FlagSyncServiceImpl::new(state);
        // Just verify the instance is created (tests the const fn new)
        let _ = svc;
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

    // ── Additional domain_variant_to_proto coverage ──────────────────────────

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
    fn flag_rule_percentage_with_parameter_field_adds_param_name() {
        let vid = make_variant_id();
        let mut key_map = HashMap::new();
        key_map.insert(vid, "t".to_string());

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id: stitchd_core::id::FlagId::new(),
            rule_index: 0,
            rule: stitchd_core::rule_engine::types::Rule {
                id: RuleId::new(),
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
    fn flag_rule_variant_output_missing_in_map_returns_empty_key() {
        let vid = make_variant_id();
        let key_map: HashMap<_, String> = HashMap::new(); // no entry for vid

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
            Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(ref k)) if k.is_empty()
        ));
    }
}
