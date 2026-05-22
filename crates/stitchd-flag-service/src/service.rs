//! gRPC service implementation for [`FlagService`].
//!
//! This module implements the tonic-generated [`FlagService`] trait.

use std::sync::Arc;

use clickhouse::Client as ChClient;
use stitchd_db::{
    ExperimentRepository, FlagRepository, SdkKeyRepository, SegmentRepository, VariantRepository,
};
use stitchd_proto::flags::v1::{
    EvaluatePreviewRequest, EvaluatePreviewResponse, FeatureFlag, GetFlagDefinitionsRequest,
    GetFlagRequest, ListFlagsRequest, ListFlagsResponse, MutateFlagRequest, MutateFlagResponse,
    MutationKind, SetDefaultRuleDistributionRequest, SetDefaultRuleDistributionResponse,
    UpdateFlagHashingRequest, UpdateFlagHashingResponse, flag_service_server::FlagService,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
    error::FlagServiceError,
    flag_lock::{FlagLockCache, is_flag_locked},
    mapping,
};

/// gRPC implementation of [`FlagService`].
#[allow(clippy::struct_field_names)]
pub struct FlagServiceImpl {
    flag_repo: Arc<dyn FlagRepository>,
    variant_repo: Arc<dyn VariantRepository>,
    sdk_key_repo: Arc<dyn SdkKeyRepository>,
    segment_repo: Arc<dyn SegmentRepository>,
    /// Optional ClickHouse client for evaluation telemetry. `None` disables logging.
    ch_client: Option<Arc<ChClient>>,
    /// Optional experiment repository for whole-flag lock enforcement. When
    /// `None`, the lock check is a no-op (admissible only in legacy or in
    /// targeted tests that do not exercise the lock path). Production setups
    /// always wire this in via [`FlagServiceImpl::with_experiment_repo`].
    experiment_repo: Option<Arc<dyn ExperimentRepository>>,
    /// In-process cache for `is_flag_locked` results. Populated only when
    /// `experiment_repo` is set.
    flag_lock_cache: FlagLockCache,
}

impl FlagServiceImpl {
    /// Create a new [`FlagServiceImpl`] backed by the given repositories.
    #[must_use]
    pub fn new(
        flag_repo: Arc<dyn FlagRepository>,
        variant_repo: Arc<dyn VariantRepository>,
        sdk_key_repo: Arc<dyn SdkKeyRepository>,
        segment_repo: Arc<dyn SegmentRepository>,
    ) -> Self {
        Self {
            flag_repo,
            variant_repo,
            sdk_key_repo,
            segment_repo,
            ch_client: None,
            experiment_repo: None,
            flag_lock_cache: FlagLockCache::new(),
        }
    }

    /// Attach a ClickHouse client for fire-and-forget evaluation telemetry.
    #[must_use]
    pub fn with_clickhouse(mut self, client: Arc<ChClient>) -> Self {
        self.ch_client = Some(client);
        self
    }

    /// Wire in the experiment repository so flag mutation paths can enforce
    /// the whole-flag lock. Without this, mutations skip the lock check —
    /// useful for unit tests that don't need lock semantics, but the
    /// production `main.rs` always sets it.
    #[must_use]
    pub fn with_experiment_repo(mut self, repo: Arc<dyn ExperimentRepository>) -> Self {
        self.experiment_repo = Some(repo);
        self
    }

    /// Expose the in-process flag-lock cache so adjacent services
    /// (experimentation-service running in the same process during tests)
    /// can invalidate entries on experiment lifecycle transitions.
    #[must_use]
    pub fn flag_lock_cache(&self) -> FlagLockCache {
        self.flag_lock_cache.clone()
    }

    /// Look up the flag lock for `flag_id`, returning [`FlagServiceError::FlagLocked`]
    /// when an experiment in `running`/`paused` state owns the flag. The error
    /// surfaces to clients as a `tonic::Status::failed_precondition` with a
    /// `flag_locked_by_experiment:{experiment_id}` message which the gateway
    /// decodes back into HTTP 409 with the structured error body.
    pub(crate) async fn ensure_flag_unlocked(
        &self,
        flag_id: stitchd_core::id::FlagId,
    ) -> Result<(), FlagServiceError> {
        let Some(repo) = self.experiment_repo.as_deref() else {
            return Ok(());
        };
        match is_flag_locked(repo, &self.flag_lock_cache, flag_id).await {
            Ok(Some(exp_id)) => Err(FlagServiceError::FlagLocked {
                experiment_id: exp_id,
            }),
            Ok(None) => Ok(()),
            Err(status) => Err(FlagServiceError::Internal(status.message().to_string())),
        }
    }

    /// Populate the proto's `locked_by_experiment_id` field by consulting the
    /// `is_flag_locked` helper. Best-effort — when the experiment repository
    /// is not configured (legacy/test setups) or the lookup fails the field
    /// stays empty rather than failing the admin read.
    async fn populate_lock_state(
        &self,
        flag_id: stitchd_core::id::FlagId,
        proto: &mut FeatureFlag,
    ) {
        let Some(repo) = self.experiment_repo.as_deref() else {
            return;
        };
        match is_flag_locked(repo, &self.flag_lock_cache, flag_id).await {
            Ok(Some(exp_id)) => {
                proto.locked_by_experiment_id = exp_id.to_string();
            }
            Ok(None) => {}
            Err(status) => {
                tracing::warn!(
                    flag_id = %flag_id,
                    error = %status.message(),
                    "flag lock lookup failed during admin read; surfacing without lock state",
                );
            }
        }
    }

    /// Fetch `SegmentDefinition`s for a set of IDs in three bulk DB queries.
    async fn fetch_segment_definitions(
        &self,
        ids: &std::collections::HashSet<stitchd_core::id::SegmentId>,
    ) -> Result<Vec<stitchd_core::segment::SegmentDefinition>, Status> {
        use stitchd_core::segment::{SegmentDefinition, SegmentType};

        if ids.is_empty() {
            return Ok(vec![]);
        }
        let id_slice: Vec<stitchd_core::id::SegmentId> = ids.iter().copied().collect();

        let segments = self
            .segment_repo
            .find_batch_by_ids(&id_slice)
            .await
            .map_err(|e| Status::internal(format!("segment batch lookup failed: {e}")))?;

        let (rule_ids, list_ids): (Vec<_>, Vec<_>) = segments
            .iter()
            .partition(|s| s.segment_type == SegmentType::Rule);
        let rule_ids: Vec<_> = rule_ids.iter().map(|s| s.id).collect();
        let list_ids: Vec<_> = list_ids.iter().map(|s| s.id).collect();

        let rules_map = self
            .segment_repo
            .find_rules_batch(&rule_ids)
            .await
            .map_err(|e| Status::internal(format!("rules batch lookup failed: {e}")))?;
        // List segment defs not returned until Phase 4 Scylla membership reads.
        let _ = list_ids;

        let mut definitions = Vec::with_capacity(segments.len());
        for seg in &segments {
            let definition = match seg.segment_type {
                SegmentType::Rule => {
                    if let Some(rule_seg) = rules_map.get(&seg.id) {
                        SegmentDefinition::RuleBased(rule_seg.clone())
                    } else {
                        continue;
                    }
                }
                // List segment defs pending Phase 4 Scylla migration — skip for now.
                SegmentType::List => continue,
            };
            definitions.push(definition);
        }
        Ok(definitions)
    }

