//! OpenAPI v3 document for the stitchd-gateway REST surface.
//!
//! Run `cargo xtask docs` or invoke the gateway with `--export-openapi <path>`
//! to regenerate `docs/src/api/openapi.json`.

use utoipa::{
    Modify, OpenApi,
    openapi::{
        Components,
        security::{ApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityScheme},
    },
};

/// Top-level OpenAPI document.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "stitchd-gateway API",
        version = "0.1.0",
        description = "REST gateway for the stitchd feature-flag and experimentation platform.\n\n\
            ## Auth Models\n\
            - **SDK routes** (`/v1/sdk/*`) require an `x-sdk-key` header validated by sdk_auth_middleware.\n\
            - **Superadmin routes** (`/v1/superadmin/*`) require a `Bearer` JWT + system-org membership.\n\
            - **Management routes** (`/v1/management/*`) require a `Bearer` JWT + non-system-org membership.\n\
            - **Resource routes** require a `Bearer` JWT in the `Authorization` header."
    ),
    paths(
        // Auth
        crate::routes::auth::login,
        // Superadmin (system-org only)
        crate::routes::admin::create_org,
        crate::routes::admin::seed_user,
        // Management (non-system-org)
        crate::routes::management::create_project,
        crate::routes::management::create_environment,
        crate::routes::management::create_sdk_key,
        crate::routes::management::create_user,
        // Flags (JWT)
        crate::routes::flags::list_flags,
        crate::routes::flags::create_flag,
        crate::routes::flags::get_flag,
        crate::routes::flags::update_flag,
        crate::routes::flags::delete_flag,
        crate::routes::flags::archive_flag,
        crate::routes::flags::update_variants,
        crate::routes::flags::update_rules,
        crate::routes::flags::update_flag_hashing,
        crate::routes::flags::set_default_rule_distribution,
        // Segments (JWT)
        crate::routes::segments::list_segments,
        crate::routes::segments::create_segment,
        crate::routes::segments::create_segment_in_env,
        crate::routes::segments::get_segment,
        crate::routes::segments::update_segment,
        crate::routes::segments::delete_segment,
        // Event definitions (JWT)
        crate::routes::events::ingest_event,
        crate::routes::events::ingest_batch,
        crate::routes::events::list_event_definitions,
        crate::routes::events::create_event_definition,
        crate::routes::events::get_event_definition,
        crate::routes::events::update_event_definition,
        crate::routes::events::delete_event_definition,
        // Event tracking (SDK key)
        crate::routes::events::track_events,
        // Event tracking (admin JWT — test-event widget)
        crate::routes::events::track_events_admin,
        // EventDetail page (JWT)
        crate::routes::events::get_event_firings,
        crate::routes::events::get_event_stats,
        // Metrics (JWT)
        crate::routes::metrics::create_metric,
        crate::routes::metrics::list_metrics,
        crate::routes::metrics::get_metric,
        crate::routes::metrics::update_metric,
        crate::routes::metrics::delete_metric,
        crate::routes::metrics::preview_metric,
        // Experiments (JWT)
        crate::routes::experiments::list_experiments,
        crate::routes::experiments::create_experiment,
        crate::routes::experiments::get_experiment,
        crate::routes::experiments::update_experiment,
        crate::routes::experiments::delete_experiment,
        crate::routes::experiments::transition_experiment,
        crate::routes::experiments::list_iterations,
        crate::routes::experiments::get_results,
        crate::routes::experiments::list_exposures,
        crate::routes::experiments::get_interactions,
        // Exclusion groups (JWT)
        crate::routes::exclusion_groups::list_exclusion_groups,
        crate::routes::exclusion_groups::create_exclusion_group,
        crate::routes::exclusion_groups::get_exclusion_group,
        crate::routes::exclusion_groups::update_exclusion_group,
        crate::routes::exclusion_groups::delete_exclusion_group,
        crate::routes::exclusion_groups::assign_experiment_to_group,
        crate::routes::exclusion_groups::unassign_experiment,
        // Stats recompute
        crate::routes::stats::trigger_recompute,
        crate::routes::stats::get_job_status,
        crate::routes::stats::get_timeseries,
        crate::routes::stats::trigger_recompute_env_scoped,
        crate::routes::stats::get_recompute_job_status,
        // Schedules (flag_lifecycle, JWT)
        crate::routes::schedules::create_schedule,
        crate::routes::schedules::list_schedules,
        crate::routes::schedules::get_schedule,
        crate::routes::schedules::cancel_schedule,
        crate::routes::schedules::pause_schedule,
        crate::routes::schedules::resume_schedule,
    ),
    components(
        schemas(
            // Auth
            crate::routes::auth::LoginBody,
            crate::routes::auth::LoginJson,
            // Superadmin
            crate::routes::admin::CreateOrgBody,
            crate::routes::admin::OrgJson,
            crate::routes::admin::SeedUserBody,
            crate::routes::admin::UserJson,
            // Management
            crate::routes::management::CreateProjectBody,
            crate::routes::management::ProjectJson,
            crate::routes::management::CreateEnvBody,
            crate::routes::management::EnvJson,
            crate::routes::management::SdkKeyJson,
            crate::routes::management::CreateUserBody,
            crate::routes::management::UserJson,
            // Flags
            crate::routes::flags::FlagMutateRequest,
            crate::routes::flags::HashingConfigItem,
            crate::routes::flags::UpdateHashingBody,
            crate::routes::flags::HashingConfigJson,
            crate::routes::flags::UpdateHashingResponse,
            crate::routes::flags::DefaultRuleAllocationBody,
            crate::routes::flags::DefaultRuleDistributionBody,
            crate::routes::flags::SetDefaultRuleDistributionBody,
            crate::routes::flags::SetDefaultRuleDistributionResponseJson,
            // Phase 4 (flag_eval_unify_20260522) — rule CRUD DTOs.
            crate::routes::flags::HashSelectorJson,
            crate::routes::flags::RuleBody,
            crate::routes::flags::ReplaceRulesBody,
            crate::routes::flags::RuleJson,
            crate::routes::flags::VariantBody,
            crate::routes::flags::VariantJson,
            crate::routes::flags::AdminFlagJson,
            // Segments
            crate::routes::segments::SegmentCreateRequest,
            crate::routes::segments::SegmentUpdateRequest,
            crate::routes::segments::AdminSegmentJson,
            crate::routes::segments::ListSegmentsQuery,
            // Events
            crate::routes::events::EventBody,
            crate::routes::events::BatchEventBody,
            crate::routes::events::IngestResponseJson,
            crate::routes::events::TypedValue,
            crate::routes::events::TrackEvent,
            crate::routes::events::TrackEventsRequest,
            crate::routes::events::RejectedEvent,
            crate::routes::events::TrackEventsResponse,
            crate::routes::events::FiringsQuery,
            crate::routes::events::StatsQuery,
            crate::routes::events::EventFiringJson,
            crate::routes::events::EventFiringsResponseJson,
            crate::routes::events::EventStatsBucketJson,
            crate::routes::events::EventStatsResponseJson,
            // Metrics
            crate::routes::metrics::CreateMetricBody,
            crate::routes::metrics::UpdateMetricBody,
            crate::routes::metrics::PreviewMetricBody,
            crate::routes::metrics::PreviewMetricResponseJson,
            crate::routes::metrics::PreviewBucketJson,
            crate::routes::metrics::ListMetricsQuery,
            crate::routes::metrics::AggregationConfigBody,
            crate::routes::metrics::RatioConfigBody,
            crate::routes::metrics::FunnelStepBody,
            crate::routes::metrics::FunnelConfigBody,
            crate::routes::metrics::MetricKindBody,
            // Core MetricDefinition + supporting types — used as response shape
            stitchd_core::metric::MetricDefinition,
            stitchd_core::metric::MetricKind,
            stitchd_core::metric::AggregationConfig,
            stitchd_core::metric::AggregationOperator,
            stitchd_core::metric::RatioConfig,
            stitchd_core::metric::FunnelConfig,
            stitchd_core::metric::FunnelStep,
            stitchd_core::metric::GoalDirection,
            stitchd_core::id::MetricId,
            stitchd_core::id::EnvironmentId,
            // Experiments
            crate::routes::experiments::CreateExperimentBody,
            crate::routes::experiments::UpdateExperimentBody,
            crate::routes::experiments::TransitionBody,
            crate::routes::experiments::ExperimentJson,
            crate::routes::experiments::IterationJson,
            crate::routes::experiments::VariantResultJson,
            crate::routes::experiments::SrmResultJson,
            crate::routes::experiments::SrmPerVariantJson,
            crate::routes::experiments::ContextTypeResultsJson,
            crate::routes::experiments::BoundTargetJson,
            crate::routes::experiments::ExperimentResultsJson,
            crate::routes::experiments::ExposureRowJson,
            crate::routes::experiments::ListExposuresQuery,
            crate::routes::experiments::ListIterationsQuery,
            crate::routes::experiments::ExperimentInteractionJson,
            crate::routes::experiments::ExperimentInteractionsJson,
            // Exclusion groups
            crate::routes::exclusion_groups::ExclusionGroupJson,
            crate::routes::exclusion_groups::CreateExclusionGroupBody,
            crate::routes::exclusion_groups::UpdateExclusionGroupBody,
            crate::routes::exclusion_groups::AssignExperimentBody,
            crate::routes::exclusion_groups::AssignmentResultJson,
            crate::routes::exclusion_groups::ListExclusionGroupsQuery,
            // Stats
            crate::routes::stats::RecomputeJobJson,
            crate::routes::stats::JobStatusJson,
            crate::routes::stats::GetTimeseriesQuery,
            crate::routes::stats::TimeseriesBucketJson,
            crate::routes::stats::GetTimeseriesResponseJson,
            // Schedules (flag_lifecycle)
            crate::routes::schedules::CreateScheduleBody,
            crate::routes::schedules::ScheduleVersionBody,
            crate::routes::schedules::ScheduleJson,
            crate::routes::schedules::ScheduleRunJson,
        )
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Components::default);
        components.add_security_scheme(
            "sdk_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("x-sdk-key"))),
        );
        components.add_security_scheme(
            "bearer_jwt",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
    }
}

