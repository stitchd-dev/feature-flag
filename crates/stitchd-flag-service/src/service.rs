//! gRPC service implementation for [`FlagService`].
//!
//! This module implements the tonic-generated [`FlagService`] trait.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use stitchd_db::{FlagRepository, SdkKeyRepository, VariantRepository};
use stitchd_proto::flags::v1::{
    FeatureFlag, GetFlagDefinitionsRequest, GetFlagRequest, ListFlagsRequest, ListFlagsResponse,
    MutateFlagRequest, MutateFlagResponse, MutationKind, UpdateFlagHashingRequest,
    UpdateFlagHashingResponse, flag_service_server::FlagService,
};

use crate::{error::FlagServiceError, mapping};

/// gRPC implementation of [`FlagService`].
#[allow(clippy::struct_field_names)]
pub struct FlagServiceImpl {
    flag_repo: Arc<dyn FlagRepository>,
    variant_repo: Arc<dyn VariantRepository>,
    sdk_key_repo: Arc<dyn SdkKeyRepository>,
}

impl FlagServiceImpl {
    /// Create a new [`FlagServiceImpl`] backed by the given repositories.
    #[must_use]
    pub fn new(
        flag_repo: Arc<dyn FlagRepository>,
        variant_repo: Arc<dyn VariantRepository>,
        sdk_key_repo: Arc<dyn SdkKeyRepository>,
    ) -> Self {
        Self {
            flag_repo,
            variant_repo,
            sdk_key_repo,
        }
    }

