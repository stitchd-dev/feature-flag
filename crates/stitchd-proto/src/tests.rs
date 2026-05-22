//! Compilation tests — verify that all generated proto types are accessible
//! and the crate builds cleanly with the expected module structure.

#[cfg(test)]
mod compilation_tests {
    use crate::{analytics, auth, common, experiments, flags, management, sdk, segments, stats};

    // ── Existing SDK-facing types ─────────────────────────────────────────────

    #[test]
    fn common_types_accessible() {
        let _: Option<common::v1::Context> = None;
        let _: Option<common::v1::ParameterValue> = None;
    }

    #[test]
    fn flag_sync_types_accessible() {
        let _: Option<flags::v1::FeatureFlag> = None;
        let _: Option<flags::v1::SyncRequest> = None;
        let _: Option<flags::v1::SyncResponse> = None;
        let _: Option<flags::v1::Variant> = None;
        let _: Option<flags::v1::FlagRule> = None;
    }

    // ── Phase 3 of flag_eval_unify_20260522 — new hash-input schema ────────────

    #[test]
    fn hash_selector_message_types_accessible() {
        // The new oneof-bearing message plus its two variants must be
        // reachable from the generated module.
        let _: Option<flags::v1::HashSelector> = None;
        let _: Option<flags::v1::ContextKeySelector> = None;
        let _: Option<flags::v1::ContextParameterSelector> = None;
    }

    #[test]
    fn percentage_allocation_hash_inputs_field_exists_and_defaults_empty() {
        // `prost`'s generated struct must include the new repeated field at
        // its declared tag. Default-constructed instances start with an empty
        // vec — exercising both the field name and the default path.
        let alloc = flags::v1::PercentageAllocation::default();
        assert!(alloc.hash_inputs.is_empty());
        assert!(alloc.context_hash_specs.is_empty());
    }

    #[test]
    fn hash_selector_oneof_context_key_round_trip() {
        use flags::v1::hash_selector::Selector;
        use prost::Message;

        let original = flags::v1::HashSelector {
            selector: Some(Selector::ContextKey(flags::v1::ContextKeySelector {
                context_type: "user".to_string(),
            })),
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).expect("encode");
        let decoded = flags::v1::HashSelector::decode(&buf[..]).expect("decode");

        match decoded.selector {
            Some(Selector::ContextKey(sel)) => assert_eq!(sel.context_type, "user"),
            other => panic!("expected ContextKey variant, got {other:?}"),
        }
    }

    #[test]
    fn hash_selector_oneof_context_parameter_round_trip() {
        use flags::v1::hash_selector::Selector;
        use prost::Message;

        let original = flags::v1::HashSelector {
            selector: Some(Selector::ContextParameter(
                flags::v1::ContextParameterSelector {
                    context_type: "device".to_string(),
                    parameter: "os".to_string(),
                },
            )),
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).expect("encode");
        let decoded = flags::v1::HashSelector::decode(&buf[..]).expect("decode");

        match decoded.selector {
            Some(Selector::ContextParameter(sel)) => {
                assert_eq!(sel.context_type, "device");
                assert_eq!(sel.parameter, "os");
            }
            other => panic!("expected ContextParameter variant, got {other:?}"),
        }
    }

    #[test]
    fn percentage_allocation_round_trip_with_dual_schema_state() {
        use flags::v1::hash_selector::Selector;
        use prost::Message;

        // Build a PercentageAllocation carrying BOTH legacy and new fields —
        // this models the Phase-3 dual-schema state the producer side will
        // continue to emit until Phase 5/6 retires the legacy map.
        let mut legacy: std::collections::HashMap<String, flags::v1::ContextHashSpec> =
            std::collections::HashMap::new();
        legacy.insert(
            "user".to_string(),
            flags::v1::ContextHashSpec {
                parameter_names: vec!["plan".to_string()],
            },
        );

        let new_inputs = vec![
            flags::v1::HashSelector {
                selector: Some(Selector::ContextKey(flags::v1::ContextKeySelector {
                    context_type: "application".to_string(),
                })),
            },
            flags::v1::HashSelector {
                selector: Some(Selector::ContextParameter(
                    flags::v1::ContextParameterSelector {
                        context_type: "user".to_string(),
                        parameter: "plan".to_string(),
                    },
                )),
            },
        ];

        let original = flags::v1::PercentageAllocation {
            context_hash_specs: legacy,
            buckets: vec![flags::v1::AllocationBucket {
                variant_key: "on".to_string(),
                weight_milli: 1000,
            }],
            hash_inputs: new_inputs,
        };

        let mut buf = Vec::new();
        original.encode(&mut buf).expect("encode");
        let decoded = flags::v1::PercentageAllocation::decode(&buf[..]).expect("decode");

        // Both schemas round-trip identically.
        assert_eq!(decoded.context_hash_specs.len(), 1);
        assert_eq!(
            decoded
                .context_hash_specs
                .get("user")
                .map(|s| s.parameter_names.as_slice()),
            Some(&["plan".to_string()][..])
        );
        assert_eq!(decoded.hash_inputs.len(), 2);
        match &decoded.hash_inputs[0].selector {
            Some(Selector::ContextKey(s)) => assert_eq!(s.context_type, "application"),
            other => panic!("hash_inputs[0] unexpected: {other:?}"),
        }
        match &decoded.hash_inputs[1].selector {
            Some(Selector::ContextParameter(s)) => {
                assert_eq!(s.context_type, "user");
                assert_eq!(s.parameter, "plan");
            }
            other => panic!("hash_inputs[1] unexpected: {other:?}"),
        }
        assert_eq!(decoded.buckets.len(), 1);
    }

