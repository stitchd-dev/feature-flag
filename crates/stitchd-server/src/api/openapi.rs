//! OpenAPI schema aggregator for the Stitchd REST API.
// utoipa::OpenApi derive macro generates a for_each internally; suppress the lint.
#![allow(clippy::needless_for_each)]

use crate::api::event_definitions::handlers::{
    CreateEventDefinitionRequest, EventDefinitionResponse,
};
use crate::api::flags::handlers::{
    BatchEvaluationResponse, CreateFlagRequest, CreateVariantRequest, EvaluationRequest,
    EvaluationResult, FlagResponse, UpdateFlagRequest,
};
use crate::api::segments::types::{
    BatchContextMembership, BatchListCheckRequest, BatchListCheckResponse, CreateSegmentRequest,
    ListCheckRequest, ListCheckResponse, SegmentResponse, UpdateSegmentRequest,
};
use stitchd_core::{
    context::{Context, EvaluationContext, ParameterValue},
    event::EventValueType,
    flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType, Variant},
    rule_engine::{
        condition::Condition,
        types::{ConditionExpr, PercentageTarget, Rule, RuleOutput, TargetField},
    },
    segment::{Segment, SegmentType},
    variants::VariantValue,
};

/// Aggregated `OpenAPI` document for the Stitchd REST API.
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Stitchd Feature Flag API",
        version = "0.1.0",
        description = "REST API for managing feature flags, segments, and evaluation."
    ),
    paths(
        crate::api::event_definitions::handlers::list_event_definitions,
        crate::api::event_definitions::handlers::create_event_definition,
        crate::api::event_definitions::handlers::delete_event_definition,
        crate::api::flags::handlers::list_flags,
        crate::api::flags::handlers::create_flag,
        crate::api::flags::handlers::get_flag,
        crate::api::flags::handlers::update_flag,
        crate::api::flags::handlers::delete_flag,
        crate::api::flags::handlers::create_variant,
        crate::api::flags::handlers::update_hashing_config,
        crate::api::flags::handlers::update_rules,
        crate::api::flags::handlers::evaluate_all_flags,
        crate::api::segments::handlers::list_segments,
        crate::api::segments::handlers::create_segment,
        crate::api::segments::handlers::get_segment,
        crate::api::segments::handlers::update_segment,
        crate::api::segments::handlers::delete_segment,
        crate::api::segments::handlers::list_check_membership,
        crate::api::segments::handlers::batch_list_check_membership,
    ),
    components(schemas(
        // Request/response types from handlers
        CreateEventDefinitionRequest,
        EventDefinitionResponse,
        EventValueType,
        CreateFlagRequest,
        UpdateFlagRequest,
        FlagResponse,
        CreateVariantRequest,
        EvaluationRequest,
        EvaluationResult,
        BatchEvaluationResponse,
        CreateSegmentRequest,
        UpdateSegmentRequest,
        SegmentResponse,
        ListCheckRequest,
        ListCheckResponse,
        BatchListCheckRequest,
        BatchListCheckResponse,
        BatchContextMembership,
        // Core domain types
        FlagRecord,
        FlagValueType,
        FlagHashingConfig,
        FlagRule,
        Variant,
        VariantValue,
        Segment,
        SegmentType,
        EvaluationContext,
        Context,
        ParameterValue,
        Rule,
        ConditionExpr,
        TargetField,
        PercentageTarget,
        RuleOutput,
        Condition,
    ))
)]
pub struct StitchdApiDoc;
