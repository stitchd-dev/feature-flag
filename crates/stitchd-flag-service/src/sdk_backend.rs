//! Backend gRPC implementation of [`FlagSdkBackendService`].
//!
//! This is the **backend half** of the SDK contract: the gateway forwards
//! authenticated SDK calls here. We MUST NOT validate the SDK key — the
//! gateway has already done that and propagates the resolved environment id
//! in the `x-env-id` gRPC metadata header. Requests without that header are
//! rejected with `Unauthenticated`.
//!
//! See `sdks/spec/proto/sdk/v1/backend.proto` for the wire contract.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use tonic::{Request, Response, Status};

use stitchd_core::id::EnvironmentId;
use stitchd_core::segment::SegmentType;
use stitchd_db::{FlagRepository, SegmentRepository, VariantRepository};
use stitchd_proto::sdk::v1::{
    FlagEvaluationEvent, IngestSdkEvalLogRequest, IngestSdkEvalLogResponse, SyncDefinitionsRequest,
    SyncDefinitionsResponse, flag_sdk_backend_service_server::FlagSdkBackendService,
};

use crate::mapping;

const ENV_ID_METADATA_KEY: &str = "x-env-id";

/// Backend gRPC service hosted on `stitchd-flag-service`. Called by the
/// gateway after SDK-key validation succeeds.
#[allow(clippy::struct_field_names)]
pub struct FlagSdkBackendServiceImpl {
    flag_repo: Arc<dyn FlagRepository>,
    variant_repo: Arc<dyn VariantRepository>,
    segment_repo: Arc<dyn SegmentRepository>,
}

impl FlagSdkBackendServiceImpl {
    /// Construct a new service backed by the given repositories.
    ///
    /// Eval-log wiring (`IngestSdkEvalLog`) lands in Phase 3 Task 3 — until
    /// then `ingest_sdk_eval_log` accepts requests but discards events.
    #[must_use]
    pub fn new(
        flag_repo: Arc<dyn FlagRepository>,
        variant_repo: Arc<dyn VariantRepository>,
        segment_repo: Arc<dyn SegmentRepository>,
    ) -> Self {
        Self {
            flag_repo,
            variant_repo,
            segment_repo,
        }
    }
}

/// Extract the resolved environment id from `x-env-id` gRPC metadata.
///
/// The gateway is the sole trust boundary — it validates the SDK key and
/// resolves the environment, then forwards the request here with the env id
/// in metadata. Requests without this header indicate either a misbehaving
/// gateway OR an SDK reaching the backend directly (bypassing auth); both
/// cases get rejected with `Unauthenticated`.
fn env_id_from_metadata(req: &Request<impl Sized>) -> Result<EnvironmentId, Status> {
    let raw = req
        .metadata()
        .get(ENV_ID_METADATA_KEY)
        .ok_or_else(|| Status::unauthenticated(format!("missing {ENV_ID_METADATA_KEY} metadata")))?
        .to_str()
        .map_err(|_| Status::unauthenticated(format!("{ENV_ID_METADATA_KEY} is not valid UTF-8")))?;
    let uuid = uuid::Uuid::parse_str(raw)
        .map_err(|_| Status::unauthenticated(format!("{ENV_ID_METADATA_KEY} is not a valid UUID")))?;
    Ok(EnvironmentId::from_uuid(uuid))
}