    /// Extract and validate the SDK key from gRPC metadata, returning the environment ID.
    async fn authenticate_sdk(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<stitchd_core::id::EnvironmentId, Status> {
        let raw_key = metadata
            .get("x-sdk-key")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing x-sdk-key metadata"))?;

        let hash = hash_sdk_key(raw_key);

        let sdk_key = self
            .sdk_key_repo
            .find_active_by_hash(&hash)
            .await
            .map_err(|_| Status::unauthenticated("invalid or revoked SDK key"))?;

        Ok(sdk_key.environment_id)
    }
}

/// Parse a string as an [`EnvironmentId`] via UUID.
#[allow(clippy::result_large_err)]
fn parse_env_id(s: &str) -> Result<stitchd_core::id::EnvironmentId, Status> {
    uuid::Uuid::parse_str(s)
        .map(stitchd_core::id::EnvironmentId::from_uuid)
        .map_err(|_| Status::invalid_argument("invalid environment_id"))
}

/// SHA-256 hash of a raw SDK key (same function as used in stitchd-server).
pub(crate) fn hash_sdk_key(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

#[allow(clippy::too_many_lines)]
#[tonic::async_trait]
impl FlagService for FlagServiceImpl {
    type GetFlagDefinitionsStream = ReceiverStream<Result<FeatureFlag, Status>>;

    async fn get_flag(
        &self,
        request: Request<GetFlagRequest>,
    ) -> Result<Response<FeatureFlag>, Status> {
        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id)?;

        let flag_key = stitchd_core::id::FlagKey::new(req.flag_key.clone())
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        // We need project_id for find_by_key; work around by listing by environment and filtering
        let flags = self
            .flag_repo
            .list_by_environment(env_id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let record = flags
            .into_iter()
            .find(|f| f.key.as_str() == flag_key.as_str())
            .ok_or_else(|| Status::not_found(format!("flag '{}' not found", req.flag_key)))?;

        let variants = self
            .variant_repo
            .find_by_flag(record.id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let rules = self
            .flag_repo
            .find_rules(record.id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let proto_flag = mapping::build_feature_flag_proto(&record, variants, &rules);
        Ok(Response::new(proto_flag))
    }

    async fn list_flags(
        &self,
        request: Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id)?;

        let flag_records = self
            .flag_repo
            .list_by_environment(env_id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let mut proto_flags = Vec::with_capacity(flag_records.len());
        for record in &flag_records {
            let variants = self
                .variant_repo
                .find_by_flag(record.id)
                .await
                .map_err(FlagServiceError::from)
                .map_err(Status::from)?;

            let rules = self
                .flag_repo
                .find_rules(record.id)
                .await
                .map_err(FlagServiceError::from)
                .map_err(Status::from)?;

            proto_flags.push(mapping::build_feature_flag_proto(record, variants, &rules));
        }

        Ok(Response::new(ListFlagsResponse { flags: proto_flags }))
    }

    async fn mutate_flag(
        &self,
        request: Request<MutateFlagRequest>,
    ) -> Result<Response<MutateFlagResponse>, Status> {
        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id)?;

        let flag_proto = req
            .flag
            .ok_or_else(|| Status::invalid_argument("flag field is required"))?;

        let kind = MutationKind::try_from(req.kind)
            .map_err(|_| Status::invalid_argument("unknown mutation kind"))?;

        match kind {
            MutationKind::Create => {
                let flag_key = stitchd_core::id::FlagKey::new(flag_proto.key.clone())
                    .map_err(|e| Status::invalid_argument(e.to_string()))?;

                let value_type = proto_value_type_to_domain(
                    stitchd_proto::flags::v1::FlagValueType::try_from(flag_proto.value_type)
                        .unwrap_or(stitchd_proto::flags::v1::FlagValueType::Unspecified),
                );

                let record = stitchd_core::flag::FlagRecord {
                    id: stitchd_core::id::FlagId::new(),
                    project_id: stitchd_core::id::ProjectId::from_uuid(uuid::Uuid::nil()), // placeholder — env owns scope
                    key: flag_key,
                    name: flag_proto.name.clone(),
                    description: flag_proto.description.clone(),
                    value_type,
                    enabled: flag_proto.enabled,
                    default_variant_id: None,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    deleted_at: None,
                    version: 1,
                };

                self.flag_repo
                    .create(&record)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                // Persist variants supplied in the creation request.
                let domain_variants: Vec<_> = flag_proto
                    .variants
                    .iter()
                    .filter_map(|v| mapping::proto_variant_to_domain(v.clone()))
                    .collect();
                if !domain_variants.is_empty() {
                    self.variant_repo
                        .replace_all_for_flag(record.id, &domain_variants)
                        .await
                        .map_err(FlagServiceError::from)
                        .map_err(Status::from)?;
                }

                let variants = self
                    .variant_repo
                    .find_by_flag(record.id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                let rules = self
                    .flag_repo
                    .find_rules(record.id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                let proto = mapping::build_feature_flag_proto(&record, variants, &rules);
                #[allow(clippy::cast_sign_loss)]
                let version = record.version as u64;
                Ok(Response::new(MutateFlagResponse {
                    flag: Some(proto),
                    version,
                }))
            }
            MutationKind::Update => {
                let flags = self
                    .flag_repo
                    .list_by_environment(env_id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                let mut record = flags
                    .into_iter()
                    .find(|f| f.key.as_str() == flag_proto.key.as_str())
                    .ok_or_else(|| {
                        Status::not_found(format!("flag '{}' not found", flag_proto.key))
                    })?;

                // Optimistic locking check
                #[allow(clippy::cast_sign_loss)]
                let stored_version = record.version as u64;
                if req.version != 0 && stored_version != req.version {
                    return Err(Status::aborted(format!(
                        "version conflict: expected {}, actual {}",
                        req.version, stored_version
                    )));
                }

                record.enabled = flag_proto.enabled;
                if !flag_proto.name.is_empty() {
                    record.name = flag_proto.name.clone();
                }
                if !flag_proto.description.is_empty() {
                    record.description = flag_proto.description.clone();
                }
                record.version += 1;
                record.updated_at = chrono::Utc::now();

                let updated = self
                    .flag_repo
                    .update(&record)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                // Replace variants if the request includes a non-empty list.
                if !flag_proto.variants.is_empty() {
                    let domain_variants: Vec<_> = flag_proto
                        .variants
                        .iter()
                        .filter_map(|v| mapping::proto_variant_to_domain(v.clone()))
                        .collect();
                    self.variant_repo
                        .replace_all_for_flag(updated.id, &domain_variants)
                        .await
                        .map_err(FlagServiceError::from)
                        .map_err(Status::from)?;
                }

                let variants = self
                    .variant_repo
                    .find_by_flag(updated.id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                let rules = self
                    .flag_repo
                    .find_rules(updated.id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                #[allow(clippy::cast_sign_loss)]
                let version = updated.version as u64;
                let proto = mapping::build_feature_flag_proto(&updated, variants, &rules);
                Ok(Response::new(MutateFlagResponse {
                    flag: Some(proto),
                    version,
                }))
            }
            MutationKind::Delete | MutationKind::Archive => {
                let flags = self
                    .flag_repo
                    .list_by_environment(env_id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                let record = flags
                    .into_iter()
                    .find(|f| f.key.as_str() == flag_proto.key.as_str())
                    .ok_or_else(|| {
                        Status::not_found(format!("flag '{}' not found", flag_proto.key))
                    })?;

                // Optimistic locking check
                #[allow(clippy::cast_sign_loss)]
                let stored_version = record.version as u64;
                if req.version != 0 && stored_version != req.version {
                    return Err(Status::aborted(format!(
                        "version conflict: expected {}, actual {}",
                        req.version, stored_version
                    )));
                }

                self.flag_repo
                    .soft_delete(record.id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;

                let proto = mapping::build_feature_flag_proto(&record, vec![], &[]);
                Ok(Response::new(MutateFlagResponse {
                    flag: Some(proto),
                    version: stored_version,
                }))
            }
            MutationKind::Unspecified => {
                Err(Status::invalid_argument("mutation kind must be specified"))
            }
        }
    }

    async fn get_flag_definitions(
        &self,
        request: Request<GetFlagDefinitionsRequest>,
    ) -> Result<Response<Self::GetFlagDefinitionsStream>, Status> {
        let env_id = self.authenticate_sdk(request.metadata()).await?;

        let flag_records = self
            .flag_repo
            .list_by_environment(env_id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let (tx, rx) = mpsc::channel(32);

        let flag_repo = Arc::clone(&self.flag_repo);
        let variant_repo = Arc::clone(&self.variant_repo);

        tokio::spawn(async move {
            for record in flag_records {
                let variants = match variant_repo.find_by_flag(record.id).await {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                        return;
                    }
                };

                let rules = match flag_repo.find_rules(record.id).await {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                        return;
                    }
                };

                let proto_flag = mapping::build_feature_flag_proto(&record, variants, &rules);
                if tx.send(Ok(proto_flag)).await.is_err() {
                    // Client disconnected
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// Replace the hashing configuration for a flag.
    async fn update_flag_hashing(
        &self,
        request: Request<UpdateFlagHashingRequest>,
    ) -> Result<Response<UpdateFlagHashingResponse>, Status> {
        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id)?;

        let flag_key = stitchd_core::id::FlagKey::new(req.flag_key.clone())
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let flags = self
            .flag_repo
            .list_by_environment(env_id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let record = flags
            .into_iter()
            .find(|f| f.key.as_str() == flag_key.as_str())
            .ok_or_else(|| Status::not_found(format!("flag '{}' not found", req.flag_key)))?;

        let domain_configs: Vec<stitchd_core::flag::FlagHashingConfig> = req
            .configs
            .iter()
            .map(|c| stitchd_core::flag::FlagHashingConfig {
                flag_id: record.id,
                parameter_key: c.parameter_key.clone(),
                parameter_type: c.parameter_type.clone(),
                order: c.order,
            })
            .collect();

        self.flag_repo
            .upsert_hashing_config(record.id, &domain_configs)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let variants = self
            .variant_repo
            .find_by_flag(record.id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let rules = self
            .flag_repo
            .find_rules(record.id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        let proto_flag = mapping::build_feature_flag_proto(&record, variants, &rules);

        let proto_configs = req.configs.clone();
        metrics::counter!("flag_service.update_flag_hashing.ok").increment(1);
        Ok(Response::new(UpdateFlagHashingResponse {
            flag: Some(proto_flag),
            configs: proto_configs,
        }))
    }
}

/// Convert a proto `FlagValueType` to the domain type.
const fn proto_value_type_to_domain(
    vt: stitchd_proto::flags::v1::FlagValueType,
) -> stitchd_core::flag::FlagValueType {
    use stitchd_core::flag::FlagValueType as D;
    use stitchd_proto::flags::v1::FlagValueType as P;
    match vt {
        P::Int => D::Int,
        P::Double => D::Double,
        P::String => D::Str,
        P::Json => D::Json,
        P::Bool | P::Unspecified => D::Bool, // default fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use stitchd_core::{
        flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType},
        id::{EnvironmentId, FlagId, FlagKey, ProjectId, SdkKeyId, VariantId},
        tenant::SdkKey,
    };
    use stitchd_db::{FlagRepository, RepositoryError, SdkKeyRepository, VariantRepository};
    use tokio_stream::StreamExt as _;

    // ── Stub repositories ──────────────────────────────────────────────────────

    #[derive(Default)]
    struct StubFlagRepo {
        flags: Mutex<Vec<FlagRecord>>,
        rules: Mutex<std::collections::HashMap<FlagId, Vec<FlagRule>>>,
    }

    impl StubFlagRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn with_flags(flags: Vec<FlagRecord>) -> Arc<Self> {
            Arc::new(Self {
                flags: Mutex::new(flags),
                rules: Mutex::new(std::collections::HashMap::new()),
            })
        }
    }

    #[async_trait]
    impl FlagRepository for StubFlagRepo {
        async fn find_by_id(&self, id: FlagId) -> Result<FlagRecord, RepositoryError> {
            self.flags
                .lock()
                .unwrap()
                .iter()
                .find(|f| f.id == id)
                .cloned()
                .ok_or(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &FlagKey,
            _project_id: ProjectId,
        ) -> Result<FlagRecord, RepositoryError> {
            self.flags
                .lock()
                .unwrap()
                .iter()
                .find(|f| f.key.as_str() == key.as_str())
                .cloned()
                .ok_or(RepositoryError::NotFound {
                    id: key.to_string(),
                })
        }

        async fn list_by_project(
            &self,
            _project_id: ProjectId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            Ok(self.flags.lock().unwrap().clone())
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            Ok(self.flags.lock().unwrap().clone())
        }

        async fn create(&self, flag: &FlagRecord) -> Result<(), RepositoryError> {
            self.flags.lock().unwrap().push(flag.clone());
            Ok(())
        }

        async fn update(&self, flag: &FlagRecord) -> Result<FlagRecord, RepositoryError> {
            let mut flags = self.flags.lock().unwrap();
            for f in flags.iter_mut() {
                if f.id == flag.id {
                    *f = flag.clone();
                    return Ok(flag.clone());
                }
            }
            Err(RepositoryError::NotFound {
                id: flag.id.to_string(),
            })
        }

        async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
            let flags = self.flags.lock().unwrap();
            if flags.iter().any(|f| f.id == id) {
                Ok(())
            } else {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
        }

        async fn find_hashing_config(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<FlagHashingConfig>, RepositoryError> {
            Ok(vec![])
        }

        async fn upsert_hashing_config(
            &self,
            _flag_id: FlagId,
            _config: &[FlagHashingConfig],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn find_rules(&self, flag_id: FlagId) -> Result<Vec<FlagRule>, RepositoryError> {
            Ok(self
                .rules
                .lock()
                .unwrap()
                .get(&flag_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn upsert_rules(
            &self,
            flag_id: FlagId,
            rules: &[FlagRule],
        ) -> Result<(), RepositoryError> {
            self.rules.lock().unwrap().insert(flag_id, rules.to_vec());
            Ok(())
        }
    }

    #[derive(Default)]
    struct StubVariantRepo;

    #[async_trait]
    impl VariantRepository for StubVariantRepo {
        async fn find_by_flag(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::Variant>, RepositoryError> {
            Ok(vec![])
        }

        async fn create(
            &self,
            _flag_id: FlagId,
            _variant: &stitchd_core::flag::Variant,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(
            &self,
            variant: &stitchd_core::flag::Variant,
        ) -> Result<stitchd_core::flag::Variant, RepositoryError> {
            Ok(variant.clone())
        }

        async fn delete(&self, _id: VariantId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn replace_all_for_flag(
            &self,
            _flag_id: FlagId,
            _variants: &[stitchd_core::flag::Variant],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    struct StubSdkKeyRepo {
        active_keys: Vec<SdkKey>,
    }

    impl StubSdkKeyRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                active_keys: vec![],
            })
        }

        fn with_hash(key_hash: String, env_id: EnvironmentId) -> Arc<Self> {
            Arc::new(Self {
                active_keys: vec![SdkKey {
                    id: SdkKeyId::new(),
                    environment_id: env_id,
                    key_hash,
                    is_active: true,
                    created_at: chrono::Utc::now(),
                    revoked_at: None,
                }],
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
            Ok(vec![])
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
                .filter(|k| k.environment_id == env_id)
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

    fn make_flag_record() -> FlagRecord {
        FlagRecord {
            id: FlagId::new(),
            project_id: ProjectId::new(),
            key: FlagKey::new("test-flag").unwrap(),
            name: String::new(),
            description: String::new(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    fn make_service_empty() -> FlagServiceImpl {
        FlagServiceImpl::new(
            StubFlagRepo::empty(),
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        )
    }

    // ── Task 2: GetFlagDefinitions failing tests ─────────────────────────────

    #[tokio::test]
    async fn get_flag_definitions_requires_sdk_key() {
        let svc = make_service_empty();
        let req = Request::new(GetFlagDefinitionsRequest {
            environment_id: EnvironmentId::new().to_string(),
        });
        // No x-sdk-key in metadata → should return Unauthenticated
        let result = svc.get_flag_definitions(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_flag_definitions_rejects_invalid_sdk_key() {
        let svc = make_service_empty();
        let mut req = Request::new(GetFlagDefinitionsRequest {
            environment_id: EnvironmentId::new().to_string(),
        });
        req.metadata_mut()
            .insert("x-sdk-key", "bad-key".parse().unwrap());
        let result = svc.get_flag_definitions(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_flag_definitions_returns_empty_stream_for_empty_environment() {
        let env_id = EnvironmentId::new();
        let raw_key = "test-sdk-key";
        let key_hash = hash_sdk_key(raw_key);
        let svc = FlagServiceImpl::new(
            StubFlagRepo::empty(),
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::with_hash(key_hash, env_id),
        );

        let mut req = Request::new(GetFlagDefinitionsRequest {
            environment_id: env_id.to_string(),
        });
        req.metadata_mut()
            .insert("x-sdk-key", raw_key.parse().unwrap());

        let result = svc.get_flag_definitions(req).await;
        assert!(result.is_ok());

        let stream = result.unwrap().into_inner();
        let items: Vec<_> = stream.collect().await;
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn get_flag_definitions_streams_all_flags() {
        let env_id = EnvironmentId::new();
        let raw_key = "test-sdk-key-2";
        let key_hash = hash_sdk_key(raw_key);

        let flag1 = make_flag_record();
        let flag2 = FlagRecord {
            id: FlagId::new(),
            key: FlagKey::new("feature-b").unwrap(),
            ..make_flag_record()
        };
        let flag_repo = StubFlagRepo::with_flags(vec![flag1, flag2]);

        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::with_hash(key_hash, env_id),
        );

        let mut req = Request::new(GetFlagDefinitionsRequest {
            environment_id: env_id.to_string(),
        });
        req.metadata_mut()
            .insert("x-sdk-key", raw_key.parse().unwrap());

        let result = svc.get_flag_definitions(req).await.unwrap();
        let mut stream = result.into_inner();

        let mut count = 0;
        while let Some(item) = stream.next().await {
            assert!(item.is_ok(), "stream item should be Ok");
            count += 1;
        }
        assert_eq!(count, 2);
    }

    // ── Task 3 Tests: Flag CRUD (GetFlag, ListFlags) ─────────────────────────

    #[tokio::test]
    async fn get_flag_returns_not_found_for_missing_key() {
        let svc = make_service_empty();
        let req = Request::new(GetFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            flag_key: "nonexistent-flag".to_string(),
        });
        let result = svc.get_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn get_flag_returns_flag_when_found() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        );

        let req = Request::new(GetFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            flag_key: flag_key.clone(),
        });
        let result = svc.get_flag(req).await;
        assert!(result.is_ok());
        let flag_proto = result.unwrap().into_inner();
        assert_eq!(flag_proto.key, flag_key);
    }

    #[tokio::test]
    async fn get_flag_returns_invalid_argument_for_bad_env_id() {
        let svc = make_service_empty();
        let req = Request::new(GetFlagRequest {
            environment_id: "not-a-uuid".to_string(),
            flag_key: "my-flag".to_string(),
        });
        let result = svc.get_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn list_flags_returns_empty_for_empty_environment() {
        let svc = make_service_empty();
        let req = Request::new(ListFlagsRequest {
            environment_id: EnvironmentId::new().to_string(),
        });
        let result = svc.list_flags(req).await;
        assert!(result.is_ok());
        assert!(result.unwrap().into_inner().flags.is_empty());
    }

    #[tokio::test]
    async fn list_flags_returns_all_flags() {
        let flag1 = make_flag_record();
        let flag2 = FlagRecord {
            id: FlagId::new(),
            key: FlagKey::new("feature-b").unwrap(),
            ..make_flag_record()
        };
        let flag_repo = StubFlagRepo::with_flags(vec![flag1, flag2]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        );

        let req = Request::new(ListFlagsRequest {
            environment_id: EnvironmentId::new().to_string(),
        });
        let result = svc.list_flags(req).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_inner().flags.len(), 2);
    }

    #[tokio::test]
    async fn list_flags_returns_invalid_argument_for_bad_env_id() {
        let svc = make_service_empty();
        let req = Request::new(ListFlagsRequest {
            environment_id: "bad-uuid".to_string(),
        });
        let result = svc.list_flags(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // ── Task 4/5 Tests: MutateFlag ─────────────────────────────────────────────

    #[tokio::test]
    async fn mutate_flag_create_succeeds() {
        let svc = make_service_empty();
        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Create as i32,
            flag: Some(FeatureFlag {
                key: "new-flag".to_string(),
                name: String::new(),
                description: String::new(),
                enabled: true,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 0,
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_ok());
        let resp = result.unwrap().into_inner();
        assert!(resp.flag.is_some());
        assert_eq!(resp.flag.unwrap().key, "new-flag");
    }

    #[tokio::test]
    async fn mutate_flag_create_fails_with_empty_key() {
        let svc = make_service_empty();
        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Create as i32,
            flag: Some(FeatureFlag {
                key: "".to_string(),
                name: String::new(),
                description: String::new(),
                enabled: true,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 0,
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn mutate_flag_update_succeeds_with_correct_version() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key.clone(),
                name: String::new(),
                description: String::new(),
                enabled: false,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 1, // matches initial version
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_ok());
        let resp = result.unwrap().into_inner();
        assert!(resp.flag.is_some());
        assert!(!resp.flag.unwrap().enabled);
    }

    #[tokio::test]
    async fn mutate_flag_update_rejects_version_mismatch() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                name: String::new(),
                description: String::new(),
                enabled: false,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 99, // wrong version
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Aborted);
    }

    #[tokio::test]
    async fn mutate_flag_delete_succeeds_with_correct_version() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Delete as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                name: String::new(),
                description: String::new(),
                enabled: true,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 1,
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mutate_flag_delete_rejects_version_mismatch() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Delete as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                name: String::new(),
                description: String::new(),
                enabled: true,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 42, // wrong
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Aborted);
    }

    #[tokio::test]
    async fn mutate_flag_archive_succeeds_with_correct_version() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Archive as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                name: String::new(),
                description: String::new(),
                enabled: true,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 1,
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mutate_flag_unspecified_kind_returns_invalid_argument() {
        let svc = make_service_empty();
        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Unspecified as i32,
            flag: Some(FeatureFlag {
                key: "some-flag".to_string(),
                name: String::new(),
                description: String::new(),
                enabled: true,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 0,
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn mutate_flag_update_returns_not_found_for_missing_flag() {
        let svc = make_service_empty();
        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: "nonexistent".to_string(),
                name: String::new(),
                description: String::new(),
                enabled: false,
                value_type: stitchd_proto::flags::v1::FlagValueType::Bool as i32,
                variants: vec![],
                rules: vec![],
                ..Default::default()
            }),
            version: 1,
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn mutate_flag_missing_flag_field_returns_invalid_argument() {
        let svc = make_service_empty();
        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            kind: MutationKind::Create as i32,
            flag: None,
            version: 0,
        });
        let result = svc.mutate_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn hash_sdk_key_is_deterministic() {
        let h1 = hash_sdk_key("my-key");
        let h2 = hash_sdk_key("my-key");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_sdk_key_different_inputs_differ() {
        let h1 = hash_sdk_key("key-a");
        let h2 = hash_sdk_key("key-b");
        assert_ne!(h1, h2);
    }
}