    #[test]
    fn percentage_allocation_legacy_only_payload_decodes_with_empty_hash_inputs() {
        // Wire forward-compatibility: an older sender that hasn't migrated yet
        // emits `context_hash_specs` + `buckets` but NO `hash_inputs`. The
        // newer receiver must decode that payload cleanly with
        // `hash_inputs == []`.
        use prost::Message;

        let mut legacy: std::collections::HashMap<String, flags::v1::ContextHashSpec> =
            std::collections::HashMap::new();
        legacy.insert(
            "user".to_string(),
            flags::v1::ContextHashSpec {
                parameter_names: vec![],
            },
        );
        let sender = flags::v1::PercentageAllocation {
            context_hash_specs: legacy,
            buckets: vec![flags::v1::AllocationBucket {
                variant_key: "treatment".to_string(),
                weight_milli: 1000,
            }],
            hash_inputs: vec![],
        };
        let mut buf = Vec::new();
        sender.encode(&mut buf).expect("encode");
        let decoded = flags::v1::PercentageAllocation::decode(&buf[..]).expect("decode");
        assert!(decoded.hash_inputs.is_empty());
        assert_eq!(decoded.context_hash_specs.len(), 1);
    }

    #[test]
    fn segment_types_accessible() {
        let _: Option<segments::v1::RuleSegment> = None;
        let _: Option<segments::v1::ListSegment> = None;
        let _: Option<segments::v1::SegmentBundle> = None;
    }

    // ── New microservice contracts ─────────────────────────────────────────────

    #[test]
    fn auth_service_types_accessible() {
        let _: Option<auth::v1::CredentialRequest> = None;
        let _: Option<auth::v1::RbacContext> = None;
    }

    #[test]
    fn flag_service_types_accessible() {
        let _: Option<flags::v1::GetFlagRequest> = None;
        let _: Option<flags::v1::ListFlagsRequest> = None;
        let _: Option<flags::v1::ListFlagsResponse> = None;
        let _: Option<flags::v1::MutateFlagRequest> = None;
        let _: Option<flags::v1::MutateFlagResponse> = None;
        let _: Option<flags::v1::GetFlagDefinitionsRequest> = None;
    }

    #[test]
    fn segmentation_service_types_accessible() {
        let _: Option<segments::v1::GetSegmentRequest> = None;
        let _: Option<segments::v1::ListSegmentsRequest> = None;
        let _: Option<segments::v1::ListSegmentsResponse> = None;
        let _: Option<segments::v1::EvaluateMembershipRequest> = None;
        let _: Option<segments::v1::EvaluateMembershipResponse> = None;
        let _: Option<segments::v1::MutateSegmentRequest> = None;
        let _: Option<segments::v1::MutateSegmentResponse> = None;
    }

    #[test]
    fn experimentation_service_types_accessible() {
        let _: Option<experiments::v1::Experiment> = None;
        let _: Option<experiments::v1::ExperimentResults> = None;
        let _: Option<experiments::v1::CreateExperimentRequest> = None;
        let _: Option<experiments::v1::GetExperimentRequest> = None;
        let _: Option<experiments::v1::ListExperimentsRequest> = None;
        let _: Option<experiments::v1::ListExperimentsResponse> = None;
        let _: Option<experiments::v1::GetResultsRequest> = None;
    }