#[tonic::async_trait]
impl FlagSdkBackendService for FlagSdkBackendServiceImpl {
    async fn sync_definitions(
        &self,
        request: Request<SyncDefinitionsRequest>,
    ) -> Result<Response<SyncDefinitionsResponse>, Status> {
        let env_id = env_id_from_metadata(&request)?;

        // 1. Fetch all flag records for this environment.
        let flag_records = self
            .flag_repo
            .list_by_environment(env_id)
            .await
            .map_err(|e| Status::internal(format!("flag_repo.list_by_environment failed: {e}")))?;

        // 2. Build proto FeatureFlags (variants + rules per flag).
        let mut flags = Vec::with_capacity(flag_records.len());
        for record in &flag_records {
            let variants = self
                .variant_repo
                .find_by_flag(record.id)
                .await
                .map_err(|e| {
                    Status::internal(format!("variant_repo.find_by_flag failed: {e}"))
                })?;
            let rules = self
                .flag_repo
                .find_rules(record.id)
                .await
                .map_err(|e| Status::internal(format!("flag_repo.find_rules failed: {e}")))?;
            flags.push(mapping::build_feature_flag_proto(record, variants, &rules));
        }

        // 3. Fetch all segments for this environment.
        let segments = self
            .segment_repo
            .list_by_environment(env_id)
            .await
            .map_err(|e| {
                Status::internal(format!("segment_repo.list_by_environment failed: {e}"))
            })?;

        // 4. Partition into rule-based + list-based; bulk-fetch each kind's payload.
        let (rule_segment_records, list_segment_records): (Vec<_>, Vec<_>) = segments
            .into_iter()
            .partition(|s| s.segment_type == SegmentType::Rule);

        let rule_ids: Vec<_> = rule_segment_records.iter().map(|s| s.id).collect();
        let list_ids: Vec<_> = list_segment_records.iter().map(|s| s.id).collect();

        let rule_payloads = self
            .segment_repo
            .find_rules_batch(&rule_ids)
            .await
            .map_err(|e| {
                Status::internal(format!("segment_repo.find_rules_batch failed: {e}"))
            })?;
        let list_payloads = self
            .segment_repo
            .find_lists_batch(&list_ids)
            .await
            .map_err(|e| {
                Status::internal(format!("segment_repo.find_lists_batch failed: {e}"))
            })?;

        // 5. Build proto rule-segments (with serialized rule tree in `rule_payload`).
        //    For each rule-based segment we emit one entry; `context_type` is empty
        //    for rule-based segments (they're context-agnostic — leaves declare their
        //    own context_type via the ConditionExpr).
        let rule_segments = rule_segment_records
            .iter()
            .map(|s| {
                let payload_bytes = rule_payloads
                    .get(&s.id)
                    .map(|rb| serde_json::to_vec(&rb.rules).unwrap_or_default())
                    .unwrap_or_default();
                stitchd_proto::segments::v1::RuleSegment {
                    id: s.id.to_string(),
                    key: s.key.clone(),
                    context_type: String::new(),
                    rule_payload: payload_bytes,
                }
            })
            .collect::<Vec<_>>();

        // 6. Build proto list-segment metadata. A list segment can target multiple
        //    context types (HashMap keyed on context_type). We emit ONE entry per
        //    (segment_id, context_type) pair so the SDK's LRU resolution stays
        //    well-defined. If the segment has no entries yet, context_type is empty.
        //    NOTE: This duplicates `id` across rows for multi-context-type segments;
        //    the SDK's snapshot dedupes by (id, context_type).
        let mut list_segments = Vec::new();
        for s in &list_segment_records {
            if let Some(lb) = list_payloads.get(&s.id) {
                if lb.lists.is_empty() {
                    list_segments.push(stitchd_proto::segments::v1::ListSegmentMeta {
                        id: s.id.to_string(),
                        key: s.key.clone(),
                        context_type: String::new(),
                    });
                } else {
                    // Sort context types for deterministic output (important for tests).
                    let mut ctx_types: Vec<&String> = lb.lists.keys().collect();
                    ctx_types.sort();
                    for ct in ctx_types {
                        list_segments.push(stitchd_proto::segments::v1::ListSegmentMeta {
                            id: s.id.to_string(),
                            key: s.key.clone(),
                            context_type: ct.clone(),
                        });
                    }
                }
            } else {
                // Segment exists but has no list rows yet (pristine state).
                list_segments.push(stitchd_proto::segments::v1::ListSegmentMeta {
                    id: s.id.to_string(),
                    key: s.key.clone(),
                    context_type: String::new(),
                });
            }
        }

        let server_timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);

