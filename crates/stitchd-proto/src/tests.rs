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
        type _Client<T> =
            analytics::v1::analytics_service_client::AnalyticsServiceClient<T>;
        type _Server<T> =
            analytics::v1::analytics_service_server::AnalyticsServiceServer<T>;
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
