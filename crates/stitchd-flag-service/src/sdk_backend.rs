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

use chrono::{DateTime, Utc};
use clickhouse::Client as ChClient;
use tonic::{Request, Response, Status};

use stitchd_core::id::EnvironmentId;
use stitchd_core::segment::SegmentType;
use stitchd_db::clickhouse::{EvalLogRow, insert_eval_log_rows};
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
    /// Optional ClickHouse client for `IngestSdkEvalLog`. When `None`, events
    /// are accepted (200 OK) but discarded — useful for local dev without CH.
    ch_client: Option<Arc<ChClient>>,
}

impl FlagSdkBackendServiceImpl {
    /// Construct a new service backed by the given repositories.
    ///
    /// `IngestSdkEvalLog` is functional but discards events until
    /// [`with_clickhouse`] is called.
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
            ch_client: None,
        }
    }

    /// Attach a ClickHouse client so `IngestSdkEvalLog` writes events to the
    /// `flag_evaluation_log` table.
    #[must_use]
    pub fn with_clickhouse(mut self, client: Arc<ChClient>) -> Self {
        self.ch_client = Some(client);
        self
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
        // List segment defs not returned until Phase 4 Scylla membership reads.
        let _ = list_ids;

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

        // 6. Build proto list-segment metadata. List payloads are empty until Phase 4
        //    Scylla membership reads are wired. For now emit one entry per segment
        //    with an empty context_type (pristine/pending state).
        let mut list_segments = Vec::new();
        for s in &list_segment_records {
            list_segments.push(stitchd_proto::segments::v1::ListSegmentMeta {
                id: s.id.to_string(),
                key: s.key.clone(),
                context_type: String::new(),
            });
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
        let env_id = env_id_from_metadata(&request)?;
        let events = request.into_inner().events;
        let received = events.len();

        // Convert SDK events → EvalLogRows. Events with unparseable flag_id are
        // SKIPPED with a warning (data quality issue: SDK should always send a
        // valid UUID except for outcome=flag_not_found, which we also skip
        // because there's no flag record to FK against).
        let mut rows = Vec::with_capacity(events.len());
        let mut skipped_unparseable = 0_usize;
        let mut skipped_flag_not_found = 0_usize;
        for ev in events {
            if ev.outcome == "flag_not_found" {
                skipped_flag_not_found += 1;
                continue;
            }
            match event_to_row(ev, env_id) {
                Ok(row) => rows.push(row),
                Err(reason) => {
                    skipped_unparseable += 1;
                    tracing::warn!(reason = %reason, "skipping malformed SDK eval event");
                }
            }
        }

        if skipped_unparseable > 0 || skipped_flag_not_found > 0 {
            tracing::debug!(
                received,
                accepted = rows.len(),
                skipped_unparseable,
                skipped_flag_not_found,
                "ingest_sdk_eval_log batch summary"
            );
        }

        // If we have no ClickHouse client, accept the batch but discard. This
        // is the "wiring not configured" path — useful for local dev. The
        // gateway forwards events here regardless.
        let Some(client) = self.ch_client.as_ref() else {
            tracing::warn!(
                received,
                "ingest_sdk_eval_log: no clickhouse client wired; events discarded"
            );
            return Ok(Response::new(IngestSdkEvalLogResponse {}));
        };

        if let Err(e) = insert_eval_log_rows(client, &rows).await {
            // Don't surface the error to the SDK — the gateway has already
            // returned 202 Accepted by this point in the SDK's view. Log and
            // count.
            tracing::error!(
                error = %e,
                row_count = rows.len(),
                "ingest_sdk_eval_log: clickhouse insert failed"
            );
            return Err(Status::internal(format!(
                "eval log insert failed: {e}"
            )));
        }

        Ok(Response::new(IngestSdkEvalLogResponse {}))
    }
}

