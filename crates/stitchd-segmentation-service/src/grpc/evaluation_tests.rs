//! Failing tests for rule-based segment evaluation handler (TDD Red phase).
//!
//! These tests exercise `evaluate_rule_membership` via the public
//! `SegmentationServiceImpl::evaluate_membership` gRPC method using an
//! in-memory mock repository.

/// Expose the mock repo for use in sibling test modules.
#[cfg(test)]
#[allow(missing_docs)]
pub mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tonic::Request;

    use stitchd_core::{
        context::ParameterValue,
        id::{EnvironmentId, RuleId, SegmentId, VariantId},
        rule_engine::{
            condition::Condition,
            types::{ConditionExpr, Rule, RuleOutput},
        },
        segment::{ContextList, ListBasedSegment, RuleBasedSegment, Segment, SegmentType},
    };
    use stitchd_db::{RepositoryError, SegmentRepository};
    use stitchd_proto::segments::v1::{
        ActivateListGenerationRequest, EvaluateMembershipRequest,
        segmentation_service_server::SegmentationService,
    };

    use crate::grpc::service::{AppState, SegmentationServiceImpl};

    // -------------------------------------------------------------------------
    // Minimal mock repository (public so crud_tests can reuse it)
    // -------------------------------------------------------------------------

    /// A minimal in-memory segment repository for unit testing.
    pub struct MockSegmentRepoForTest {
        pub segments: Mutex<HashMap<(EnvironmentId, String), Segment>>,
        pub rule_defs: Mutex<HashMap<SegmentId, RuleBasedSegment>>,
        pub list_defs: Mutex<HashMap<SegmentId, ListBasedSegment>>,
        pub list_memberships: Mutex<HashMap<(EnvironmentId, String, String, String), bool>>,
    }

    impl Default for MockSegmentRepoForTest {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockSegmentRepoForTest {
        #[must_use]
        pub fn new() -> Self {
            Self {
                segments: Mutex::new(HashMap::new()),
                rule_defs: Mutex::new(HashMap::new()),
                list_defs: Mutex::new(HashMap::new()),
                list_memberships: Mutex::new(HashMap::new()),
            }
        }

        /// # Panics
        ///
        /// Panics if the internal mutex is poisoned.
        pub fn insert_rule_segment(
            &self,
            env_id: EnvironmentId,
            key: &str,
            rules: Vec<Rule>,
        ) -> Segment {
            let seg_id = SegmentId::new();
            let seg = Segment {
                id: seg_id,
                environment_id: env_id,
                key: key.to_string(),
                segment_type: SegmentType::Rule,
                name: String::new(),
                description: String::new(),
                tags: vec![],
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                deleted_at: None,
                version: 1,
            };
            let def = RuleBasedSegment { id: seg_id, rules };
            self.segments
                .lock()
                .unwrap()
                .insert((env_id, key.to_string()), seg.clone());
            self.rule_defs.lock().unwrap().insert(seg_id, def);
            seg
        }

        /// # Panics
        ///
        /// Panics if the internal mutex is poisoned.
        pub fn insert_list_segment(
            &self,
            env_id: EnvironmentId,
            key: &str,
            lists: HashMap<String, ContextList>,
        ) -> Segment {
            let seg_id = SegmentId::new();
            let seg = Segment {
                id: seg_id,
                environment_id: env_id,
                key: key.to_string(),
                segment_type: SegmentType::List,
                name: String::new(),
                description: String::new(),
                tags: vec![],
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                deleted_at: None,
                version: 1,
            };
            let def = ListBasedSegment { id: seg_id, lists };
            self.segments
                .lock()
                .unwrap()
                .insert((env_id, key.to_string()), seg.clone());
            self.list_defs.lock().unwrap().insert(seg_id, def);
            seg
        }

        /// # Panics
        ///
        /// Panics if the internal mutex is poisoned.
        #[allow(dead_code)]
        pub fn insert_list_membership_entry(
            &self,
            env_id: EnvironmentId,
            segment_key: &str,
            context_type: &str,
            context_key: &str,
            is_member: bool,
        ) {
            self.list_memberships.lock().unwrap().insert(
                (
                    env_id,
                    segment_key.to_string(),
                    context_type.to_string(),
                    context_key.to_string(),
                ),
                is_member,
            );
        }
    }

    #[async_trait]
    impl SegmentRepository for MockSegmentRepoForTest {
        async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
            self.segments
                .lock()
                .unwrap()
                .values()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &str,
            environment_id: EnvironmentId,
        ) -> Result<Segment, RepositoryError> {
            self.segments
                .lock()
                .unwrap()
                .get(&(environment_id, key.to_string()))
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound {
                    id: format!("{environment_id}/{key}"),
                })
        }

        async fn list_by_environment(
            &self,
            environment_id: EnvironmentId,
        ) -> Result<Vec<Segment>, RepositoryError> {
            Ok(self
                .segments
                .lock()
                .unwrap()
                .values()
                .filter(|s| s.environment_id == environment_id)
                .cloned()
                .collect())
        }

        async fn list_by_environment_paginated(
            &self,
            environment_id: EnvironmentId,
            offset: u64,
            limit: u64,
        ) -> Result<(Vec<Segment>, u64), RepositoryError> {
            let all: Vec<Segment> = self
                .segments
                .lock()
                .unwrap()
                .values()
                .filter(|s| s.environment_id == environment_id)
                .cloned()
                .collect();
            let total = all.len() as u64;
            let page = all
                .into_iter()
                .skip(usize::try_from(offset).unwrap_or(usize::MAX))
                .take(usize::try_from(limit).unwrap_or(usize::MAX))
                .collect();
            Ok((page, total))
        }

        async fn create(&self, segment: &Segment) -> Result<(), RepositoryError> {
            self.segments.lock().unwrap().insert(
                (segment.environment_id, segment.key.clone()),
                segment.clone(),
            );
            Ok(())
        }

        async fn update(&self, segment: &Segment) -> Result<Segment, RepositoryError> {
            let key = (segment.environment_id, segment.key.clone());
            let exists = self.segments.lock().unwrap().contains_key(&key);
            if !exists {
                return Err(RepositoryError::NotFound {
                    id: segment.id.to_string(),
                });
            }
            self.segments.lock().unwrap().insert(key, segment.clone());
            Ok(segment.clone())
        }

        async fn find_with_rules(
            &self,
            id: SegmentId,
        ) -> Result<RuleBasedSegment, RepositoryError> {
            self.rule_defs
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })
        }

        async fn set_list_entries(
            &self,
            id: SegmentId,
            context_type: &str,
            include: &[String],
            exclude: &[String],
        ) -> Result<(), RepositoryError> {
            let mut defs = self.list_defs.lock().unwrap();
            let def = defs.entry(id).or_insert_with(|| ListBasedSegment {
                id,
                lists: HashMap::new(),
            });
            def.lists.insert(
                context_type.to_string(),
                ContextList {
                    include: include.iter().cloned().collect(),
                    exclude: exclude.iter().cloned().collect(),
                },
            );
            drop(defs);
            Ok(())
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
        ) -> Result<stitchd_db::ListSegmentSummary, RepositoryError> {
            Ok(stitchd_db::ListSegmentSummary::default())
        }

        async fn soft_delete(&self, id: SegmentId) -> Result<(), RepositoryError> {
            let mut segs = self.segments.lock().unwrap();
            let seg = segs
                .values_mut()
                .find(|s| s.id == id)
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;
            seg.deleted_at = Some(chrono::Utc::now());
            drop(segs);
            Ok(())
        }

        async fn check_list_membership(
            &self,
            environment_id: EnvironmentId,
            context_type: &str,
            context_key: &str,
            segment_keys: &[String],
        ) -> Result<HashMap<String, bool>, RepositoryError> {
            let memberships = self.list_memberships.lock().unwrap();
            let segs = self.segments.lock().unwrap();
            let list_defs = self.list_defs.lock().unwrap();

            let result = segment_keys
                .iter()
                .map(|seg_key| {
                    // First check the explicit membership map.
                    let explicit_key = (
                        environment_id,
                        seg_key.clone(),
                        context_type.to_string(),
                        context_key.to_string(),
                    );
                    if let Some(&val) = memberships.get(&explicit_key) {
                        return (seg_key.clone(), val);
                    }

                    // Fall back to evaluating the in-memory list definition.
                    let is_member = segs
                        .get(&(environment_id, seg_key.clone()))
                        .and_then(|seg| list_defs.get(&seg.id))
                        .is_some_and(|def| {
                            def.lists.get(context_type).is_some_and(|ctx_list| {
                                // Exclude takes precedence over include.
                                if ctx_list.exclude.contains(context_key) {
                                    false
                                } else {
                                    ctx_list.include.contains(context_key)
                                }
                            })
                        });

                    (seg_key.clone(), is_member)
                })
                .collect();
            Ok(result)
        }

        async fn lookup_entry_raw(
            &self,
            id: SegmentId,
            context_type: &str,
            key: &str,
        ) -> Result<(bool, bool), RepositoryError> {
            // include and exclude are reported independently (exclude does not
            // override include here) — mirrors the PG repo contract so the
            // admin UI can show the full picture.
            let defs = self.list_defs.lock().unwrap();
            let Some(def) = defs.get(&id) else {
                return Err(RepositoryError::NotFound { id: id.to_string() });
            };
            let flags = def.lists.get(context_type).map_or((false, false), |cl| {
                (cl.include.contains(key), cl.exclude.contains(key))
            });
            drop(defs);
            Ok(flags)
        }

        async fn batch_check_list_membership(
            &self,
            _environment_id: EnvironmentId,
            _contexts: &[(String, String)],
            _segment_keys: &[String],
        ) -> Result<Vec<stitchd_db::ContextMembership>, RepositoryError> {
            Ok(vec![])
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

        async fn find_batch_by_ids(
            &self,
            ids: &[SegmentId],
        ) -> Result<Vec<Segment>, RepositoryError> {
            let result = self
                .segments
                .lock()
                .unwrap()
                .values()
                .filter(|s| ids.contains(&s.id))
                .cloned()
                .collect();
            Ok(result)
        }

        async fn find_rules_batch(
            &self,
            ids: &[SegmentId],
        ) -> Result<std::collections::HashMap<SegmentId, RuleBasedSegment>, RepositoryError>
        {
            let defs = self.rule_defs.lock().unwrap();
            let result = ids
                .iter()
                .filter_map(|id| defs.get(id).map(|d| (*id, d.clone())))
                .collect();
            Ok(result)
        }

        async fn find_memberships_batch(
            &self,
            _env_id: stitchd_core::id::EnvironmentId,
            _contexts: &[(String, String)],
            _ids: &[SegmentId],
        ) -> Result<Vec<stitchd_db::SegmentIdMembership>, RepositoryError> {
            Ok(Vec::new())
        }
    }

    // -------------------------------------------------------------------------
    // Helper builders
    // -------------------------------------------------------------------------

    fn eq_rule(context_type: &str, param: &str, value: &str) -> Rule {
        Rule {
            name: None,
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: context_type.to_string(),
                param: param.to_string(),
                value: ParameterValue::Str(value.to_string()),
            }),
            output: RuleOutput::Variant(VariantId::new()),
        }
    }

    fn make_service(repo: MockSegmentRepoForTest) -> SegmentationServiceImpl {
        SegmentationServiceImpl::new(AppState::new(Arc::new(repo)))
    }

    fn env_id() -> (EnvironmentId, String) {
        let id = EnvironmentId::new();
        (id, id.as_uuid().to_string())
    }

    // -------------------------------------------------------------------------
    // Rule-based evaluation tests
    // -------------------------------------------------------------------------

    /// A rule-based segment that matches when `user.plan == "pro"` should return
    /// `is_member = true` when the context key identifies a known user and the
    /// `context_type` carries the discriminant.
    ///
    /// NOTE: `EvaluateMembership` does NOT accept arbitrary parameters — it only
    /// receives `(environment_id, segment_key, context_key, context_type)`.
    /// Rule-based evaluation uses the rule engine with a bare `Context` (key-only,
    /// no extra parameters). Therefore a rule that checks `user.plan == "pro"`
    /// will NEVER match via `EvaluateMembership` unless the rule only checks the
    /// context key itself.
    ///
    /// This test verifies the correct behaviour: if the segment rule requires
    /// `user.plan == "pro"` and the evaluation input has no such parameter,
    /// the result is `is_member = false`.
    #[tokio::test]
    async fn rule_based_no_params_in_context_returns_not_member() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = env_id();

        // Rule: user.plan == "pro" — won't match because bare context has no params.
        let rule = eq_rule("user", "plan", "pro");
        repo.insert_rule_segment(env_id, "beta-users", vec![rule]);

        let svc = make_service(repo);
        let resp = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "beta-users".to_string(),
                context_key: "user-123".to_string(),
                context_type: "user".to_string(),
            }))
            .await
            .expect("should not return gRPC error");

        assert!(
            !resp.into_inner().is_member,
            "bare context without matching params should not match rule"
        );
    }

    /// A segment with an empty rule list never matches any context.
    #[tokio::test]
    async fn rule_based_empty_rules_returns_not_member() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = env_id();

        repo.insert_rule_segment(env_id, "empty-seg", vec![]);

        let svc = make_service(repo);
        let resp = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "empty-seg".to_string(),
                context_key: "user-456".to_string(),
                context_type: "user".to_string(),
            }))
            .await
            .expect("should not return gRPC error");

        assert!(!resp.into_inner().is_member);
    }

    /// Requesting evaluation for a segment that does not exist returns `NOT_FOUND`.
    #[tokio::test]
    async fn rule_based_missing_segment_returns_not_found_status() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = env_id();

        let svc = make_service(repo);
        let result = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "nonexistent".to_string(),
                context_key: "user-789".to_string(),
                context_type: "user".to_string(),
            }))
            .await;

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    /// Passing an invalid `environment_id` UUID returns `INVALID_ARGUMENT`.
    #[tokio::test]
    async fn rule_based_invalid_env_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);

        let result = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: "not-a-uuid".to_string(),
                segment_key: "beta-users".to_string(),
                context_key: "user-123".to_string(),
                context_type: "user".to_string(),
            }))
            .await;

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    /// A rule with `InSegment` condition returns `FAILED_PRECONDITION`.
    #[tokio::test]
    async fn rule_based_in_segment_condition_returns_failed_precondition() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = env_id();

        // Rule containing an invalid InSegment condition.
        let bad_rule = Rule {
            name: None,
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::InSegment(SegmentId::new())),
            output: RuleOutput::Variant(VariantId::new()),
        };
        repo.insert_rule_segment(env_id, "invalid-seg", vec![bad_rule]);

        let svc = make_service(repo);
        let result = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "invalid-seg".to_string(),
                context_key: "user-123".to_string(),
                context_type: "user".to_string(),
            }))
            .await;

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    }

    // -------------------------------------------------------------------------
    // ActivateListGeneration (flag_lifecycle_20260604, Phase 10.3)
    // -------------------------------------------------------------------------

    /// Activating a prepared list generation full-replaces the member set via the
    /// generation-swap (`set_list_entries`), returning the activated counts.
    #[tokio::test]
    async fn activate_list_generation_swaps_in_prepared_member_set() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, _env_str) = env_id();
        let seg = repo.insert_list_segment(env_id, "vip", HashMap::new());
        let svc = make_service(repo);

        let resp = svc
            .activate_list_generation(Request::new(ActivateListGenerationRequest {
                segment_id: seg.id.as_uuid().to_string(),
                context_type: "user".to_string(),
                include: vec!["alice".to_string(), "bob".to_string()],
                exclude: vec!["mallory".to_string()],
            }))
            .await
            .expect("activation succeeds")
            .into_inner();

        assert_eq!(resp.include_count, 2);
        assert_eq!(resp.exclude_count, 1);
    }

    /// An invalid `segment_id` is rejected with `INVALID_ARGUMENT`.
    #[tokio::test]
    async fn activate_list_generation_rejects_bad_segment_id() {
        let svc = make_service(MockSegmentRepoForTest::new());
        let err = svc
            .activate_list_generation(Request::new(ActivateListGenerationRequest {
                segment_id: "not-a-uuid".to_string(),
                context_type: "user".to_string(),
                include: vec![],
                exclude: vec![],
            }))
            .await
            .expect_err("bad id rejected");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }
}
