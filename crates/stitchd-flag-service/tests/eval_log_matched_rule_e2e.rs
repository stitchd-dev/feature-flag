//! Phase 2 Task 2.3: Integration test asserting eval-log `matched_rule_id`
//! shape end-to-end through the SDK ingest path.
//!
//! Drives `FlagSdkBackendServiceImpl::ingest_sdk_eval_log` in-process
//! (no gRPC server) against a real ClickHouse instance, then verifies the
//! resulting rows in `flag_evaluation_log` carry the expected
//! `(targeting_on, matched_rule_id)` tuple for the three canonical scenarios:
//!   1. Custom rule matched           → targeting_on = true,  matched_rule_id = <uuid>
//!   2. Default-rule fall-through     → targeting_on = true,  matched_rule_id IS NULL
//!   3. Disabled flag (targeting off) → targeting_on = false, matched_rule_id IS NULL
//!
//! Gated on `STITCHD_CLICKHOUSE_URL`: if absent, the test logs a skip and
//! returns Ok — matches the existing `eval_preview_clickhouse.rs` pattern.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use clickhouse::Client as ChClient;
use tonic::Request;

use stitchd_core::event::EventDefinition;
use stitchd_core::flag::{FlagHashingConfig, FlagRecord, FlagRule, Variant};
use stitchd_core::id::{
    EnvironmentId, EventDefinitionId, FlagId, FlagKey, ProjectId, SegmentId, VariantId,
};
use stitchd_core::rule_engine::types::Rule;
use stitchd_core::segment::{RuleBasedSegment, Segment};
use stitchd_db::{
    ContextMembership, EventDefinitionRepository, FlagRepository, ListSegmentSummary,
    RepositoryError, SegmentIdMembership, SegmentRepository, VariantRepository,
};
use stitchd_flag_service::sdk_backend::FlagSdkBackendServiceImpl;
use stitchd_proto::sdk::v1::{
    FlagEvaluationEvent, IngestSdkEvalLogRequest, flag_sdk_backend_service_server::FlagSdkBackendService,
};

// ── ClickHouse client + skip-gate helper ────────────────────────────────────

fn ch_client() -> Option<Arc<ChClient>> {
    let url = std::env::var("STITCHD_CLICKHOUSE_URL").ok()?;
    let db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let user = std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_string());
    let password = std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_default();
    Some(Arc::new(
        ChClient::default()
            .with_url(url)
            .with_database(db)
            .with_user(user)
            .with_password(password),
    ))
}

// ── Minimal stub repositories (only `new()`-construction surface is exercised) ──

#[derive(Default)]
struct StubFlagRepo;

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
        _env_id: EnvironmentId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        Ok(vec![])
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
    ) -> Result<Vec<FlagHashingConfig>, RepositoryError> {
        Ok(vec![])
    }
    async fn upsert_hashing_config(
        &self,
        _flag_id: FlagId,
        _config: &[FlagHashingConfig],
    ) -> Result<(), RepositoryError> {
        unimplemented!()
    }
    async fn find_rules(&self, _flag_id: FlagId) -> Result<Vec<FlagRule>, RepositoryError> {
        Ok(vec![])
    }
    async fn upsert_rules(
        &self,
        _flag_id: FlagId,
        _rules: &[FlagRule],
    ) -> Result<(), RepositoryError> {
        unimplemented!()
    }
}

#[derive(Default)]
struct StubVariantRepo;

