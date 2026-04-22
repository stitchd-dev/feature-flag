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
            - **SDK routes** (`/v1/environments/*/evaluate`, `/v1/environments/*/events`, \
              `/v1/environments/*/segments/list-check`) require an `x-sdk-key` header.\n\
            - **Admin / resource routes** require a `Bearer` JWT in the `Authorization` header."
    ),
    paths(
        // Auth
        crate::routes::auth::login,
        // Admin (system-org only)
        crate::routes::admin::create_org,
        crate::routes::admin::seed_user,
        // Management (non-system-org)
        crate::routes::management::create_project,
        crate::routes::management::create_environment,
        crate::routes::management::create_sdk_key,
        crate::routes::management::create_user,
        // SDK key routes
        crate::routes::sdk::evaluate,
        crate::routes::sdk::ingest_event,
        crate::routes::sdk::ingest_batch_events,
        crate::routes::sdk::list_check_membership,
        crate::routes::sdk::batch_list_check_membership,
        // Flags (JWT)
        crate::routes::flags::list_flags,
        crate::routes::flags::create_flag,
        crate::routes::flags::get_flag,
        crate::routes::flags::update_flag,
        crate::routes::flags::delete_flag,
        crate::routes::flags::create_variant,
        crate::routes::flags::update_rules,
        crate::routes::flags::update_flag_hashing,
        // Segments (JWT)
        crate::routes::segments::list_segments,
        crate::routes::segments::create_segment,
        crate::routes::segments::get_segment,
        crate::routes::segments::update_segment,
        crate::routes::segments::delete_segment,
        // Events (JWT)
        crate::routes::events::ingest_event,
        crate::routes::events::ingest_batch,
        crate::routes::events::list_event_definitions,
        crate::routes::events::create_event_definition,
        crate::routes::events::get_event_definition,
        crate::routes::events::update_event_definition,
        crate::routes::events::delete_event_definition,
        // Experiments (JWT)
        crate::routes::experiments::list_experiments,
        crate::routes::experiments::create_experiment,
        crate::routes::experiments::get_experiment,
        crate::routes::experiments::update_experiment,
        crate::routes::experiments::delete_experiment,
        crate::routes::experiments::transition_experiment,
        crate::routes::experiments::list_iterations,
        crate::routes::experiments::get_results,
    ),
    components(
        schemas(
            // Auth
            crate::routes::auth::LoginBody,
            crate::routes::auth::LoginJson,
            // Admin
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
            // SDK
            crate::routes::sdk::EvaluateRequest,
            crate::routes::sdk::EvaluateResponse,
            crate::routes::sdk::SdkEventBody,
            crate::routes::sdk::IngestResponseJson,
            crate::routes::sdk::ListCheckRequest,
            crate::routes::sdk::ListCheckResponse,
            // Flags
            crate::routes::flags::FlagMutateRequest,
            crate::routes::flags::FlagJson,
            crate::routes::flags::HashingConfigItem,
            crate::routes::flags::UpdateHashingBody,
            crate::routes::flags::HashingConfigJson,
            crate::routes::flags::UpdateHashingResponse,
            // Segments
            crate::routes::segments::SegmentCreateRequest,
            crate::routes::segments::SegmentJson,
            // Events
            crate::routes::events::EventBody,
            crate::routes::events::BatchEventBody,
            crate::routes::events::IngestResponseJson,
            // Experiments
            crate::routes::experiments::CreateExperimentBody,
            crate::routes::experiments::UpdateExperimentBody,
            crate::routes::experiments::TransitionBody,
            crate::routes::experiments::ExperimentJson,
            crate::routes::experiments::IterationJson,
            crate::routes::experiments::VariantResultJson,
            crate::routes::experiments::ExperimentResultsJson,
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