/// Serialise the OpenAPI document to a pretty-printed JSON string and write it
/// to `path`.  Exits the process with code 0 on success, 1 on failure.
pub fn export_to_file(path: &str) {
    let spec = match ApiDoc::openapi().to_pretty_json() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to serialise OpenAPI spec: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::write(path, &spec) {
        eprintln!("error: failed to write OpenAPI spec to {path}: {e}");
        std::process::exit(1);
    }
    println!("OpenAPI spec written to {path}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_doc_serialises_to_valid_non_empty_json() {
        let doc = ApiDoc::openapi();
        let json = doc.to_json().expect("must serialise to JSON");
        assert!(!json.is_empty(), "serialised spec must not be empty");

        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("serialised spec must be valid JSON");

        // Required OpenAPI v3 top-level fields
        assert!(parsed.get("openapi").is_some(), "must have 'openapi' field");
        assert!(parsed.get("info").is_some(), "must have 'info' field");
        assert_eq!(
            parsed["info"]["title"].as_str().unwrap_or(""),
            "stitchd-gateway API"
        );
        assert_eq!(parsed["info"]["version"].as_str().unwrap_or(""), "0.1.0");
    }

    #[test]
    fn variant_result_schema_documents_sequential_fields() {
        // The Admin UI consumes these via the OpenAPI schema; the `ToSchema`
        // derive must surface the additive sequential_* fields on VariantResultJson.
        let doc = ApiDoc::openapi();
        let json = doc.to_json().expect("must serialise");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let props = &parsed["components"]["schemas"]["VariantResultJson"]["properties"];
        for field in [
            "sequential_p_value",
            "sequential_ci_lower",
            "sequential_ci_upper",
            "sequential_crossed",
            "sequential_insufficient_data",
            "sequential_method",
        ] {
            assert!(
                props.get(field).is_some(),
                "VariantResultJson schema must document {field}"
            );
        }
    }

    #[test]
    fn security_schemes_are_present() {
        let doc = ApiDoc::openapi();
        let json = doc.to_json().expect("must serialise");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let schemes = &parsed["components"]["securitySchemes"];
        assert!(
            schemes.get("sdk_key").is_some(),
            "must define sdk_key security scheme"
        );
        assert!(
            schemes.get("bearer_jwt").is_some(),
            "must define bearer_jwt security scheme"
        );
    }

    #[test]
    fn export_to_file_writes_valid_json() {
        let dir = std::env::temp_dir();
        let path = dir.join("stitchd_gateway_openapi_test.json");
        let path_str = path.to_str().unwrap();

        export_to_file(path_str);

        let content = std::fs::read_to_string(&path).expect("file must be written");
        assert!(!content.is_empty(), "written file must not be empty");
        let parsed: serde_json::Value =
            serde_json::from_str(&content).expect("written file must be valid JSON");
        assert!(parsed.get("openapi").is_some());

        let _ = std::fs::remove_file(&path);
    }
}