#[async_trait]
impl VariantRepository for StubVariantRepo {
    async fn find_by_flag(&self, _flag_id: FlagId) -> Result<Vec<Variant>, RepositoryError> {
        Ok(vec![])
    }
    async fn create(&self, _flag_id: FlagId, _v: &Variant) -> Result<(), RepositoryError> {
        unimplemented!()
    }
    async fn update(&self, _v: &Variant) -> Result<Variant, RepositoryError> {
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
struct StubSegmentRepo;

#[async_trait]
impl SegmentRepository for StubSegmentRepo {
    async fn find_by_id(&self, _id: SegmentId) -> Result<Segment, RepositoryError> {
        unimplemented!()
    }
    async fn find_by_key(
        &self,
        _key: &str,
        _env_id: EnvironmentId,
    ) -> Result<Segment, RepositoryError> {
        unimplemented!()
    }
    async fn find_with_rules(&self, _id: SegmentId) -> Result<RuleBasedSegment, RepositoryError> {
        unimplemented!()
    }
    async fn list_by_environment(
        &self,
        _env_id: EnvironmentId,
    ) -> Result<Vec<Segment>, RepositoryError> {
        Ok(vec![])
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
        _rules: &[Rule],
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
    ) -> Result<std::collections::HashMap<String, bool>, RepositoryError> {
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
        _ids: &[SegmentId],
    ) -> Result<std::collections::HashMap<SegmentId, RuleBasedSegment>, RepositoryError> {
        Ok(Default::default())
    }
    async fn add_entries(
        &self,
        _id: SegmentId,
        _ctx: &str,
        _lt: &str,
        _keys: &[String],
    ) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn remove_entries(
        &self,
        _id: SegmentId,
        _ctx: &str,
        _lt: &str,
        _keys: &[String],
    ) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn get_list_segment_summary(
        &self,
        _id: SegmentId,
    ) -> Result<ListSegmentSummary, RepositoryError> {
        Ok(ListSegmentSummary::default())
    }
    async fn find_memberships_batch(
        &self,
        _env_id: EnvironmentId,
        _contexts: &[(String, String)],
        _segment_ids: &[SegmentId],
    ) -> Result<Vec<SegmentIdMembership>, RepositoryError> {
        unimplemented!()
    }
}

#[derive(Default)]
struct StubEventDefRepo;

#[async_trait]
impl EventDefinitionRepository for StubEventDefRepo {
    async fn find_by_id(
        &self,
        _id: EventDefinitionId,
    ) -> Result<EventDefinition, RepositoryError> {
        unimplemented!()
    }
    async fn find_by_key(
        &self,
        _key: &str,
        _env_id: EnvironmentId,
    ) -> Result<EventDefinition, RepositoryError> {
        unimplemented!()
    }
    async fn list_by_environment(
        &self,
        _env_id: EnvironmentId,
    ) -> Result<Vec<EventDefinition>, RepositoryError> {
        Ok(vec![])
    }
    async fn list_by_environment_paginated(
        &self,
        _env_id: EnvironmentId,
        _offset: u64,
        _limit: u64,
        _include_archived: bool,
    ) -> Result<(Vec<EventDefinition>, u64), RepositoryError> {
        unimplemented!()
    }
    async fn create(&self, _d: &EventDefinition) -> Result<(), RepositoryError> {
        unimplemented!()
    }
    async fn update(&self, _d: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
        unimplemented!()
    }
    async fn soft_delete(&self, _id: EventDefinitionId) -> Result<(), RepositoryError> {
        unimplemented!()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn make_service(ch: Arc<ChClient>) -> FlagSdkBackendServiceImpl {
    FlagSdkBackendServiceImpl::new(
        Arc::new(StubFlagRepo),
        Arc::new(StubVariantRepo),
        Arc::new(StubSegmentRepo),
    )
    .with_event_definition_repo(Arc::new(StubEventDefRepo))
    .with_clickhouse(ch)
}

fn make_event(
    flag_id: uuid::Uuid,
    flag_key: &str,
    variant_key: &str,
    outcome: &str,
    matched_rule_id: &str,
    context_key: &str,
) -> FlagEvaluationEvent {
    FlagEvaluationEvent {
        flag_key: flag_key.to_string(),
        flag_id: flag_id.to_string(),
        variant_key: variant_key.to_string(),
        context_type: "user".to_string(),
        context_key: context_key.to_string(),
        evaluated_at: Utc::now().to_rfc3339(),
        matched_rule_id: matched_rule_id.to_string(),
        outcome: outcome.to_string(),
        reasoning_included: false,
        context_parameters: std::collections::HashMap::new(),
        environment_id: String::new(),
    }
}

// ── Row type for assertion reads ────────────────────────────────────────────

#[derive(clickhouse::Row, serde::Deserialize, Debug)]
struct AssertRow {
    targeting_on: bool,
    context_key: String,
    #[serde(with = "clickhouse::serde::uuid::option")]
    matched_rule_id: Option<uuid::Uuid>,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn eval_log_matched_rule_three_scenarios_end_to_end() {
    let Some(ch) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };

    let env_id = EnvironmentId::new();
    let flag_id = uuid::Uuid::new_v4();
    // Use a flag_key unique to this test run so the SELECT-by-flag_key below
    // doesn't see noise from concurrent test runs.
    let flag_key = format!("e2e-matched-rule-{}", uuid::Uuid::new_v4());

    let rule_id = uuid::Uuid::new_v4();

    // Three SDK events spanning the three (targeting_on, matched_rule_id) shapes:
    let events = vec![
        // Scenario 1: custom rule matched.
        make_event(
            flag_id,
            &flag_key,
            "treatment",
            "matched",
            &rule_id.to_string(),
            "alice",
        ),
        // Scenario 2: targeting on, no custom rule matched (default-rule path).
        make_event(flag_id, &flag_key, "control", "default_rule", "", "bob"),
        // Scenario 3: disabled flag.
        make_event(
            flag_id,
            &flag_key,
            "__disabled__",
            "disabled",
            "",
            "carol",
        ),
    ];

    let svc = make_service(Arc::clone(&ch));

    let mut req = Request::new(IngestSdkEvalLogRequest { events });
    req.metadata_mut()
        .insert("x-env-id", env_id.to_string().parse().unwrap());
    svc.ingest_sdk_eval_log(req)
        .await
        .expect("ingest_sdk_eval_log");

    // Give CH a moment to materialise the inserts.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let rows: Vec<AssertRow> = ch
        .query(
            "SELECT targeting_on, context_key, matched_rule_id \
             FROM flag_evaluation_log \
             WHERE flag_key = ? \
             ORDER BY context_key",
        )
        .bind(&flag_key)
        .fetch_all()
        .await
        .expect("fetch test rows");

    assert_eq!(
        rows.len(),
        3,
        "expected 3 rows for this run, got {}",
        rows.len()
    );

    // Rows are ordered by context_key ASC → alice, bob, carol.
    let alice = rows.iter().find(|r| r.context_key == "alice").unwrap();
    let bob = rows.iter().find(|r| r.context_key == "bob").unwrap();
    let carol = rows.iter().find(|r| r.context_key == "carol").unwrap();

    // Scenario 1: rule matched
    assert!(alice.targeting_on, "alice (matched) must have targeting_on=true");
    assert_eq!(
        alice.matched_rule_id,
        Some(rule_id),
        "alice's matched_rule_id must be the rule UUID"
    );

    // Scenario 2: default-rule fall-through
    assert!(bob.targeting_on, "bob (default_rule) must have targeting_on=true");
    assert!(
        bob.matched_rule_id.is_none(),
        "bob's matched_rule_id must be NULL for default-rule fall-through"
    );

    // Scenario 3: disabled flag
    assert!(
        !carol.targeting_on,
        "carol (disabled) must have targeting_on=false"
    );
    assert!(
        carol.matched_rule_id.is_none(),
        "carol's matched_rule_id must be NULL for disabled flag"
    );

    println!(
        "OK: 3 scenarios verified — alice=matched, bob=default_rule, carol=disabled"
    );
}