    #[test]
    fn analytics_service_types_accessible() {
        // Event ingestion
        let _: Option<analytics::v1::MetricEvent> = None;
        let _: Option<analytics::v1::IngestEventRequest> = None;
        let _: Option<analytics::v1::IngestEventResponse> = None;
        // Context registry
        let _: Option<analytics::v1::RegisterContextRequest> = None;
        let _: Option<analytics::v1::RegisterContextResponse> = None;
        let _: Option<analytics::v1::ListContextTypesRequest> = None;
        let _: Option<analytics::v1::ListContextTypesResponse> = None;
        let _: Option<analytics::v1::ListContextParamsRequest> = None;
        let _: Option<analytics::v1::ListContextParamsResponse> = None;
        // Eval stats + context intelligence
        let _: Option<analytics::v1::GetEvalStatsRequest> = None;
        let _: Option<analytics::v1::GetEvalStatsResponse> = None;
        let _: Option<analytics::v1::GetContextIntelligenceRequest> = None;
        let _: Option<analytics::v1::GetContextIntelligenceResponse> = None;
        // Experiment results (added by feature-flag-mwk.1.1 / 1.3)
        let _: Option<analytics::v1::ExperimentResult> = None;
        let _: Option<analytics::v1::WriteExperimentResultsRequest> = None;
        let _: Option<analytics::v1::WriteExperimentResultsResponse> = None;
        let _: Option<analytics::v1::ListExperimentResultsRequest> = None;
        let _: Option<analytics::v1::GetExperimentResultRequest> = None;
    }

    #[test]
    fn analytics_service_client_and_server_stubs_generated() {
        type _Client<T> = analytics::v1::analytics_service_client::AnalyticsServiceClient<T>;
        type _Server<T> = analytics::v1::analytics_service_server::AnalyticsServiceServer<T>;
    }

    #[test]
    fn management_service_types_accessible() {
        let _: Option<management::v1::CreateOrgRequest> = None;
        let _: Option<management::v1::CreateOrgResponse> = None;
        let _: Option<management::v1::CreateProjectRequest> = None;
        let _: Option<management::v1::ListProjectsRequest> = None;
        let _: Option<management::v1::ProjectSummary> = None;
        let _: Option<management::v1::CreateEnvironmentRequest> = None;
        let _: Option<management::v1::ListEnvironmentsRequest> = None;
        let _: Option<management::v1::EnvironmentSummary> = None;
        let _: Option<management::v1::RenameProjectRequest> = None;
        let _: Option<management::v1::DeleteProjectRequest> = None;
        let _: Option<management::v1::RenameEnvironmentRequest> = None;
        let _: Option<management::v1::DeleteEnvironmentRequest> = None;
    }

    #[test]
    fn stats_service_types_accessible() {
        let _: Option<stats::v1::TriggerRecomputeRequest> = None;
        let _: Option<stats::v1::TriggerRecomputeResponse> = None;
        let _: Option<stats::v1::GetJobStatusRequest> = None;
        let _: Option<stats::v1::GetJobStatusResponse> = None;
    }

    #[test]
    fn stats_service_client_and_server_stubs_generated() {
        type _Client<T> = stats::v1::stats_service_client::StatsServiceClient<T>;
        type _Server<T> = stats::v1::stats_service_server::StatsServiceServer<T>;
    }

    // ── SDK contracts (sdks/spec/proto/sdk/v1/) ─────────────────────────────

    #[test]
    fn sdk_service_messages_accessible() {
        let _: Option<sdk::v1::SyncDefinitionsRequest> = None;
        let _: Option<sdk::v1::SyncDefinitionsResponse> = None;
        let _: Option<sdk::v1::IngestSdkEvalLogRequest> = None;
        let _: Option<sdk::v1::IngestSdkEvalLogResponse> = None;
        let _: Option<sdk::v1::FlagEvaluationEvent> = None;
    }

    #[test]
    fn sdk_service_client_and_server_stubs_generated() {
        // Both client and server stubs should be generated by tonic_build.
        // If these typedefs compile, both halves of the gRPC contract exist.
        type _GatewayClient<T> = sdk::v1::sdk_service_client::SdkServiceClient<T>;
        type _GatewayServer<T> = sdk::v1::sdk_service_server::SdkServiceServer<T>;
        type _FlagBackendClient<T> =
            sdk::v1::flag_sdk_backend_service_client::FlagSdkBackendServiceClient<T>;
        type _FlagBackendServer<T> =
            sdk::v1::flag_sdk_backend_service_server::FlagSdkBackendServiceServer<T>;
        type _SegBackendClient<T> =
            sdk::v1::segmentation_sdk_backend_service_client::SegmentationSdkBackendServiceClient<
                T,
            >;
        type _SegBackendServer<T> =
            sdk::v1::segmentation_sdk_backend_service_server::SegmentationSdkBackendServiceServer<
                T,
            >;
    }

    #[test]
    fn sdk_backend_messages_accessible() {
        let _: Option<sdk::v1::BatchCheckListMembershipRequest> = None;
        let _: Option<sdk::v1::BatchCheckListMembershipResponse> = None;
        let _: Option<sdk::v1::MembershipQuery> = None;
        let _: Option<sdk::v1::MembershipResult> = None;
    }
}