/// Map a proto `ParameterValue` (which doesn't impl Serialize) onto a
/// `serde_json::Value` for embedding in `params_json`. Unknown/empty variants
/// become `Null`.
fn param_value_to_json(pv: stitchd_proto::common::v1::ParameterValue) -> serde_json::Value {
    use stitchd_proto::common::v1::parameter_value::Value;
    match pv.value {
        Some(Value::IntValue(i)) => serde_json::Value::from(i),
        Some(Value::DoubleValue(d)) => {
            serde_json::Number::from_f64(d).map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        Some(Value::StringValue(s)) => serde_json::Value::String(s),
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(b),
        Some(Value::SemverValue(s)) => serde_json::Value::String(s),
        None => serde_json::Value::Null,
    }
}

/// Convert a single SDK `FlagEvaluationEvent` into an `EvalLogRow`. Returns
/// `Err(reason)` if any required field can't be parsed.
fn event_to_row(ev: FlagEvaluationEvent, env_id: EnvironmentId) -> Result<EvalLogRow, String> {
    let flag_id = uuid::Uuid::parse_str(&ev.flag_id)
        .map_err(|_| format!("flag_id is not a valid UUID: {:?}", ev.flag_id))?;
    let evaluated_at: DateTime<Utc> = ev
        .evaluated_at
        .parse::<DateTime<chrono::FixedOffset>>()
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| format!("evaluated_at is not a valid RFC3339 timestamp: {:?}", ev.evaluated_at))?;
    // params_json: serialise non-private parameters; private ones are already
    // stripped by the SDK before sending (per sdks/spec/docs/05-events.md).
    // The proto's ParameterValue doesn't impl Serialize directly (generated
    // code), so we convert through serde_json::Value manually.
    let params_json = if ev.context_parameters.is_empty() {
        "{}".to_string()
    } else {
        let mut obj = serde_json::Map::with_capacity(ev.context_parameters.len());
        for (k, v) in ev.context_parameters {
            obj.insert(k, param_value_to_json(v));
        }
        serde_json::to_string(&serde_json::Value::Object(obj))
            .unwrap_or_else(|_| "{}".to_string())
    };
    Ok(EvalLogRow {
        env_id: env_id.as_uuid(),
        flag_id,
        flag_key: ev.flag_key,
        variant_key: ev.variant_key,
        is_disabled: ev.outcome == "disabled",
        evaluated_at,
        context_type: ev.context_type,
        context_key: ev.context_key,
        params_json,
    })
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
        RuleBasedSegment, Segment, SegmentType,
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
        async fn add_entries(&self, _id: SegmentId, _ctx: &str, _lt: &str, _keys: &[String]) -> Result<(), RepositoryError> { Ok(()) }
        async fn remove_entries(&self, _id: SegmentId, _ctx: &str, _lt: &str, _keys: &[String]) -> Result<(), RepositoryError> { Ok(()) }
        async fn get_list_segment_summary(&self, _id: SegmentId) -> Result<stitchd_db::ListSegmentSummary, RepositoryError> { Ok(stitchd_db::ListSegmentSummary::default()) }
        async fn find_memberships_batch(
            &self,
            _env_id: EnvironmentId,
            contexts: &[(String, String)],
            segment_ids: &[SegmentId],
        ) -> Result<Vec<stitchd_db::SegmentIdMembership>, RepositoryError> {
            Ok(contexts
                .iter()
                .map(|(t, k)| stitchd_db::SegmentIdMembership {
                    context_type: t.clone(),
                    context_key: k.clone(),
                    memberships: segment_ids.iter().map(|id| (*id, false)).collect(),
                })
                .collect())
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
        segment_repo.with_segment(env_id, rule_seg);
        segment_repo.with_segment(env_id, list_seg);

        // NOTE: list_payloads is empty until Phase 4 Scylla wiring.
        // List segments appear with empty context_type (pristine state).

        let svc = make_service(StubFlagRepo::arc(), segment_repo);
        let resp = svc
            .sync_definitions(make_request_with_env(env_id))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.rule_segments.len(), 1);
        assert_eq!(resp.rule_segments[0].key, "pro-users");

        // List segment appears once with empty context_type (pending Phase 4 Scylla wiring).
        assert_eq!(resp.list_segments.len(), 1);
        assert_eq!(resp.list_segments[0].context_type, "");
        assert_eq!(resp.list_segments[0].key, "early-access");
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
    async fn ingest_sdk_eval_log_accepts_empty_batch_without_ch_client() {
        // No ch_client wired → events discarded but request still succeeds.
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

    #[tokio::test]
    async fn ingest_sdk_eval_log_discards_events_when_no_ch_client_wired() {
        // Even with valid events, when ch_client is None we return success
        // and just log a warning. Verifies we don't accidentally error on the
        // discard-events code path.
        let svc = make_service(StubFlagRepo::arc(), StubSegmentRepo::arc());
        let env_id = EnvironmentId::new();
        let event = FlagEvaluationEvent {
            flag_key: "f".into(),
            flag_id: uuid::Uuid::new_v4().to_string(),
            variant_key: "on".into(),
            context_type: "user".into(),
            context_key: "alice".into(),
            evaluated_at: "2026-05-16T18:00:00.000Z".into(),
            matched_rule_id: String::new(),
            outcome: "default_rule".into(),
            reasoning_included: false,
            context_parameters: std::collections::HashMap::new(),
            environment_id: String::new(),
        };
        let mut req = Request::new(IngestSdkEvalLogRequest { events: vec![event] });
        req.metadata_mut()
            .insert("x-env-id", env_id.to_string().parse().unwrap());
        let _ = svc.ingest_sdk_eval_log(req).await.unwrap();
    }

    // ── event_to_row unit tests ─────────────────────────────────────────────

    #[test]
    fn event_to_row_happy_path() {
        let env_id = EnvironmentId::new();
        let flag_uuid = uuid::Uuid::new_v4();
        let event = FlagEvaluationEvent {
            flag_key: "checkout-flow".into(),
            flag_id: flag_uuid.to_string(),
            variant_key: "treatment".into(),
            context_type: "user".into(),
            context_key: "alice".into(),
            evaluated_at: "2026-05-16T18:32:14.123Z".into(),
            matched_rule_id: String::new(),
            outcome: "matched".into(),
            reasoning_included: false,
            context_parameters: std::collections::HashMap::new(),
            environment_id: String::new(),
        };
        let row = event_to_row(event, env_id).unwrap();
        assert_eq!(row.env_id, env_id.as_uuid());
        assert_eq!(row.flag_id, flag_uuid);
        assert_eq!(row.flag_key, "checkout-flow");
        assert_eq!(row.variant_key, "treatment");
        assert!(!row.is_disabled);
        assert_eq!(row.context_type, "user");
        assert_eq!(row.context_key, "alice");
        assert_eq!(row.params_json, "{}");
    }

    #[test]
    fn event_to_row_marks_disabled_outcome() {
        let env_id = EnvironmentId::new();
        let mut event = FlagEvaluationEvent {
            flag_key: "f".into(),
            flag_id: uuid::Uuid::new_v4().to_string(),
            variant_key: "off".into(),
            context_type: "u".into(),
            context_key: "k".into(),
            evaluated_at: "2026-05-16T18:00:00Z".into(),
            matched_rule_id: String::new(),
            outcome: "disabled".into(),
            reasoning_included: false,
            context_parameters: std::collections::HashMap::new(),
            environment_id: String::new(),
        };
        let row = event_to_row(event.clone(), env_id).unwrap();
        assert!(row.is_disabled);
        // Compare with non-disabled outcome.
        event.outcome = "matched".into();
        let row2 = event_to_row(event, env_id).unwrap();
        assert!(!row2.is_disabled);
    }

    #[test]
    fn event_to_row_rejects_invalid_flag_id() {
        let env_id = EnvironmentId::new();
        let event = FlagEvaluationEvent {
            flag_key: "f".into(),
            flag_id: "not-a-uuid".into(),
            variant_key: "v".into(),
            context_type: "u".into(),
            context_key: "k".into(),
            evaluated_at: "2026-05-16T18:00:00Z".into(),
            matched_rule_id: String::new(),
            outcome: "matched".into(),
            reasoning_included: false,
            context_parameters: std::collections::HashMap::new(),
            environment_id: String::new(),
        };
        let err = event_to_row(event, env_id).unwrap_err();
        assert!(err.contains("flag_id"));
    }

    #[test]
    fn event_to_row_rejects_invalid_evaluated_at() {
        let env_id = EnvironmentId::new();
        let event = FlagEvaluationEvent {
            flag_key: "f".into(),
            flag_id: uuid::Uuid::new_v4().to_string(),
            variant_key: "v".into(),
            context_type: "u".into(),
            context_key: "k".into(),
            evaluated_at: "yesterday".into(),
            matched_rule_id: String::new(),
            outcome: "matched".into(),
            reasoning_included: false,
            context_parameters: std::collections::HashMap::new(),
            environment_id: String::new(),
        };
        let err = event_to_row(event, env_id).unwrap_err();
        assert!(err.contains("evaluated_at"));
    }

    // Silence unused-import warnings on test-only types.
    fn _unused_test_imports() {
        let _: Option<Context> = None;
    }
}