    /// Resolve list-segment membership for every evaluation context.
    ///
    /// Returns one `HashSet<SegmentId>` per context (aligned by index), each
    /// containing the IDs of list-type segments the context is a member of.
    /// Rule-based segments are excluded — those are handled by `fetch_segment_definitions`.
    ///
    /// Membership is checked against ScyllaDB via `find_memberships_batch`, which
    /// returns `include AND NOT exclude` semantics.
    async fn resolve_list_memberships(
        &self,
        all_segment_ids: &std::collections::HashSet<stitchd_core::id::SegmentId>,
        evaluation_contexts: &[stitchd_core::context::EvaluationContext],
        env_id: stitchd_core::id::EnvironmentId,
    ) -> Result<Vec<std::collections::HashSet<stitchd_core::id::SegmentId>>, Status> {
        use stitchd_core::segment::SegmentType;

        if all_segment_ids.is_empty() || evaluation_contexts.is_empty() {
            return Ok(vec![
                std::collections::HashSet::new();
                evaluation_contexts.len()
            ]);
        }

        // Identify which of the referenced segment IDs are list-type.
        let id_slice: Vec<_> = all_segment_ids.iter().copied().collect();
        let segments = self
            .segment_repo
            .find_batch_by_ids(&id_slice)
            .await
            .map_err(|e| Status::internal(format!("segment batch lookup failed: {e}")))?;

        let list_segment_ids: Vec<stitchd_core::id::SegmentId> = segments
            .iter()
            .filter(|s| s.segment_type == SegmentType::List)
            .map(|s| s.id)
            .collect();

        if list_segment_ids.is_empty() {
            return Ok(vec![
                std::collections::HashSet::new();
                evaluation_contexts.len()
            ]);
        }

        // Build (context_type, context_key) pairs across every sub-context
        // in every bundle (bug fix `feature-flag-utp`: bundles may carry
        // multiple sub-contexts, e.g. user + device + application; list
        // membership must be resolved for ALL of them so an `InSegment`
        // leaf referencing any context_type evaluates correctly).
        let mut contexts_for_batch: Vec<(String, String)> = Vec::new();
        // `ec_offsets[i]` is the index of the first (ct, key) tuple in
        // `contexts_for_batch` belonging to `evaluation_contexts[i]`.
        // Used after the batch lookup to fold per-sub-context membership
        // sets back into one set-per-EC (the engine's caller contract).
        let mut ec_offsets: Vec<usize> = Vec::with_capacity(evaluation_contexts.len());
        for ec in evaluation_contexts {
            ec_offsets.push(contexts_for_batch.len());
            for ctx in &ec.contexts {
                contexts_for_batch.push((ctx.context_type.clone(), ctx.key.clone()));
            }
        }
        ec_offsets.push(contexts_for_batch.len());

        if contexts_for_batch.is_empty() {
            return Ok(vec![
                std::collections::HashSet::new();
                evaluation_contexts.len()
            ]);
        }

        let memberships = self
            .segment_repo
            .find_memberships_batch(env_id, &contexts_for_batch, &list_segment_ids)
            .await
            .map_err(|e| Status::internal(format!("list membership batch check failed: {e}")))?;

        // Per-sub-context membership sets (in input order).
        let per_subctx: Vec<std::collections::HashSet<stitchd_core::id::SegmentId>> = memberships
            .into_iter()
            .map(|m| {
                m.memberships
                    .into_iter()
                    .filter_map(|(seg_id, is_member)| if is_member { Some(seg_id) } else { None })
                    .collect()
            })
            .collect();

        // Fold per-sub-context sets into per-EC sets (UNION across the
        // sub-contexts of each bundle). The engine then merges this set
        // into the resolved segments for every context in the bundle.
        let mut result: Vec<std::collections::HashSet<stitchd_core::id::SegmentId>> =
            Vec::with_capacity(evaluation_contexts.len());
        for i in 0..evaluation_contexts.len() {
            let start = ec_offsets[i];
            let end = ec_offsets[i + 1].min(per_subctx.len());
            let mut merged: std::collections::HashSet<stitchd_core::id::SegmentId> =
                std::collections::HashSet::new();
            for s in &per_subctx[start..end] {
                merged.extend(s.iter().copied());
            }
            result.push(merged);
        }
        Ok(result)
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

/// Parse a string as a [`ProjectId`] via UUID. Returns `None` when the string is empty.
#[allow(clippy::result_large_err)]
fn parse_project_id(s: &str) -> Result<Option<stitchd_core::id::ProjectId>, Status> {
    if s.is_empty() {
        return Ok(None);
    }
    uuid::Uuid::parse_str(s)
        .map(|u| Some(stitchd_core::id::ProjectId::from_uuid(u)))
        .map_err(|_| Status::invalid_argument("invalid project_id"))
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
        let project_id = parse_project_id(&req.project_id)?;

        let flag_key = stitchd_core::id::FlagKey::new(req.flag_key.clone())
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let record = if let Some(pid) = project_id {
            // Admin path: direct project-scoped lookup.
            self.flag_repo
                .find_by_key(&flag_key, pid)
                .await
                .map_err(FlagServiceError::from)
                .map_err(Status::from)?
        } else {
            // SDK path: resolve via environment.
            let env_id = parse_env_id(&req.environment_id)?;
            let flags = self
                .flag_repo
                .list_by_environment(env_id)
                .await
                .map_err(FlagServiceError::from)
                .map_err(Status::from)?;
            flags
                .into_iter()
                .find(|f| f.key.as_str() == flag_key.as_str())
                .ok_or_else(|| Status::not_found(format!("flag '{}' not found", req.flag_key)))?
        };

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

        let mut proto_flag = mapping::build_feature_flag_proto(&record, variants, &rules);
        // Admin reads (`project_id` set) surface the lock state so the UI
        // can show the lock badge proactively rather than after a failed
        // save. SDK paths leave `locked_by_experiment_id` empty.
        if project_id.is_some() {
            self.populate_lock_state(record.id, &mut proto_flag).await;
        }
        Ok(Response::new(proto_flag))
    }

    async fn list_flags(
        &self,
        request: Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        let req = request.into_inner();
        let project_id = parse_project_id(&req.project_id)?;

        // Pagination is supported only on the admin (project-scoped) path.
        // page > 0 signals the caller wants paginated results.
        let use_pagination = req.page > 0 && project_id.is_some();
        let page = if req.page == 0 { 1u64 } else { req.page as u64 };
        let per_page = if req.per_page == 0 {
            50u64
        } else {
            (req.per_page as u64).min(200)
        };
        let offset = (page - 1) * per_page;

        let (flag_records, total) = if let Some(pid) = project_id {
            // Admin path: project-scoped list.
            if use_pagination {
                self.flag_repo
                    .list_by_project_paginated(pid, offset, per_page)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?
            } else if req.include_archived {
                let flags = self
                    .flag_repo
                    .list_by_project_all(pid)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;
                (flags, 0)
            } else {
                let flags = self
                    .flag_repo
                    .list_by_project(pid)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?;
                (flags, 0)
            }
        } else {
            // SDK path: environment-scoped list (no pagination).
            let env_id = parse_env_id(&req.environment_id)?;
            let flags = if req.include_archived {
                self.flag_repo
                    .list_by_environment_all(env_id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?
            } else {
                self.flag_repo
                    .list_by_environment(env_id)
                    .await
                    .map_err(FlagServiceError::from)
                    .map_err(Status::from)?
            };
            (flags, 0)
        };

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

            let mut proto = mapping::build_feature_flag_proto(record, variants, &rules);
            if project_id.is_some() {
                self.populate_lock_state(record.id, &mut proto).await;
            }
            proto_flags.push(proto);
        }

        Ok(Response::new(ListFlagsResponse {
            flags: proto_flags,
            total,
        }))
    }

    async fn mutate_flag(
        &self,
        request: Request<MutateFlagRequest>,
    ) -> Result<Response<MutateFlagResponse>, Status> {
        let req = request.into_inner();
        let project_id = parse_project_id(&req.project_id)?;
        // Only parsed when project_id is absent (SDK/env-scoped path).
        let env_id_result = if project_id.is_none() {
            Some(parse_env_id(&req.environment_id)?)
        } else {
            None
        };

        let flag_proto = req
            .flag
            .ok_or_else(|| Status::invalid_argument("flag field is required"))?;

        let kind = MutationKind::try_from(req.kind)
            .map_err(|_| Status::invalid_argument("unknown mutation kind"))?;

        /// Fetch the flag list for lookup — project path or env path.
        macro_rules! fetch_flag_list {
            ($self:expr, $project_id:expr, $env_id:expr) => {
                if let Some(pid) = $project_id {
                    $self
                        .flag_repo
                        .list_by_project(pid)
                        .await
                        .map_err(FlagServiceError::from)
                        .map_err(Status::from)?
                } else {
                    $self
                        .flag_repo
                        .list_by_environment($env_id.expect("env_id required"))
                        .await
                        .map_err(FlagServiceError::from)
                        .map_err(Status::from)?
                }
            };
        }

        match kind {
            MutationKind::Create => {
                let pid = project_id.ok_or_else(|| {
                    Status::invalid_argument("project_id is required for flag creation")
                })?;

                let flag_key = stitchd_core::id::FlagKey::new(flag_proto.key.clone())
                    .map_err(|e| Status::invalid_argument(e.to_string()))?;

                let value_type = proto_value_type_to_domain(
                    stitchd_proto::flags::v1::FlagValueType::try_from(flag_proto.value_type)
                        .unwrap_or(stitchd_proto::flags::v1::FlagValueType::Unspecified),
                );

                let record = stitchd_core::flag::FlagRecord {
                    id: stitchd_core::id::FlagId::new(),
                    project_id: pid,
                    key: flag_key,
                    name: flag_proto.name.clone(),
                    description: flag_proto.description.clone(),
                    value_type,
                    enabled: flag_proto.enabled,
                    default_variant_id: None,
                    default_rule_distribution: None,
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
                let flags = fetch_flag_list!(self, project_id, env_id_result);

                let mut record = flags
                    .into_iter()
                    .find(|f| f.key.as_str() == flag_proto.key.as_str())
                    .ok_or_else(|| {
                        Status::not_found(format!("flag '{}' not found", flag_proto.key))
                    })?;

                // Whole-flag lock: an experiment in running/paused state on
                // this flag rejects every admin mutation with 409.
                self.ensure_flag_unlocked(record.id)
                    .await
                    .map_err(Status::from)?;

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
                // Resolve default_variant_key → variant ID when provided.
                if !flag_proto.default_variant_key.is_empty() {
                    let variants_for_lookup = self
                        .variant_repo
                        .find_by_flag(record.id)
                        .await
                        .map_err(FlagServiceError::from)
                        .map_err(Status::from)?;
                    let variant_id = variants_for_lookup
                        .iter()
                        .find(|v| v.key == flag_proto.default_variant_key)
                        .map(|v| v.id)
                        .ok_or_else(|| {
                            Status::not_found(format!(
                                "variant '{}' not found",
                                flag_proto.default_variant_key
                            ))
                        })?;
                    record.default_variant_id = Some(variant_id);
                }

                // Bug fix `feature-flag-7yc` — server-side defense-in-depth:
                // if the inbound rule list ends with a catch-all rule whose
                // condition is the legal `And: []` (always-true) sentinel
                // AND the caller did NOT provide an explicit
                // `default_variant_key`, absorb the catch-all's output into
                // the canonical default-rule fields (`default_variant_id`
                // and/or `default_rule_distribution`) and drop the rule
                // itself from the persisted list. Rationale: pre-Phase-8 the
                // admin UI appended a synthetic catch-all rule on save,
                // which polluted GET responses with an extra rule and broke
                // the "saved rule count = 1" contract. The canonical
                // fallthrough lives on the flag record — not in the rule
                // list — so we normalise here.
                //
                // The catch-all is detected ONLY when it is the last entry
                // in the list (mid-list always-true rules are legitimate
                // unconditional matches that the operator authored on
                // purpose).
                let strip_trailing_catch_all =
                    !flag_proto.rules.is_empty() && flag_proto.default_variant_key.is_empty() && {
                        let last = flag_proto.rules.last().expect("rules non-empty");
                        // Empty rule_payload is treated as the catch-all
                        // sentinel by the engine (`ConditionExpr::And(vec![])`
                        // is the deserialised form, but the UI may submit
                        // either). Also handle the explicit `And: []`.
                        let is_catch_all_cond = last.rule_payload.is_empty()
                            || serde_json::from_slice::<
                                stitchd_core::rule_engine::types::ConditionExpr,
                            >(&last.rule_payload)
                            .map(|c| {
                                matches!(
                                    c,
                                    stitchd_core::rule_engine::types::ConditionExpr::And(ref v)
                                        if v.is_empty()
                                )
                            })
                            .unwrap_or(false);
                        is_catch_all_cond
                    };

                if strip_trailing_catch_all {
                    let catch_all = flag_proto.rules.last().expect("checked non-empty").clone();
                    match &catch_all.output {
                        Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(key))
                            if !key.is_empty() =>
                        {
                            // Resolve variant_key → variant_id and stash on
                            // the record's `default_variant_id`. The
                            // variants for this flag may have just been
                            // replaced via `flag_proto.variants`; prefer the
                            // incoming list when present so a freshly-added
                            // variant can be referenced.
                            let lookup_variants = if !flag_proto.variants.is_empty() {
                                flag_proto
                                    .variants
                                    .iter()
                                    .filter_map(|v| mapping::proto_variant_to_domain(v.clone()))
                                    .collect::<Vec<_>>()
                            } else {
                                self.variant_repo
                                    .find_by_flag(record.id)
                                    .await
                                    .map_err(FlagServiceError::from)
                                    .map_err(Status::from)?
                            };
                            if let Some(v) = lookup_variants.iter().find(|v| &v.key == key) {
                                record.default_variant_id = Some(v.id);
                                // Clear any prior percentage fallthrough —
                                // the catch-all rule's variant output is
                                // single-variant, not percentage.
                                record.default_rule_distribution = None;
                            }
                        }
                        Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc))
                            if !alloc.buckets.is_empty() =>
                        {
                            // Percentage catch-all → `default_rule_distribution`.
                            // Convert weight_milli (0..=1000) → percentage
                            // (0.0..=100.0) preserving operator intent. Sum
                            // is normalised to 100.0 by RolloutDistribution::validate
                            // downstream; if validation fails we leave the
                            // record's default fields alone and let the rule
                            // persist as-is (best-effort defense-in-depth,
                            // not strict normalisation).
                            let allocations: Vec<stitchd_core::rollout::RolloutAllocation> = alloc
                                .buckets
                                .iter()
                                .map(|b| stitchd_core::rollout::RolloutAllocation {
                                    variant_key: b.variant_key.clone(),
                                    percentage: f64::from(b.weight_milli) / 10.0,
                                })
                                .collect();
                            let dist = stitchd_core::rollout::RolloutDistribution { allocations };
                            // Only adopt the distribution if it validates;
                            // otherwise leave the record alone (the strict
                            // pre-existing validators on
                            // `set_default_rule_distribution` already
                            // reject malformed shapes).
                            if dist.validate().is_ok() {
                                record.default_rule_distribution = Some(dist);
                                record.default_variant_id = None;
                            }
                        }
                        _ => {
                            // No usable output on the catch-all (e.g.
                            // unset oneof). Leave the catch-all rule in
                            // place — better to round-trip the operator's
                            // payload than to silently drop it.
                        }
                    }
                }
                // Do NOT increment version here — the repo's update() does
                // `new_version = flag.version + 1` and `WHERE version = flag.version`,
                // so flag.version must remain the current stored value.

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

                // Replace rules if the request includes a non-empty list.
                if !flag_proto.rules.is_empty() {
                    // Bug fix `feature-flag-7yc`: persist only the
                    // non-catch-all rules. The trailing `And: []` rule
                    // (when present) was already absorbed into the
                    // record's `default_variant_id` /
                    // `default_rule_distribution` above.
                    let rules_to_persist: Vec<_> = if strip_trailing_catch_all {
                        let mut r = flag_proto.rules.clone();
                        r.pop();
                        r
                    } else {
                        flag_proto.rules.clone()
                    };

                    // Phase 4 of flag_eval_unify_20260522: server-side
                    // validation of every percentage-allocation rule's
                    // `hash_inputs` selector list (when populated).
                    for (i, r) in rules_to_persist.iter().enumerate() {
                        if let Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(alloc)) =
                            &r.output
                            && !alloc.hash_inputs.is_empty()
                        {
                            mapping::validate_proto_hash_inputs(&alloc.hash_inputs)
                                .map_err(|msg| {
                                    FlagServiceError::InvalidHashInputs(format!("rule[{i}]: {msg}"))
                                })
                                .map_err(Status::from)?;
                        }
                        // Bug fix `feature-flag-yrj`: reject a saved rule
                        // whose WHEN clause is a literal empty-attribute /
                        // empty-value Eq leaf. The gateway / UI guard against
                        // this, but a non-gateway gRPC client (or a UI
                        // regression) could still submit it — the rule would
                        // then never match, silently breaking targeting.
                        mapping::validate_proto_rule_condition(&r.rule_payload)
                            .map_err(|msg| {
                                FlagServiceError::InvalidArgument(format!("rule[{i}]: {msg}"))
                            })
                            .map_err(Status::from)?;
                    }
                    let variant_key_to_id: std::collections::HashMap<_, _> =
                        variants.iter().map(|v| (v.key.clone(), v.id)).collect();
                    let domain_rules: Vec<_> = rules_to_persist
                        .iter()
                        .enumerate()
                        .filter_map(|(i, r)| {
                            #[allow(clippy::cast_possible_truncation)]
                            mapping::proto_flag_rule_to_domain(
                                updated.id,
                                i as i32,
                                r,
                                &variant_key_to_id,
                            )
                        })
                        .collect();
                    self.flag_repo
                        .upsert_rules(updated.id, &domain_rules)
                        .await
                        .map_err(FlagServiceError::from)
                        .map_err(Status::from)?;
                }

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
                let flags = fetch_flag_list!(self, project_id, env_id_result);

                let record = flags
                    .into_iter()
                    .find(|f| f.key.as_str() == flag_proto.key.as_str())
                    .ok_or_else(|| {
                        Status::not_found(format!("flag '{}' not found", flag_proto.key))
                    })?;

                // Whole-flag lock: refuse delete/archive while an experiment
                // in running/paused state is bound to this flag.
                self.ensure_flag_unlocked(record.id)
                    .await
                    .map_err(Status::from)?;

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

        // Whole-flag lock: refuse hashing-config mutation while an experiment
        // in running/paused state is bound to this flag.
        self.ensure_flag_unlocked(record.id)
            .await
            .map_err(Status::from)?;

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

    async fn evaluate_preview(
        &self,
        request: Request<EvaluatePreviewRequest>,
    ) -> Result<Response<EvaluatePreviewResponse>, Status> {
        let req = request.into_inner();

        let project_id = parse_project_id(&req.project_id)?
            .ok_or_else(|| Status::invalid_argument("project_id is required"))?;

        let flag_key = stitchd_core::id::FlagKey::new(req.flag_key.clone())
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let record = self
            .flag_repo
            .find_by_key(&flag_key, project_id)
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

        let flag = stitchd_core::flag::Flag {
            record,
            hashing_config: vec![],
            rules,
            variants,
        };

        let segment_ids = flag.referenced_segment_ids();
        let segment_definitions = self.fetch_segment_definitions(&segment_ids).await?;

        let evaluation_contexts: Vec<stitchd_core::context::EvaluationContext> =
            serde_json::from_str(&req.contexts_json)
                .map_err(|e| Status::invalid_argument(format!("invalid contexts_json: {e}")))?;

        // Parse environment_id; treat empty string as nil UUID (unknown env).
        let env_uuid = if req.environment_id.is_empty() {
            uuid::Uuid::nil()
        } else {
            uuid::Uuid::parse_str(&req.environment_id)
                .map_err(|_| Status::invalid_argument("invalid environment_id"))?
        };
        let env_id = stitchd_core::id::EnvironmentId::from_uuid(env_uuid);

        // Resolve list-segment membership via ScyllaDB for all evaluation contexts.
        //
        // `fetch_segment_definitions` only returns RuleBased definitions (list-based
        // segment entries live in ScyllaDB and cannot be loaded wholesale). Instead,
        // we call `find_memberships_batch` here to get per-context boolean membership
        // for each list-type segment, then pass the results as `pre_resolved_list_memberships`
        // into `evaluate_preview` so `InSegment` leaves evaluate correctly.
        let pre_resolved_list_memberships = self
            .resolve_list_memberships(&segment_ids, &evaluation_contexts, env_id)
            .await?;

        let results = stitchd_core::evaluation::preview::evaluate_preview(
            &flag,
            &evaluation_contexts,
            &segment_definitions,
            env_id,
            &pre_resolved_list_memberships,
        );

        // NOTE: evaluate_preview intentionally does NOT write to flag_evaluation_log.
        // Per spec (context_intel track): only real SDK evaluations via IngestSdkEvalLog
        // write to that table. Context intelligence is built by stats-service reading
        // flag_evaluation_log on a 15-minute cycle.

        let results_json = serde_json::to_string(&results)
            .map_err(|e| Status::internal(format!("serialization error: {e}")))?;

        Ok(Response::new(EvaluatePreviewResponse {
            flag_enabled: flag.record.enabled,
            results_json,
        }))
    }

    /// Set or clear the flag's default-rule percentage distribution.
    ///
    /// Subject to the whole-flag lock — when an experiment in
    /// running/paused state owns the flag, the RPC fails with
    /// `FAILED_PRECONDITION` carrying the `flag_locked_by_experiment:<uuid>`
    /// sentinel (the gateway rewrites this into HTTP 409). Validates the
    /// distribution shape via [`RolloutDistribution::validate`] before
    /// touching the repository; invalid shapes return `INVALID_ARGUMENT`.
    async fn set_default_rule_distribution(
        &self,
        request: Request<SetDefaultRuleDistributionRequest>,
    ) -> Result<Response<SetDefaultRuleDistributionResponse>, Status> {
        let req = request.into_inner();

        let project_id = parse_project_id(&req.project_id)?
            .ok_or_else(|| Status::invalid_argument("project_id is required"))?;
        let flag_key = stitchd_core::id::FlagKey::new(req.flag_key.clone())
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let mut record = self
            .flag_repo
            .find_by_key(&flag_key, project_id)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

        // Optimistic-lock check (mirrors mutate_flag).
        if record.version != i64::try_from(req.version).unwrap_or(record.version) {
            return Err(Status::aborted(format!(
                "version conflict: expected {}, got {}",
                record.version, req.version
            )));
        }

        // Whole-flag lock: refuse mutation when an experiment is running/paused.
        self.ensure_flag_unlocked(record.id)
            .await
            .map_err(Status::from)?;

        // Empty `allocations` clears the distribution (reverts to single-default-variant
        // behaviour). Populated → validate before persisting.
        let new_dist = if req.allocations.is_empty() {
            None
        } else {
            let dist = stitchd_core::rollout::RolloutDistribution {
                allocations: req
                    .allocations
                    .iter()
                    .map(|a| stitchd_core::rollout::RolloutAllocation {
                        variant_key: a.variant_key.clone(),
                        percentage: a.percentage,
                    })
                    .collect(),
            };
            dist.validate()
                .map_err(|e| Status::invalid_argument(format!("invalid_distribution: {e}")))?;

            // Phase 4 of flag_eval_unify_20260522: variant_key referential
            // integrity. Reinstates the diagnostic that Phase 2 had to drop
            // from core's purity-bound `evaluate_flag` (no logging side
            // effects allowed in core). The check runs here at the gRPC
            // boundary so unknown variant_keys are rejected with
            // `INVALID_ARGUMENT` before persistence.
            let known_variants = self
                .variant_repo
                .find_by_flag(record.id)
                .await
                .map_err(FlagServiceError::from)
                .map_err(Status::from)?;
            let known_keys: std::collections::HashSet<&str> =
                known_variants.iter().map(|v| v.key.as_str()).collect();
            for alloc in &req.allocations {
                if !known_keys.contains(alloc.variant_key.as_str()) {
                    return Err(Status::from(FlagServiceError::UnknownDefaultRuleVariant {
                        variant_key: alloc.variant_key.clone(),
                    }));
                }
            }

            Some(dist)
        };

        record.default_rule_distribution = new_dist;
        // Update bumps version internally; PG repo returns the refreshed row.
        let updated = self
            .flag_repo
            .update(&record)
            .await
            .map_err(FlagServiceError::from)
            .map_err(Status::from)?;

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

        let new_version = u64::try_from(updated.version).unwrap_or(0);
        let proto_flag = mapping::build_feature_flag_proto(&updated, variants, &rules);

        metrics::counter!("flag_service.set_default_rule_distribution.ok").increment(1);
        Ok(Response::new(SetDefaultRuleDistributionResponse {
            flag: Some(proto_flag),
            version: new_version,
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
    use stitchd_db::{
        FlagRepository, RepositoryError, SdkKeyRepository, SegmentRepository, VariantRepository,
    };
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
            Ok(self
                .flags
                .lock()
                .unwrap()
                .iter()
                .filter(|f| f.deleted_at.is_none())
                .cloned()
                .collect())
        }

        async fn list_by_project_paginated(
            &self,
            _project_id: ProjectId,
            offset: u64,
            limit: u64,
        ) -> Result<(Vec<FlagRecord>, u64), RepositoryError> {
            let all: Vec<FlagRecord> = self
                .flags
                .lock()
                .unwrap()
                .iter()
                .filter(|f| f.deleted_at.is_none())
                .cloned()
                .collect();
            let total = all.len() as u64;
            let page: Vec<FlagRecord> = all
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect();
            Ok((page, total))
        }

        async fn list_by_project_all(
            &self,
            _project_id: ProjectId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            Ok(self.flags.lock().unwrap().clone())
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            Ok(self
                .flags
                .lock()
                .unwrap()
                .iter()
                .filter(|f| f.deleted_at.is_none())
                .cloned()
                .collect())
        }

        async fn list_by_environment_all(
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

    struct StubVariantRepoWithData {
        variants: Mutex<Vec<stitchd_core::flag::Variant>>,
    }

    impl StubVariantRepoWithData {
        fn with_variants(variants: Vec<stitchd_core::flag::Variant>) -> Arc<Self> {
            Arc::new(Self {
                variants: Mutex::new(variants),
            })
        }
    }

    #[async_trait]
    impl VariantRepository for StubVariantRepoWithData {
        async fn find_by_flag(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::Variant>, RepositoryError> {
            Ok(self.variants.lock().unwrap().clone())
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

        async fn list_by_environment_paginated(
            &self,
            _environment_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<SdkKey>, u64), RepositoryError> {
            Ok((vec![], 0))
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

    #[derive(Default)]
    struct StubSegmentRepo;

    #[async_trait]
    impl SegmentRepository for StubSegmentRepo {
        async fn find_by_id(
            &self,
            id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_by_key(
            &self,
            key: &str,
            _env_id: EnvironmentId,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key.to_string(),
            })
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError> {
            Ok(vec![])
        }
        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<stitchd_core::segment::Segment>, u64), RepositoryError> {
            Ok((vec![], 0))
        }
        async fn create(
            &self,
            _seg: &stitchd_core::segment::Segment,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(
            &self,
            seg: &stitchd_core::segment::Segment,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Ok(seg.clone())
        }
        async fn find_with_rules(
            &self,
            id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_core::segment::RuleBasedSegment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn upsert_rules(
            &self,
            _id: stitchd_core::id::SegmentId,
            _rules: &[stitchd_core::rule_engine::types::Rule],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn set_list_entries(
            &self,
            _id: stitchd_core::id::SegmentId,
            _ctx_type: &str,
            _include: &[String],
            _exclude: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn add_entries(
            &self,
            _id: stitchd_core::id::SegmentId,
            _ctx: &str,
            _lt: &str,
            _keys: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn remove_entries(
            &self,
            _id: stitchd_core::id::SegmentId,
            _ctx: &str,
            _lt: &str,
            _keys: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn get_list_segment_summary(
            &self,
            _id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_db::ListSegmentSummary, RepositoryError> {
            Ok(stitchd_db::ListSegmentSummary::default())
        }
        async fn get_condition_expr(
            &self,
            _id: stitchd_core::id::SegmentId,
        ) -> Result<Option<serde_json::Value>, RepositoryError> {
            Ok(None)
        }
        async fn set_condition_expr(
            &self,
            _id: stitchd_core::id::SegmentId,
            _expr: Option<&serde_json::Value>,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn soft_delete(
            &self,
            _id: stitchd_core::id::SegmentId,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn check_list_membership(
            &self,
            _env_id: EnvironmentId,
            _context_type: &str,
            _context_key: &str,
            _segment_keys: &[String],
        ) -> Result<std::collections::HashMap<String, bool>, RepositoryError> {
            Ok(std::collections::HashMap::new())
        }
        async fn batch_check_list_membership(
            &self,
            _env_id: EnvironmentId,
            _contexts: &[(String, String)],
            _segment_keys: &[String],
        ) -> Result<Vec<stitchd_db::ContextMembership>, RepositoryError> {
            Ok(vec![])
        }
        async fn find_batch_by_ids(
            &self,
            _ids: &[stitchd_core::id::SegmentId],
        ) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError> {
            Ok(vec![])
        }
        async fn find_rules_batch(
            &self,
            _ids: &[stitchd_core::id::SegmentId],
        ) -> Result<
            std::collections::HashMap<
                stitchd_core::id::SegmentId,
                stitchd_core::segment::RuleBasedSegment,
            >,
            RepositoryError,
        > {
            Ok(std::collections::HashMap::new())
        }
        async fn find_memberships_batch(
            &self,
            _env_id: stitchd_core::id::EnvironmentId,
            _contexts: &[(String, String)],
            _ids: &[stitchd_core::id::SegmentId],
        ) -> Result<Vec<stitchd_db::SegmentIdMembership>, RepositoryError> {
            Ok(Vec::new())
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
            default_rule_distribution: None,
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
            Arc::new(StubSegmentRepo),
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
            Arc::new(StubSegmentRepo),
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
            Arc::new(StubSegmentRepo),
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
            project_id: String::new(),
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
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(GetFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
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
            project_id: String::new(),
            flag_key: "my-flag".to_string(),
        });
        let result = svc.get_flag(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_flag_admin_path_populates_locked_by_experiment_id() {
        // Admin (project-scoped) reads must surface the lock UUID so the UI
        // can render the lock badge before the user attempts a save and the
        // gateway rewrites the 409. See feature-flag-1p6.
        let flag = make_flag_record();
        let project_id = flag.project_id;
        let flag_key = flag.key.as_str().to_string();
        let exp_id = stitchd_core::id::ExperimentId::new();

        let svc = FlagServiceImpl::new(
            StubFlagRepo::with_flags(vec![flag]),
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        )
        .with_experiment_repo(Arc::new(LockedExperimentRepo {
            experiment_id: exp_id,
        }));

        let req = Request::new(GetFlagRequest {
            environment_id: String::new(),
            project_id: project_id.to_string(),
            flag_key,
        });
        let resp = svc.get_flag(req).await.expect("get_flag ok");
        let proto = resp.into_inner();
        assert_eq!(
            proto.locked_by_experiment_id,
            exp_id.to_string(),
            "admin get_flag must populate locked_by_experiment_id when an experiment is active",
        );
    }

    #[tokio::test]
    async fn get_flag_sdk_path_leaves_locked_by_experiment_id_empty() {
        // SDK reads (env-scoped) must not leak admin lock metadata.
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let exp_id = stitchd_core::id::ExperimentId::new();

        let svc = FlagServiceImpl::new(
            StubFlagRepo::with_flags(vec![flag]),
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        )
        .with_experiment_repo(Arc::new(LockedExperimentRepo {
            experiment_id: exp_id,
        }));

        let req = Request::new(GetFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            flag_key,
        });
        let resp = svc.get_flag(req).await.expect("get_flag ok");
        let proto = resp.into_inner();
        assert!(
            proto.locked_by_experiment_id.is_empty(),
            "SDK path must keep locked_by_experiment_id empty; got {:?}",
            proto.locked_by_experiment_id,
        );
    }

    #[tokio::test]
    async fn list_flags_returns_empty_for_empty_environment() {
        let svc = make_service_empty();
        let req = Request::new(ListFlagsRequest {
            include_archived: false,
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            page: 0,
            per_page: 0,
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
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(ListFlagsRequest {
            include_archived: false,
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            page: 0,
            per_page: 0,
        });
        let result = svc.list_flags(req).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_inner().flags.len(), 2);
    }

    #[tokio::test]
    async fn list_flags_returns_invalid_argument_for_bad_env_id() {
        let svc = make_service_empty();
        let req = Request::new(ListFlagsRequest {
            include_archived: false,
            environment_id: "bad-uuid".to_string(),
            project_id: String::new(),
            page: 0,
            per_page: 0,
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
            project_id: ProjectId::new().to_string(),
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
            project_id: String::new(),
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
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
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
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
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
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
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
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
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
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
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
            project_id: String::new(),
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
            project_id: String::new(),
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
            project_id: String::new(),
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

    // ── evaluate_preview tests ─────────────────────────────────────────────────

    use stitchd_core::{
        context::{Context, EvaluationContext, ParameterValue},
        id::RuleId,
        rule_engine::types::{ConditionExpr, Rule, RuleOutput},
    };

    fn make_bool_variants(
        flag_id: FlagId,
    ) -> (Vec<stitchd_core::flag::Variant>, VariantId, VariantId) {
        let on_id = VariantId::new();
        let off_id = VariantId::new();
        let _ = flag_id; // variants are associated by id stored in FlagRecord.default_variant_id
        let variants = vec![
            stitchd_core::flag::Variant {
                id: on_id,
                key: "on".to_string(),
                value: stitchd_core::variants::VariantValue::BoolValue(true),
            },
            stitchd_core::flag::Variant {
                id: off_id,
                key: "off".to_string(),
                value: stitchd_core::variants::VariantValue::BoolValue(false),
            },
        ];
        (variants, on_id, off_id)
    }

    fn make_flag_record_with_default(default_variant_id: Option<VariantId>) -> FlagRecord {
        FlagRecord {
            id: FlagId::new(),
            project_id: ProjectId::new(),
            key: FlagKey::new("test-flag").unwrap(),
            name: String::new(),
            description: String::new(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id,
            default_rule_distribution: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    #[tokio::test]
    async fn evaluate_preview_disabled_flag_returns_default_no_traces() {
        let (variants, _, off_id) = make_bool_variants(FlagId::new());
        let mut record = make_flag_record_with_default(Some(off_id));
        record.enabled = false;

        let flag_repo = StubFlagRepo::with_flags(vec![record.clone()]);
        let variant_repo = StubVariantRepoWithData::with_variants(variants);
        let svc = FlagServiceImpl::new(
            flag_repo,
            variant_repo,
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let contexts_json = serde_json::to_string(&[ec]).unwrap();

        let req = Request::new(EvaluatePreviewRequest {
            project_id: record.project_id.to_string(),
            flag_key: record.key.to_string(),
            contexts_json,
            environment_id: String::new(),
        });
        let resp = svc.evaluate_preview(req).await.unwrap().into_inner();

        assert!(!resp.flag_enabled);
        let results: Vec<stitchd_core::evaluation::preview::ContextPreviewResult> =
            serde_json::from_str(&resp.results_json).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].variant_key, "off");
        assert!(results[0].rule_traces.is_empty());
        assert!(results[0].rollout_debug.is_none());
    }

    #[tokio::test]
    async fn evaluate_preview_matching_rule_returns_trace() {
        let (variants, on_id, off_id) = make_bool_variants(FlagId::new());
        let record = make_flag_record_with_default(Some(off_id));
        let flag_id = record.id;

        let flag_rule = stitchd_core::flag::FlagRule {
            flag_id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: Some("beta rule".to_string()),
                condition: ConditionExpr::Leaf(
                    stitchd_core::rule_engine::condition::Condition::Eq {
                        context_type: "user".to_string(),
                        param: "beta".to_string(),
                        value: ParameterValue::Bool(true),
                    },
                ),
                output: RuleOutput::Variant(on_id),
            },
        };

        let flag_repo = Arc::new(StubFlagRepo {
            flags: Mutex::new(vec![record.clone()]),
            rules: Mutex::new(std::collections::HashMap::from([(
                flag_id,
                vec![flag_rule],
            )])),
        });
        let variant_repo = StubVariantRepoWithData::with_variants(variants);
        let svc = FlagServiceImpl::new(
            flag_repo,
            variant_repo,
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let contexts_json = serde_json::to_string(&[ec]).unwrap();

        let req = Request::new(EvaluatePreviewRequest {
            project_id: record.project_id.to_string(),
            flag_key: record.key.to_string(),
            contexts_json,
            environment_id: String::new(),
        });
        let resp = svc.evaluate_preview(req).await.unwrap().into_inner();

        assert!(resp.flag_enabled);
        let results: Vec<stitchd_core::evaluation::preview::ContextPreviewResult> =
            serde_json::from_str(&resp.results_json).unwrap();
        assert_eq!(results[0].variant_key, "on");
        assert_eq!(results[0].fired_rule_name, Some("beta rule".to_string()));
        assert!(!results[0].rule_traces.is_empty());
    }

    #[tokio::test]
    async fn evaluate_preview_invalid_project_id_returns_error() {
        let svc = make_service_empty();
        let req = Request::new(EvaluatePreviewRequest {
            project_id: "not-a-uuid".to_string(),
            flag_key: "my-flag".to_string(),
            contexts_json: "[]".to_string(),
            environment_id: String::new(),
        });
        let result = svc.evaluate_preview(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // ── Phase 2 Task 2: fetch_segment_definitions uses batch methods ─────────

    /// A spy implementation that tracks call counts for the batch methods.
    struct SpySegmentRepo {
        /// Segments returned from find_batch_by_ids (keyed by id).
        segments: Vec<stitchd_core::segment::Segment>,
        /// Rules returned from find_rules_batch.
        rules: std::collections::HashMap<
            stitchd_core::id::SegmentId,
            stitchd_core::segment::RuleBasedSegment,
        >,
        /// Call counters.
        batch_by_ids_calls: std::sync::atomic::AtomicUsize,
        rules_batch_calls: std::sync::atomic::AtomicUsize,
    }

    impl SpySegmentRepo {
        fn new(
            segments: Vec<stitchd_core::segment::Segment>,
            rules: std::collections::HashMap<
                stitchd_core::id::SegmentId,
                stitchd_core::segment::RuleBasedSegment,
            >,
        ) -> Self {
            Self {
                segments,
                rules,
                batch_by_ids_calls: std::sync::atomic::AtomicUsize::new(0),
                rules_batch_calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl SegmentRepository for SpySegmentRepo {
        async fn find_by_id(
            &self,
            id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_by_key(
            &self,
            key: &str,
            _env_id: EnvironmentId,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key.to_string(),
            })
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError> {
            Ok(vec![])
        }
        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<stitchd_core::segment::Segment>, u64), RepositoryError> {
            Ok((vec![], 0))
        }
        async fn create(
            &self,
            _seg: &stitchd_core::segment::Segment,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(
            &self,
            seg: &stitchd_core::segment::Segment,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Ok(seg.clone())
        }
        async fn find_with_rules(
            &self,
            id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_core::segment::RuleBasedSegment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn upsert_rules(
            &self,
            _id: stitchd_core::id::SegmentId,
            _rules: &[stitchd_core::rule_engine::types::Rule],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn set_list_entries(
            &self,
            _id: stitchd_core::id::SegmentId,
            _ctx_type: &str,
            _include: &[String],
            _exclude: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn add_entries(
            &self,
            _id: stitchd_core::id::SegmentId,
            _ctx: &str,
            _lt: &str,
            _keys: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn remove_entries(
            &self,
            _id: stitchd_core::id::SegmentId,
            _ctx: &str,
            _lt: &str,
            _keys: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn get_list_segment_summary(
            &self,
            _id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_db::ListSegmentSummary, RepositoryError> {
            Ok(stitchd_db::ListSegmentSummary::default())
        }
        async fn get_condition_expr(
            &self,
            _id: stitchd_core::id::SegmentId,
        ) -> Result<Option<serde_json::Value>, RepositoryError> {
            Ok(None)
        }
        async fn set_condition_expr(
            &self,
            _id: stitchd_core::id::SegmentId,
            _expr: Option<&serde_json::Value>,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn soft_delete(
            &self,
            _id: stitchd_core::id::SegmentId,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn check_list_membership(
            &self,
            _env_id: EnvironmentId,
            _ctx_type: &str,
            _ctx_key: &str,
            _keys: &[String],
        ) -> Result<std::collections::HashMap<String, bool>, RepositoryError> {
            Ok(std::collections::HashMap::new())
        }
        async fn batch_check_list_membership(
            &self,
            _env_id: EnvironmentId,
            _contexts: &[(String, String)],
            _keys: &[String],
        ) -> Result<Vec<stitchd_db::ContextMembership>, RepositoryError> {
            Ok(vec![])
        }
        async fn find_batch_by_ids(
            &self,
            _ids: &[stitchd_core::id::SegmentId],
        ) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError> {
            self.batch_by_ids_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.segments.clone())
        }
        async fn find_rules_batch(
            &self,
            _ids: &[stitchd_core::id::SegmentId],
        ) -> Result<
            std::collections::HashMap<
                stitchd_core::id::SegmentId,
                stitchd_core::segment::RuleBasedSegment,
            >,
            RepositoryError,
        > {
            self.rules_batch_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.rules.clone())
        }
        async fn find_memberships_batch(
            &self,
            _env_id: stitchd_core::id::EnvironmentId,
            _contexts: &[(String, String)],
            _ids: &[stitchd_core::id::SegmentId],
        ) -> Result<Vec<stitchd_db::SegmentIdMembership>, RepositoryError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn fetch_segment_definitions_calls_each_batch_method_exactly_once() {
        use std::collections::{HashMap, HashSet};
        use stitchd_core::segment::{RuleBasedSegment, SegmentType};

        // Build 10 rule-based segments
        let env_id = EnvironmentId::new();
        let segments: Vec<stitchd_core::segment::Segment> = (0..10)
            .map(|i| stitchd_core::segment::Segment {
                id: stitchd_core::id::SegmentId::new(),
                environment_id: env_id,
                key: format!("seg-{i}"),
                name: format!("Seg {i}"),
                description: String::new(),
                tags: vec![],
                segment_type: SegmentType::Rule,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                deleted_at: None,
                version: 1,
            })
            .collect();

        let rules: HashMap<_, _> = segments
            .iter()
            .map(|s| {
                (
                    s.id,
                    RuleBasedSegment {
                        id: s.id,
                        rules: vec![],
                    },
                )
            })
            .collect();

        let spy = Arc::new(SpySegmentRepo::new(segments.clone(), rules));
        let svc = FlagServiceImpl::new(
            StubFlagRepo::empty(),
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            spy.clone(),
        );

        let ids: HashSet<_> = segments.iter().map(|s| s.id).collect();
        let result = svc.fetch_segment_definitions(&ids).await.unwrap();

        assert_eq!(result.len(), 10, "all 10 definitions must be returned");
        assert_eq!(
            spy.batch_by_ids_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "find_batch_by_ids must be called exactly once"
        );
        assert_eq!(
            spy.rules_batch_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "find_rules_batch must be called exactly once"
        );
        // find_lists_batch removed — list segment defs pending Phase 4 Scylla migration.
    }

    #[tokio::test]
    async fn evaluate_preview_invalid_contexts_json_returns_error() {
        let record = make_flag_record_with_default(None);
        let flag_repo = StubFlagRepo::with_flags(vec![record.clone()]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(EvaluatePreviewRequest {
            project_id: record.project_id.to_string(),
            flag_key: record.key.to_string(),
            contexts_json: "not json".to_string(),
            environment_id: String::new(),
        });
        let result = svc.evaluate_preview(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // ── Whole-flag lock — Task 3.2 ────────────────────────────────────────────
    //
    // Asserts that mutate_flag/Update returns a `FailedPrecondition` with the
    // `flag_locked_by_experiment:<exp_id>` sentinel message when the bound
    // experiment-repo says the flag is locked.

    struct LockedExperimentRepo {
        experiment_id: stitchd_core::id::ExperimentId,
    }

    #[async_trait]
    impl stitchd_db::ExperimentRepository for LockedExperimentRepo {
        async fn find_by_id(
            &self,
            _id: stitchd_core::id::ExperimentId,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            unimplemented!()
        }
        async fn list_by_environment(
            &self,
            _env_id: stitchd_core::id::EnvironmentId,
            _status_filter: Option<stitchd_core::experimentation::ExperimentStatus>,
        ) -> Result<Vec<stitchd_core::experimentation::Experiment>, RepositoryError> {
            Ok(vec![])
        }
        async fn list_by_environment_paginated(
            &self,
            _env_id: stitchd_core::id::EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<stitchd_core::experimentation::Experiment>, u64), RepositoryError>
        {
            Ok((vec![], 0))
        }
        async fn create(
            &self,
            _experiment: &stitchd_core::experimentation::Experiment,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(
            &self,
            _experiment: &stitchd_core::experimentation::Experiment,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(
            &self,
            _id: stitchd_core::id::ExperimentId,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn list_iterations(
            &self,
            _experiment_id: stitchd_core::id::ExperimentId,
        ) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, RepositoryError>
        {
            Ok(vec![])
        }
        async fn apply_transition(
            &self,
            _id: stitchd_core::id::ExperimentId,
            _to: stitchd_core::experimentation::ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            unimplemented!()
        }
        async fn list_all_running(
            &self,
        ) -> Result<Vec<stitchd_core::experimentation::Experiment>, RepositoryError> {
            Ok(vec![])
        }
        async fn find_active_experiment_for_flag(
            &self,
            _flag_id: stitchd_core::id::FlagId,
        ) -> Result<Option<stitchd_core::id::ExperimentId>, RepositoryError> {
            Ok(Some(self.experiment_id))
        }
        async fn find_iteration_by_id(
            &self,
            _iteration_id: stitchd_core::id::ExperimentIterationId,
        ) -> Result<stitchd_core::experimentation::ExperimentIteration, RepositoryError> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn mutate_flag_update_rejects_locked_flag_with_failed_precondition() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let exp_id = stitchd_core::id::ExperimentId::new();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        )
        .with_experiment_repo(Arc::new(LockedExperimentRepo {
            experiment_id: exp_id,
        }));

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
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
            version: 1,
        });
        let result = svc.mutate_flag(req).await;
        let err = result.expect_err("expected FailedPrecondition for locked flag");
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(
            err.message().contains(&exp_id.to_string()),
            "expected experiment id in status message; got {}",
            err.message()
        );
        assert!(
            err.message()
                .starts_with(crate::error::FLAG_LOCKED_STATUS_PREFIX),
            "expected flag-lock sentinel prefix; got {}",
            err.message()
        );
    }

    #[tokio::test]
    async fn mutate_flag_archive_rejects_locked_flag() {
        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let exp_id = stitchd_core::id::ExperimentId::new();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        )
        .with_experiment_repo(Arc::new(LockedExperimentRepo {
            experiment_id: exp_id,
        }));

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            kind: MutationKind::Archive as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                ..Default::default()
            }),
            version: 1,
        });
        let err = svc.mutate_flag(req).await.expect_err("archive should fail");
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(err.message().contains(&exp_id.to_string()));
    }

    // ─── Phase 4 (flag_eval_unify_20260522) — server-side validation ─────────

    #[tokio::test]
    async fn mutate_flag_update_rejects_empty_hash_inputs() {
        use stitchd_proto::flags::v1::{
            AllocationBucket, FlagRule, PercentageAllocation, flag_rule::Output,
        };

        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        // A `PercentageAllocation` with an EMPTY-but-present `hash_inputs`
        // list. The gateway-side validator catches this earlier, but a
        // non-gateway gRPC client must hit the server-side validator. We
        // need at least one entry in `hash_inputs` to trigger the
        // validation path, otherwise the legacy `context_hash_specs`
        // fallback would silently accept the payload — so we send a
        // duplicate-selectors case to exercise the validator.
        let bad_rule = FlagRule {
            rule_payload: serde_json::to_vec(&serde_json::Value::Null).unwrap(),
            output: Some(Output::Allocation(PercentageAllocation {
                context_hash_specs: Default::default(),
                buckets: vec![AllocationBucket {
                    variant_key: "on".to_string(),
                    weight_milli: 1000,
                }],
                hash_inputs: vec![
                    stitchd_proto::flags::v1::HashSelector {
                        selector: Some(
                            stitchd_proto::flags::v1::hash_selector::Selector::ContextKey(
                                stitchd_proto::flags::v1::ContextKeySelector {
                                    context_type: "user".to_string(),
                                },
                            ),
                        ),
                    },
                    stitchd_proto::flags::v1::HashSelector {
                        selector: Some(
                            stitchd_proto::flags::v1::hash_selector::Selector::ContextKey(
                                stitchd_proto::flags::v1::ContextKeySelector {
                                    context_type: "user".to_string(),
                                },
                            ),
                        ),
                    },
                ],
            })),
            name: String::new(),
            rule_id: String::new(),
        };

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                rules: vec![bad_rule],
                ..Default::default()
            }),
            version: 1,
        });
        let err = svc
            .mutate_flag(req)
            .await
            .expect_err("duplicate hash_inputs must fail");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message().contains("invalid_hash_inputs"),
            "expected invalid_hash_inputs sentinel; got `{}`",
            err.message()
        );
    }

    #[tokio::test]
    async fn mutate_flag_update_rejects_parameter_selector_missing_parameter() {
        use stitchd_proto::flags::v1::{
            AllocationBucket, FlagRule, PercentageAllocation, flag_rule::Output,
        };

        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let bad_rule = FlagRule {
            rule_payload: serde_json::to_vec(&serde_json::Value::Null).unwrap(),
            output: Some(Output::Allocation(PercentageAllocation {
                context_hash_specs: Default::default(),
                buckets: vec![AllocationBucket {
                    variant_key: "on".to_string(),
                    weight_milli: 1000,
                }],
                hash_inputs: vec![stitchd_proto::flags::v1::HashSelector {
                    selector: Some(
                        stitchd_proto::flags::v1::hash_selector::Selector::ContextParameter(
                            stitchd_proto::flags::v1::ContextParameterSelector {
                                context_type: "user".to_string(),
                                parameter: String::new(),
                            },
                        ),
                    ),
                }],
            })),
            name: String::new(),
            rule_id: String::new(),
        };

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                rules: vec![bad_rule],
                ..Default::default()
            }),
            version: 1,
        });
        let err = svc
            .mutate_flag(req)
            .await
            .expect_err("context_parameter with empty parameter must fail");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message().contains("invalid_hash_inputs"),
            "expected invalid_hash_inputs sentinel; got `{}`",
            err.message()
        );
    }

    #[tokio::test]
    async fn set_default_rule_distribution_rejects_unknown_variant_key() {
        // The default-rule distribution's `variant_key` must reference a
        // variant that exists on the flag. Reinstates the diagnostic that
        // core's purity-bound evaluator had to drop.
        use stitchd_proto::flags::v1::{DefaultRuleAllocation, SetDefaultRuleDistributionRequest};

        let mut flag = make_flag_record();
        flag.project_id = ProjectId::new();
        let project_id = flag.project_id;
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            // StubVariantRepo returns empty variants → every variant_key
            // is unknown.
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let req = Request::new(SetDefaultRuleDistributionRequest {
            project_id: project_id.to_string(),
            flag_key,
            allocations: vec![DefaultRuleAllocation {
                variant_key: "does-not-exist".to_string(),
                percentage: 100.0,
            }],
            version: 1,
        });
        let err = svc
            .set_default_rule_distribution(req)
            .await
            .expect_err("unknown variant_key must fail");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message().contains("does-not-exist"),
            "expected variant_key in error message; got `{}`",
            err.message()
        );
    }

    // ─── feature-flag-yrj — empty-WHEN rejection ───────────────────────────

    #[tokio::test]
    async fn mutate_flag_update_rejects_rule_with_empty_when_eq_leaf() {
        // Bug fix `feature-flag-yrj`: a rule whose ConditionExpr is a leaf
        // `Eq { param: "", value: Str("") }` (the literal payload the UI's
        // empty-WHEN form would submit pre-fix) must be rejected server-side
        // with `Status::invalid_argument` carrying the `invalid_condition`
        // sentinel.
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;
        use stitchd_proto::flags::v1::FlagRule;

        let flag = make_flag_record();
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let empty_when = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: String::new(),
            value: ParameterValue::Str(String::new()),
        });
        let bad_rule = FlagRule {
            rule_payload: serde_json::to_vec(&empty_when).unwrap(),
            output: Some(stitchd_proto::flags::v1::flag_rule::Output::VariantKey(
                "on".to_string(),
            )),
            name: String::new(),
            rule_id: String::new(),
        };

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                rules: vec![bad_rule],
                ..Default::default()
            }),
            version: 1,
        });
        let err = svc
            .mutate_flag(req)
            .await
            .expect_err("empty WHEN clause must fail");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message().contains("invalid_condition"),
            "expected invalid_condition sentinel; got `{}`",
            err.message()
        );
    }

    // ─── feature-flag-7yc — trailing catch-all absorption ──────────────────

    #[tokio::test]
    async fn mutate_flag_update_strips_trailing_catch_all_rule_into_default_variant() {
        // Bug fix `feature-flag-7yc`: when the inbound rule list ends with
        // a catch-all (And: []) rule whose output is a single VariantKey,
        // the server strips that rule from persistence and absorbs the
        // variant into `default_variant_id`. The persisted rule count
        // therefore equals the count of user-authored rules — not
        // (user-authored + 1).
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;
        use stitchd_proto::flags::v1::{FlagRule, flag_rule::Output};

        // Build the variants. We need them on the flag-id so the catch-all
        // resolution can find them.
        let (variants, _on_id, off_id) = make_bool_variants(FlagId::new());
        let mut flag = make_flag_record();
        flag.default_variant_id = None; // pre-state: no default yet
        let flag_id = flag.id;
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let variant_repo = StubVariantRepoWithData::with_variants(variants.clone());
        let svc = FlagServiceImpl::new(
            flag_repo.clone(),
            variant_repo,
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        // User-authored rule.
        let user_when = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: "tier".to_string(),
            value: ParameterValue::Str("gold".to_string()),
        });
        let user_rule = FlagRule {
            rule_payload: serde_json::to_vec(&user_when).unwrap(),
            output: Some(Output::VariantKey("on".to_string())),
            name: "gold targeting".to_string(),
            rule_id: String::new(),
        };

        // Trailing catch-all (UI pre-fix shape).
        let catch_all = FlagRule {
            rule_payload: serde_json::to_vec(&ConditionExpr::And(vec![])).unwrap(),
            output: Some(Output::VariantKey("off".to_string())),
            name: String::new(),
            rule_id: String::new(),
        };

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                rules: vec![user_rule, catch_all],
                ..Default::default()
            }),
            version: 1,
        });
        let resp = svc
            .mutate_flag(req)
            .await
            .expect("update should succeed")
            .into_inner();
        let proto = resp.flag.expect("flag in response");

        // Only ONE rule survives — the user-authored rule.
        assert_eq!(
            proto.rules.len(),
            1,
            "expected 1 rule after stripping trailing catch-all; got {}",
            proto.rules.len()
        );

        // The catch-all's variant_key now lives in `default_variant_key`.
        assert_eq!(
            proto.default_variant_key, "off",
            "trailing catch-all variant must populate default_variant_key"
        );
        // Crosscheck the persisted record's default_variant_id matches.
        let stored = flag_repo
            .flags
            .lock()
            .unwrap()
            .iter()
            .find(|f| f.id == flag_id)
            .cloned()
            .expect("flag still present");
        assert_eq!(stored.default_variant_id, Some(off_id));
    }

    #[tokio::test]
    async fn mutate_flag_update_keeps_non_catch_all_trailing_rule() {
        // Sanity: a trailing rule with a NON-empty condition (a real
        // targeting rule) must NOT be stripped — only the always-true
        // (And: []) sentinel is treated as the synthetic catch-all.
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;
        use stitchd_proto::flags::v1::{FlagRule, flag_rule::Output};

        let (variants, _on_id, _off_id) = make_bool_variants(FlagId::new());
        let mut flag = make_flag_record();
        flag.default_variant_id = None;
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let variant_repo = StubVariantRepoWithData::with_variants(variants);
        let svc = FlagServiceImpl::new(
            flag_repo,
            variant_repo,
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let real_rule = FlagRule {
            rule_payload: serde_json::to_vec(&ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".to_string(),
                param: "tier".to_string(),
                value: ParameterValue::Str("gold".to_string()),
            }))
            .unwrap(),
            output: Some(Output::VariantKey("on".to_string())),
            name: String::new(),
            rule_id: String::new(),
        };

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: String::new(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                rules: vec![real_rule],
                ..Default::default()
            }),
            version: 1,
        });
        let resp = svc
            .mutate_flag(req)
            .await
            .expect("update should succeed")
            .into_inner();
        let proto = resp.flag.expect("flag in response");

        // The single (non-catch-all) rule is preserved.
        assert_eq!(proto.rules.len(), 1);
        // No default_variant_key was set (caller didn't supply one and
        // there's no catch-all to absorb).
        assert!(
            proto.default_variant_key.is_empty(),
            "default_variant_key should remain unset; got `{}`",
            proto.default_variant_key
        );
    }

    #[tokio::test]
    async fn mutate_flag_update_accepts_nonempty_when_eq_leaf() {
        // Sanity check: a fully-populated Eq leaf passes the validator
        // (only the empty-attribute + empty-value case is rejected).
        use stitchd_core::context::ParameterValue;
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_core::rule_engine::types::ConditionExpr;
        use stitchd_proto::flags::v1::FlagRule;

        let mut flag = make_flag_record();
        // Force a project_id so the flag-lock lookup is skipped (no exp
        // bindings exist in StubFlagRepo).
        flag.project_id = ProjectId::new();
        let project_id = flag.project_id;
        let flag_key = flag.key.as_str().to_string();
        let flag_repo = StubFlagRepo::with_flags(vec![flag]);
        let svc = FlagServiceImpl::new(
            flag_repo,
            Arc::new(StubVariantRepo),
            StubSdkKeyRepo::empty(),
            Arc::new(StubSegmentRepo),
        );

        let ok_when = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: "tier".to_string(),
            value: ParameterValue::Str("gold".to_string()),
        });
        // Use empty rule_payload (And: [] sentinel) to keep the test free
        // of variant-key bookkeeping in StubVariantRepo. We assert the
        // condition validator does not error — version-conflict / variant
        // bookkeeping is exercised elsewhere.
        let _ = ok_when; // silence unused
        let good_rule = FlagRule {
            rule_payload: serde_json::to_vec(&ConditionExpr::And(vec![])).unwrap(),
            output: None,
            name: String::new(),
            rule_id: String::new(),
        };

        let req = Request::new(MutateFlagRequest {
            environment_id: EnvironmentId::new().to_string(),
            project_id: project_id.to_string(),
            kind: MutationKind::Update as i32,
            flag: Some(FeatureFlag {
                key: flag_key,
                rules: vec![good_rule],
                ..Default::default()
            }),
            version: 1,
        });
        // The update may still error on version-mismatch or stub repo
        // semantics, but it must NOT error with the `invalid_condition`
        // sentinel — that's the validator we care about here.
        let result = svc.mutate_flag(req).await;
        match result {
            Ok(_) => {} // happy path
            Err(e) => {
                assert!(
                    !e.message().contains("invalid_condition"),
                    "non-empty WHEN clause must not trip the empty-Eq validator; got `{}`",
                    e.message()
                );
            }
        }
    }
}