        Ok(Response::new(SyncDefinitionsResponse {
            flags,
            rule_segments,
            list_segments,
            server_timestamp_ms,
            environment_id: env_id.to_string(),
        }))
    }

    async fn ingest_sdk_eval_log(
        &self,
        request: Request<IngestSdkEvalLogRequest>,
    ) -> Result<Response<IngestSdkEvalLogResponse>, Status> {
        // Real implementation lands in Phase 3 Task 3 — for now we accept the
        // request to keep the trait implementation valid but discard events
        // (logging a warning so it's visible in dev).
        let env_id = env_id_from_metadata(&request)?;
        let n = request.into_inner().events.len();
        // Stamp env_id on each event before forwarding to eval_log_writer (Task 3.3).
        let _ = (env_id, n);
        // No-op until Task 3.3 wires up the eval_log_writer.
        tracing::warn!(
            event_count = n,
            "ingest_sdk_eval_log called but eval_log writer not wired (Phase 3 Task 3 pending)"
        );
        Ok(Response::new(IngestSdkEvalLogResponse {}))
    }
}

// Re-export FlagEvaluationEvent so callers in Phase 3 Task 3 don't need a
// second import path. Currently unused — silences dead_code warning until wired.
#[allow(dead_code)]
pub(crate) fn _unused_event_marker(e: FlagEvaluationEvent) -> chrono::DateTime<Utc> {
    let _ = e.flag_key;
    Utc::now()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use stitchd_core::context::Context;
    use stitchd_core::flag::{FlagRecord, FlagRule, FlagValueType, Variant};
    use stitchd_core::id::{FlagId, FlagKey, ProjectId, SegmentId, VariantId};
    use stitchd_core::segment::{
        ContextList, ListBasedSegment, RuleBasedSegment, Segment, SegmentType,
    };
    use stitchd_db::{ContextMembership, RepositoryError};

    // ── Stub repositories ───────────────────────────────────────────────────

    #[derive(Default)]
    struct StubFlagRepo {
        flags_by_env: Mutex<HashMap<EnvironmentId, Vec<FlagRecord>>>,
        rules_by_flag: Mutex<HashMap<FlagId, Vec<FlagRule>>>,
    }
    impl StubFlagRepo {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn with_flag(self: &Arc<Self>, env_id: EnvironmentId, record: FlagRecord) {
            self.flags_by_env
                .lock()
                .unwrap()
                .entry(env_id)
                .or_default()
                .push(record);
        }
    }

    #[async_trait]
    impl FlagRepository for StubFlagRepo {
        async fn find_by_id(&self, _id: FlagId) -> Result<FlagRecord, RepositoryError> {
            unimplemented!()
        }
        async fn find_by_key(
            &self,
            _key: &FlagKey,
            _project_id: ProjectId,
        ) -> Result<FlagRecord, RepositoryError> {
            unimplemented!()
        }
        async fn list_by_project(
            &self,
            _project_id: ProjectId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            unimplemented!()
        }
        async fn list_by_project_paginated(
            &self,
            _project_id: ProjectId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<FlagRecord>, u64), RepositoryError> {
            unimplemented!()
        }
        async fn list_by_project_all(
            &self,
            _project_id: ProjectId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            unimplemented!()
        }
        async fn list_by_environment(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            Ok(self
                .flags_by_env
                .lock()
                .unwrap()
                .get(&env_id)
                .cloned()
                .unwrap_or_default())
        }
        async fn list_by_environment_all(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            unimplemented!()
        }
        async fn create(&self, _record: &FlagRecord) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn update(&self, _record: &FlagRecord) -> Result<FlagRecord, RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: FlagId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn find_hashing_config(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, RepositoryError> {
            Ok(vec![])
        }
        async fn upsert_hashing_config(
            &self,
            _flag_id: FlagId,
            _config: &[stitchd_core::flag::FlagHashingConfig],
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn find_rules(
            &self,
            flag_id: FlagId,
        ) -> Result<Vec<FlagRule>, RepositoryError> {
            Ok(self
                .rules_by_flag
                .lock()
                .unwrap()
                .get(&flag_id)
                .cloned()
                .unwrap_or_default())
        }
        async fn upsert_rules(
            &self,
            _flag_id: FlagId,
            _rules: &[FlagRule],
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
    }

    struct StubVariantRepo;
    #[async_trait]
    impl VariantRepository for StubVariantRepo {
        async fn find_by_flag(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<Variant>, RepositoryError> {
            Ok(vec![])
        }
        async fn create(
            &self,
            _flag_id: FlagId,
            _variant: &Variant,
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn update(&self, _variant: &Variant) -> Result<Variant, RepositoryError> {
            unimplemented!()
        }
        async fn delete(&self, _id: VariantId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn replace_all_for_flag(
            &self,
            _flag_id: FlagId,
            _variants: &[Variant],
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
    }

    #[derive(Default)]
    struct StubSegmentRepo {
        segments_by_env: Mutex<HashMap<EnvironmentId, Vec<Segment>>>,
        rules: Mutex<HashMap<SegmentId, RuleBasedSegment>>,
        lists: Mutex<HashMap<SegmentId, ListBasedSegment>>,
    }
    impl StubSegmentRepo {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn with_segment(self: &Arc<Self>, env_id: EnvironmentId, s: Segment) {
            self.segments_by_env
                .lock()
                .unwrap()
                .entry(env_id)
                .or_default()
                .push(s);
        }
        fn with_list_payload(self: &Arc<Self>, id: SegmentId, lb: ListBasedSegment) {
            self.lists.lock().unwrap().insert(id, lb);
        }
    }

    #[async_trait]
    impl SegmentRepository for StubSegmentRepo {
        async fn find_by_id(
            &self,
            _id: SegmentId,
        ) -> Result<Segment, RepositoryError> {
            unimplemented!()
        }
        async fn find_by_key(
            &self,
            _key: &str,
            _env_id: EnvironmentId,
        ) -> Result<Segment, RepositoryError> {
            unimplemented!()
        }
        async fn find_with_rules(
            &self,
            _id: SegmentId,
        ) -> Result<RuleBasedSegment, RepositoryError> {
            unimplemented!()
        }
        async fn find_with_list(
            &self,
            _id: SegmentId,
        ) -> Result<ListBasedSegment, RepositoryError> {
            unimplemented!()
        }
        async fn list_by_environment(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<Segment>, RepositoryError> {
            Ok(self
                .segments_by_env
                .lock()
                .unwrap()
                .get(&env_id)
                .cloned()
                .unwrap_or_default())
        }
        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<Segment>, u64), RepositoryError> {
            unimplemented!()
        }
        async fn create(&self, _s: &Segment) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn update(&self, _s: &Segment) -> Result<Segment, RepositoryError> {
            unimplemented!()
        }
        async fn upsert_rules(
            &self,
            _id: SegmentId,
            _rules: &[stitchd_core::rule_engine::types::Rule],
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn set_list_entries(
            &self,
            _id: SegmentId,
            _context_type: &str,
            _include: &[String],
            _exclude: &[String],
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn get_condition_expr(
            &self,
            _id: SegmentId,
        ) -> Result<Option<serde_json::Value>, RepositoryError> {
            Ok(None)
        }
        async fn set_condition_expr(
            &self,
            _id: SegmentId,
            _expr: Option<&serde_json::Value>,
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: SegmentId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn check_list_membership(
            &self,
            _env_id: EnvironmentId,
            _context_type: &str,
            _context_key: &str,
            _segment_keys: &[String],
        ) -> Result<HashMap<String, bool>, RepositoryError> {
            unimplemented!()
        }
        async fn batch_check_list_membership(
            &self,
            _env_id: EnvironmentId,
            _contexts: &[(String, String)],
            _segment_keys: &[String],
        ) -> Result<Vec<ContextMembership>, RepositoryError> {
            unimplemented!()
        }
        async fn find_batch_by_ids(
            &self,
            _ids: &[SegmentId],
        ) -> Result<Vec<Segment>, RepositoryError> {
            unimplemented!()
        }
        async fn find_rules_batch(
            &self,
            ids: &[SegmentId],
        ) -> Result<HashMap<SegmentId, RuleBasedSegment>, RepositoryError> {
            let rules = self.rules.lock().unwrap();
            Ok(ids.iter().filter_map(|id| rules.get(id).map(|r| (*id, r.clone()))).collect())
        }
        async fn find_lists_batch(
            &self,
            ids: &[SegmentId],
        ) -> Result<HashMap<SegmentId, ListBasedSegment>, RepositoryError> {
            let lists = self.lists.lock().unwrap();
            Ok(ids.iter().filter_map(|id| lists.get(id).map(|l| (*id, l.clone()))).collect())
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn make_request_with_env(env_id: EnvironmentId) -> Request<SyncDefinitionsRequest> {
        let mut req = Request::new(SyncDefinitionsRequest {});
        req.metadata_mut()
            .insert("x-env-id", env_id.to_string().parse().unwrap());
        req
    }

    fn make_flag_record(_env_id: EnvironmentId, key: &str) -> FlagRecord {
        // FlagRecord is project-scoped; environment is resolved via the
        // repository's list_by_environment query (not a field on the record itself).
        FlagRecord {
            id: FlagId::new(),
            project_id: ProjectId::new(),
            key: FlagKey::new(key).unwrap(),
            name: String::new(),
            description: String::new(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: None,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
        }
    }

    fn make_segment(env_id: EnvironmentId, key: &str, segment_type: SegmentType) -> Segment {
        Segment {
            id: SegmentId::new(),
            environment_id: env_id,
            key: key.to_string(),
            name: key.to_string(),
            description: String::new(),
            tags: vec![],
            segment_type,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
        }
    }

    fn make_service(
        flag_repo: Arc<StubFlagRepo>,
        segment_repo: Arc<StubSegmentRepo>,
    ) -> FlagSdkBackendServiceImpl {
        FlagSdkBackendServiceImpl::new(flag_repo, Arc::new(StubVariantRepo), segment_repo)
    }

    // ── Tests ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn sync_definitions_rejects_missing_env_id_metadata() {
        let svc = make_service(StubFlagRepo::arc(), StubSegmentRepo::arc());
        let req = Request::new(SyncDefinitionsRequest {}); // no x-env-id
        let status = svc.sync_definitions(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
        assert!(status.message().contains("x-env-id"));
    }

    #[tokio::test]
    async fn sync_definitions_rejects_invalid_uuid_in_env_id() {
        let svc = make_service(StubFlagRepo::arc(), StubSegmentRepo::arc());
        let mut req = Request::new(SyncDefinitionsRequest {});
        req.metadata_mut()
            .insert("x-env-id", "not-a-uuid".parse().unwrap());
        let status = svc.sync_definitions(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
        assert!(status.message().contains("valid UUID"));
    }

    #[tokio::test]
    async fn sync_definitions_returns_empty_snapshot_for_unknown_env() {
        let env_id = EnvironmentId::new();
        let svc = make_service(StubFlagRepo::arc(), StubSegmentRepo::arc());
        let resp = svc.sync_definitions(make_request_with_env(env_id)).await.unwrap();
        let resp = resp.into_inner();
        assert_eq!(resp.flags.len(), 0);
        assert_eq!(resp.rule_segments.len(), 0);
        assert_eq!(resp.list_segments.len(), 0);
        assert_eq!(resp.environment_id, env_id.to_string());
        assert!(resp.server_timestamp_ms > 0);
    }

    #[tokio::test]
    async fn sync_definitions_returns_flags_for_environment() {
        let env_id = EnvironmentId::new();
        let flag_repo = StubFlagRepo::arc();
        flag_repo.with_flag(env_id, make_flag_record(env_id, "my-flag"));
        flag_repo.with_flag(env_id, make_flag_record(env_id, "other-flag"));

        let svc = make_service(flag_repo, StubSegmentRepo::arc());
        let resp = svc
            .sync_definitions(make_request_with_env(env_id))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.flags.len(), 2);
        let keys: Vec<_> = resp.flags.iter().map(|f| f.key.as_str()).collect();
        assert!(keys.contains(&"my-flag"));
        assert!(keys.contains(&"other-flag"));
    }

    #[tokio::test]
    async fn sync_definitions_partitions_rule_and_list_segments() {
        let env_id = EnvironmentId::new();
        let segment_repo = StubSegmentRepo::arc();

        let rule_seg = make_segment(env_id, "pro-users", SegmentType::Rule);
        let list_seg = make_segment(env_id, "early-access", SegmentType::List);
        let list_seg_id = list_seg.id;
        segment_repo.with_segment(env_id, rule_seg);
        segment_repo.with_segment(env_id, list_seg);

        // Seed a list-based payload spanning two context types.
        let mut lists = HashMap::new();
        let mut user_include = std::collections::HashSet::new();
        user_include.insert("alice".to_string());
        lists.insert(
            "user".to_string(),
            ContextList { include: user_include, exclude: std::collections::HashSet::new() },
        );
        let mut org_include = std::collections::HashSet::new();
        org_include.insert("acme".to_string());
        lists.insert(
            "org".to_string(),
            ContextList { include: org_include, exclude: std::collections::HashSet::new() },
        );
        segment_repo.with_list_payload(list_seg_id, ListBasedSegment { id: list_seg_id, lists });

        let svc = make_service(StubFlagRepo::arc(), segment_repo);
        let resp = svc
            .sync_definitions(make_request_with_env(env_id))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.rule_segments.len(), 1);
        assert_eq!(resp.rule_segments[0].key, "pro-users");

        // Multi-context-type list segment becomes two ListSegmentMeta entries,
        // one per context type, sorted alphabetically.
        assert_eq!(resp.list_segments.len(), 2);
        let ctx_types: Vec<_> = resp.list_segments.iter().map(|m| m.context_type.as_str()).collect();
        assert_eq!(ctx_types, vec!["org", "user"]);
        // Both share the same segment id (the dedup-by-(id,context_type) invariant).
        assert_eq!(resp.list_segments[0].id, resp.list_segments[1].id);
    }

    #[tokio::test]
    async fn sync_definitions_isolates_environments() {
        // Flag in env A must not appear in env B's snapshot.
        let env_a = EnvironmentId::new();
        let env_b = EnvironmentId::new();
        let flag_repo = StubFlagRepo::arc();
        flag_repo.with_flag(env_a, make_flag_record(env_a, "env-a-flag"));

        let svc = make_service(flag_repo, StubSegmentRepo::arc());

        let resp_a = svc
            .sync_definitions(make_request_with_env(env_a))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp_a.flags.len(), 1);

        let resp_b = svc
            .sync_definitions(make_request_with_env(env_b))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp_b.flags.len(), 0);
    }

    #[tokio::test]
    async fn ingest_sdk_eval_log_accepts_empty_batch() {
        // Placeholder behaviour for Phase 3 Task 2 (real wiring lands in Task 3).
        let svc = make_service(StubFlagRepo::arc(), StubSegmentRepo::arc());
        let env_id = EnvironmentId::new();
        let mut req = Request::new(IngestSdkEvalLogRequest { events: vec![] });
        req.metadata_mut()
            .insert("x-env-id", env_id.to_string().parse().unwrap());
        let resp = svc.ingest_sdk_eval_log(req).await.unwrap();
        let _ = resp.into_inner();
    }

    #[tokio::test]
    async fn ingest_sdk_eval_log_rejects_missing_env_id() {
        let svc = make_service(StubFlagRepo::arc(), StubSegmentRepo::arc());
        let req = Request::new(IngestSdkEvalLogRequest { events: vec![] });
        let status = svc.ingest_sdk_eval_log(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    // Silence unused-import warnings on test-only types.
    fn _unused_test_imports() {
        let _: Option<Context> = None;
    }
}
