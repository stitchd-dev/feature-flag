//! Flag route handlers — proxy REST requests to the Flag Service via gRPC.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::flags::v1::{
    EvaluatePreviewRequest, FeatureFlag, FlagHashingConfig, GetFlagRequest, ListFlagsRequest,
    MutateFlagRequest, MutationKind, UpdateFlagHashingRequest,
};

use crate::error::GatewayError;
use crate::pagination::{PaginatedResponse, PaginationParams};
use crate::state::GatewayState;

// ─── REST request / response types ───────────────────────────────────────────

/// A variant value in a create/update request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VariantBody {
    pub key: String,
    pub value: serde_json::Value,
}

/// Request body for creating or updating a flag.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FlagMutateRequest {
    pub key: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    /// Value type: "bool" | "int" | "double" | "string" | "json".
    /// Accepts both `value_type` and `flag_type` for consistency with list responses.
    #[serde(alias = "flag_type")]
    pub value_type: Option<String>,
    pub variants: Option<Vec<VariantBody>>,
    #[schema(value_type = Object, nullable = true)]
    pub flag: Option<serde_json::Value>,
    pub version: Option<u64>,
    /// Key of the variant to serve when no rules match (or flag is disabled).
    pub default_variant_key: Option<String>,
}

/// Query parameters for listing flags.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListFlagsQuery {
    #[serde(default)]
    pub include_archived: bool,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct HashingConfigItem {
    pub parameter_key: String,
    pub parameter_type: String,
    pub order: i32,
}

/// JSON wire shape mirroring `stitchd_core::evaluation::types::HashSelector`.
///
/// `#[serde(tag = "kind", rename_all = "snake_case")]` matches the core
/// type's representation so the gateway DTO and core type round-trip
/// directly through `serde_json::to_value` if a caller needs to.
///
/// Wire form:
/// ```json
/// {"kind": "context_key", "context_type": "user"}
/// {"kind": "context_parameter", "context_type": "device", "parameter": "os"}
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HashSelectorJson {
    /// Hash the `key` of the named context type.
    ContextKey {
        /// Context type whose `key` is hashed (e.g. "user").
        context_type: String,
    },
    /// Hash a named parameter of the named context type.
    ContextParameter {
        /// Context type to look up (e.g. "device").
        context_type: String,
        /// Parameter name within that context (e.g. "os").
        parameter: String,
    },
}

impl HashSelectorJson {
    /// Convert the JSON DTO into a proto `HashSelector`.
    fn to_proto(&self) -> stitchd_proto::flags::v1::HashSelector {
        use stitchd_proto::flags::v1::{
            ContextKeySelector, ContextParameterSelector, HashSelector as ProtoSelector,
            hash_selector::Selector,
        };
        let inner = match self {
            HashSelectorJson::ContextKey { context_type } => {
                Selector::ContextKey(ContextKeySelector {
                    context_type: context_type.clone(),
                })
            }
            HashSelectorJson::ContextParameter {
                context_type,
                parameter,
            } => Selector::ContextParameter(ContextParameterSelector {
                context_type: context_type.clone(),
                parameter: parameter.clone(),
            }),
        };
        ProtoSelector {
            selector: Some(inner),
        }
    }

    /// Reconstruct a JSON DTO from a proto `HashSelector`. Returns `None`
    /// when the oneof is empty (legacy / corrupted payload).
    fn from_proto(p: &stitchd_proto::flags::v1::HashSelector) -> Option<Self> {
        use stitchd_proto::flags::v1::hash_selector::Selector;
        match p.selector.as_ref()? {
            Selector::ContextKey(s) => Some(Self::ContextKey {
                context_type: s.context_type.clone(),
            }),
            Selector::ContextParameter(s) => Some(Self::ContextParameter {
                context_type: s.context_type.clone(),
                parameter: s.parameter.clone(),
            }),
        }
    }

    /// Stable identity used by duplicate-detection: `(context_type, field)`
    /// where `field` is either `"__key__"` (for ContextKey) or the parameter
    /// name (for ContextParameter). The reserved `__key__` sentinel can never
    /// collide with a real parameter name (validated separately).
    fn identity(&self) -> (String, String) {
        match self {
            Self::ContextKey { context_type } => (context_type.clone(), "__key__".to_string()),
            Self::ContextParameter {
                context_type,
                parameter,
            } => (context_type.clone(), parameter.clone()),
        }
    }
}

/// Validate a `hash_inputs` array per FR-8 of `flag_eval_unify_20260522`:
///
/// 1. Non-empty.
/// 2. No two selectors share the same `(context_type, field)` identity.
/// 3. `ContextParameter` selectors require a non-empty `parameter`.
///
/// Returns a human-readable error message when invalid; `Ok(())` otherwise.
pub fn validate_hash_inputs(selectors: &[HashSelectorJson]) -> Result<(), String> {
    if selectors.is_empty() {
        return Err("hash_inputs must not be empty".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for selector in selectors {
        if let HashSelectorJson::ContextParameter { parameter, .. } = selector
            && parameter.is_empty()
        {
            return Err(
                "hash_inputs: context_parameter selector requires a non-empty `parameter`"
                    .to_string(),
            );
        }
        let id = selector.identity();
        if !seen.insert(id.clone()) {
            return Err(format!(
                "hash_inputs: duplicate selector for context_type=`{}` field=`{}`",
                id.0,
                if id.1 == "__key__" { "<key>" } else { &id.1 }
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateHashingBody {
    pub configs: Vec<HashingConfigItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HashingConfigJson {
    pub parameter_key: String,
    pub parameter_type: String,
    pub order: i32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateHashingResponse {
    pub flag: AdminFlagJson,
    pub configs: Vec<HashingConfigJson>,
}

/// Variant as returned in admin API responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct VariantJson {
    pub key: String,
    pub value: serde_json::Value,
}

/// Rule as returned in admin API responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct RuleJson {
    /// UUID of the underlying `feature_flag_rules.id` row. Empty string when the
    /// flag-service did not surface a rule ID (legacy SDK-sync style payload).
    pub rule_id: String,
    /// Optional human-readable label set by the user; ignored by the evaluator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Decoded ConditionExpr JSON.
    pub condition: serde_json::Value,
    /// Rule output — `{"variant_key": "..."}` or `{"allocation": [...]}`.
    pub output: serde_json::Value,
    /// Segment UUIDs referenced in the condition expression (convenience for the UI).
    /// Populated by extracting all `InSegment` / `NotInSegment` leaf values.
    pub segment_ids: Vec<String>,
}

/// Full admin representation of a feature flag.
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminFlagJson {
    pub flag_id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub flag_type: String,
    pub enabled: bool,
    pub status: String,
    pub version: u64,
    pub variants: Vec<VariantJson>,
    pub rules: Vec<RuleJson>,
    pub default_variant_key: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// UUID of the experiment currently locking this flag (running or paused),
    /// or `None` when the flag is not locked. Lets the admin UI render the
    /// lock badge before the user attempts a save and gets a 409.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_by_experiment_id: Option<String>,
}

fn proto_variant_value_to_json(
    v: Option<stitchd_proto::flags::v1::VariantValue>,
) -> serde_json::Value {
    use stitchd_proto::flags::v1::variant_value::Value;
    match v.and_then(|vv| vv.value) {
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(b),
        Some(Value::IntValue(i)) => serde_json::json!(i),
        Some(Value::DoubleValue(d)) => serde_json::json!(d),
        Some(Value::StringValue(s)) => serde_json::Value::String(s),
        Some(Value::JsonValue(s)) => {
            serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s))
        }
        None => serde_json::Value::Null,
    }
}

/// Recursively collect all segment UUIDs from a `ConditionExpr` JSON tree.
///
/// The `ConditionExpr` serde representation uses variant names as keys:
/// - `{"Leaf": {"InSegment": "<uuid>"}}`
/// - `{"Leaf": {"NotInSegment": "<uuid>"}}`
/// - `{"And": [...]}`  / `{"Or": [...]}` / `{"Not": {...}}`
fn collect_segment_ids(expr: &serde_json::Value, out: &mut Vec<String>) {
    match expr {
        serde_json::Value::Object(map) => {
            if let Some(leaf) = map.get("Leaf") {
                if let Some(seg_id) = leaf.get("InSegment").and_then(|v| v.as_str()) {
                    out.push(seg_id.to_string());
                } else if let Some(seg_id) = leaf.get("NotInSegment").and_then(|v| v.as_str()) {
                    out.push(seg_id.to_string());
                }
            }
            for child in map.values() {
                collect_segment_ids(child, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_segment_ids(item, out);
            }
        }
        _ => {}
    }
}

fn flag_rule_to_json(r: &stitchd_proto::flags::v1::FlagRule) -> RuleJson {
    use stitchd_proto::flags::v1::flag_rule::Output;

    let condition = if r.rule_payload.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&r.rule_payload).unwrap_or(serde_json::Value::Null)
    };

    let output = match &r.output {
        Some(Output::VariantKey(k)) => serde_json::json!({ "variant_key": k }),
        Some(Output::Allocation(alloc)) => {
            let hash_inputs_json: Vec<serde_json::Value> = alloc
                .hash_inputs
                .iter()
                .filter_map(HashSelectorJson::from_proto)
                .filter_map(|sel| serde_json::to_value(&sel).ok())
                .collect();

            let mut hash_targets: Vec<serde_json::Value> = Vec::new();
            for sel in &alloc.hash_inputs {
                use stitchd_proto::flags::v1::hash_selector::Selector;
                if let Some(inner) = sel.selector.as_ref() {
                    match inner {
                        Selector::ContextKey(s) => hash_targets.push(serde_json::json!({
                            "context_type": s.context_type,
                            "field": "key"
                        })),
                        Selector::ContextParameter(s) => hash_targets.push(serde_json::json!({
                            "context_type": s.context_type,
                            "field": s.parameter
                        })),
                    }
                }
            }
            let buckets: Vec<_> = alloc
                .buckets
                .iter()
                .map(|b| serde_json::json!({ "variant_key": b.variant_key, "weight_milli": b.weight_milli }))
                .collect();
            serde_json::json!({
                "allocation": {
                    "hash_inputs": hash_inputs_json,
                    "hash_targets": hash_targets,
                    "buckets": buckets
                }
            })
        }
        None => serde_json::Value::Null,
    };

    let mut segment_ids = Vec::new();
    collect_segment_ids(&condition, &mut segment_ids);
    // Deduplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    segment_ids.retain(|id| seen.insert(id.clone()));

    let name = if r.name.is_empty() {
        None
    } else {
        Some(r.name.clone())
    };
    RuleJson {
        rule_id: r.rule_id.clone(),
        name,
        condition,
        output,
        segment_ids,
    }
}

/// Validate that every variant's value matches the declared flag type.
/// Returns `None` when all values are valid, or `Some(error_message)` on the
/// first bad value.
fn validate_variant_values(
    variants: &[VariantBody],
    value_type: stitchd_proto::flags::v1::FlagValueType,
) -> Option<String> {
    use stitchd_proto::flags::v1::FlagValueType;
    for v in variants {
        let ok = match value_type {
            FlagValueType::Bool => matches!(v.value, serde_json::Value::Bool(_)),
            FlagValueType::Int => {
                // Must be a JSON number that round-trips as an i64.
                v.value.as_i64().is_some()
            }
            FlagValueType::Double => {
                // Any JSON number is acceptable.
                v.value.is_number()
            }
            FlagValueType::String => v.value.is_string(),
            // JSON flags accept any valid JSON value (object, array, primitive).
            FlagValueType::Json | FlagValueType::Unspecified => true,
        };
        if !ok {
            let expected = match value_type {
                FlagValueType::Bool => "boolean (true or false)",
                FlagValueType::Int => "integer number (e.g. 42)",
                FlagValueType::Double => "decimal number (e.g. 3.14)",
                FlagValueType::String => "string (e.g. \"hello\")",
                _ => "JSON value",
            };
            return Some(format!(
                "Variant \"{}\": expected {}, got `{}`",
                v.key, expected, v.value
            ));
        }
    }
    // Ensure all variant keys are non-empty and unique.
    let mut seen = std::collections::HashSet::new();
    for v in variants {
        if v.key.trim().is_empty() {
            return Some("Variant key must not be empty".to_string());
        }
        if !seen.insert(v.key.trim()) {
            return Some(format!("Duplicate variant key: \"{}\"", v.key.trim()));
        }
    }
    None
}

fn parse_value_type(s: &str) -> stitchd_proto::flags::v1::FlagValueType {
    use stitchd_proto::flags::v1::FlagValueType;
    match s {
        "bool" => FlagValueType::Bool,
        "int" => FlagValueType::Int,
        "double" => FlagValueType::Double,
        "string" | "str" => FlagValueType::String,
        "json" => FlagValueType::Json,
        _ => FlagValueType::Unspecified,
    }
}

fn variant_body_to_proto(v: VariantBody) -> stitchd_proto::flags::v1::Variant {
    use stitchd_proto::flags::v1::{VariantValue, variant_value::Value};
    let value = match &v.value {
        serde_json::Value::Bool(b) => Some(Value::BoolValue(*b)),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::IntValue)
            .or_else(|| n.as_f64().map(Value::DoubleValue)),
        serde_json::Value::String(s) => Some(Value::StringValue(s.clone())),
        other => Some(Value::JsonValue(other.to_string())),
    };
    stitchd_proto::flags::v1::Variant {
        key: v.key,
        value: Some(VariantValue { value }),
    }
}

fn flag_value_type_str(t: i32) -> &'static str {
    use stitchd_proto::flags::v1::FlagValueType;
    match FlagValueType::try_from(t).unwrap_or(FlagValueType::Unspecified) {
        FlagValueType::Bool => "bool",
        FlagValueType::Int => "int",
        FlagValueType::Double => "double",
        FlagValueType::String => "string",
        FlagValueType::Json => "json",
        FlagValueType::Unspecified => "unspecified",
    }
}

fn flag_to_admin_json(f: &FeatureFlag) -> AdminFlagJson {
    let variants = f
        .variants
        .iter()
        .map(|v| VariantJson {
            key: v.key.clone(),
            value: proto_variant_value_to_json(v.value.clone()),
        })
        .collect();

    let rules = f.rules.iter().map(flag_rule_to_json).collect();

    let created_at = if f.created_at_ms != 0 {
        chrono::DateTime::from_timestamp_millis(f.created_at_ms)
            .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
    } else {
        None
    };
    let updated_at = if f.updated_at_ms != 0 {
        chrono::DateTime::from_timestamp_millis(f.updated_at_ms)
            .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
    } else {
        None
    };

    AdminFlagJson {
        flag_id: f.flag_id.clone(),
        key: f.key.clone(),
        name: f.name.clone(),
        description: f.description.clone(),
        flag_type: flag_value_type_str(f.value_type).to_string(),
        enabled: f.enabled,
        status: if f.archived { "archived" } else { "active" }.to_string(),
        version: f.version,
        variants,
        rules,
        default_variant_key: if f.default_variant_key.is_empty() {
            None
        } else {
            Some(f.default_variant_key.clone())
        },
        created_at,
        updated_at,
        locked_by_experiment_id: if f.locked_by_experiment_id.is_empty() {
            None
        } else {
            Some(f.locked_by_experiment_id.clone())
        },
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `GET /v1/projects/{project_id}/flags`
#[utoipa::path(
    get,
    path = "/v1/projects/{project_id}/flags",
    tag = "flags",
    params(("project_id" = String, Path, description = "Project / environment ID")),
    responses(
        (status = 200, description = "Paginated list of flags"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_flags(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
    Query(query): Query<ListFlagsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let pagination = &query.pagination;
    let req = tonic::Request::new(ListFlagsRequest {
        environment_id: String::new(),
        project_id,
        include_archived: query.include_archived,
        page: pagination.effective_page(),
        per_page: pagination.effective_per_page(),
    });
    let mut client = state.flag_client.lock().await;
    let inner = client
        .list_flags(req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    let items: Vec<AdminFlagJson> = inner.flags.iter().map(flag_to_admin_json).collect();
    let total = inner.total;
    Ok(Json(PaginatedResponse::new(items, total, pagination)))
}

/// `POST /v1/projects/{project_id}/flags`
#[utoipa::path(
    post,
    path = "/v1/projects/{project_id}/flags",
    tag = "flags",
    params(("project_id" = String, Path, description = "Project / environment ID")),
    request_body = FlagMutateRequest,
    responses(
        (status = 201, description = "Flag created", body = AdminFlagJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_flag(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
    Json(body): Json<FlagMutateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    let proto_value_type = body
        .value_type
        .as_deref()
        .map(parse_value_type)
        .unwrap_or(stitchd_proto::flags::v1::FlagValueType::Bool);
    let variant_list = body.variants.unwrap_or_default();
    if let Some(err) = validate_variant_values(&variant_list, proto_value_type) {
        return Err(GatewayError::BadRequest(err));
    }
    let variants = variant_list
        .into_iter()
        .map(variant_body_to_proto)
        .collect();
    let flag = FeatureFlag {
        key: body.key.unwrap_or_default(),
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        enabled: body.enabled.unwrap_or(false),
        value_type: proto_value_type as i32,
        variants,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: String::new(),
        project_id,
        kind: MutationKind::Create as i32,
        flag: Some(flag),
        version: 0,
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    Ok((StatusCode::CREATED, Json(flag_json)))
}

/// `GET /v1/projects/{project_id}/flags/{flag_id}`
#[utoipa::path(
    get,
    path = "/v1/projects/{project_id}/flags/{flag_id}",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project / environment ID"),
        ("flag_id" = String, Path, description = "Flag key"),
    ),
    responses(
        (status = 200, description = "Flag", body = AdminFlagJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Flag not found"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetFlagRequest {
        environment_id: String::new(),
        project_id,
        flag_key,
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.get_flag(req).await.map_err(GatewayError::from)?;
    Ok(Json(flag_to_admin_json(&resp.into_inner())))
}

/// `PUT /v1/projects/{project_id}/flags/{flag_id}`
#[utoipa::path(
    put,
    path = "/v1/projects/{project_id}/flags/{flag_id}",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project / environment ID"),
        ("flag_id" = String, Path, description = "Flag key"),
    ),
    request_body = FlagMutateRequest,
    responses(
        (status = 200, description = "Updated flag", body = AdminFlagJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<FlagMutateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    // When `enabled` is omitted, preserve the current value by fetching first.
    let current_enabled = if body.enabled.is_none() {
        let get_req = tonic::Request::new(GetFlagRequest {
            environment_id: String::new(),
            project_id: project_id.clone(),
            flag_key: flag_key.clone(),
        });
        let mut client = state.flag_client.lock().await;
        client
            .get_flag(get_req)
            .await
            .ok()
            .map(|r| r.into_inner().enabled)
            .unwrap_or(true)
    } else {
        body.enabled.unwrap_or(true)
    };
    let flag = FeatureFlag {
        key: flag_key,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        enabled: current_enabled,
        default_variant_key: body.default_variant_key.unwrap_or_default(),
        value_type: body
            .value_type
            .as_deref()
            .map(parse_value_type)
            .unwrap_or(stitchd_proto::flags::v1::FlagValueType::Unspecified)
            as i32,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: String::new(),
        project_id,
        kind: MutationKind::Update as i32,
        flag: Some(flag),
        version: body.version.unwrap_or(0),
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    Ok(Json(flag_json))
}

/// `DELETE /v1/projects/{project_id}/flags/{flag_id}`
#[utoipa::path(
    delete,
    path = "/v1/projects/{project_id}/flags/{flag_id}",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project / environment ID"),
        ("flag_id" = String, Path, description = "Flag key"),
    ),
    responses(
        (status = 204, description = "Flag deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let flag = FeatureFlag {
        key: flag_key,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: String::new(),
        project_id,
        kind: MutationKind::Delete as i32,
        flag: Some(flag),
        version: 0,
    });
    let mut client = state.flag_client.lock().await;
    client.mutate_flag(req).await.map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/projects/{project_id}/flags/{flag_id}/archive`
#[utoipa::path(
    post,
    path = "/v1/projects/{project_id}/flags/{flag_id}/archive",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project / environment ID"),
        ("flag_id" = String, Path, description = "Flag key"),
    ),
    request_body = FlagMutateRequest,
    responses(
        (status = 200, description = "Flag archived", body = AdminFlagJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Flag not found"),
        (status = 409, description = "Version conflict"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn archive_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<FlagMutateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    let flag = FeatureFlag {
        key: flag_key,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: String::new(),
        project_id,
        kind: MutationKind::Archive as i32,
        flag: Some(flag),
        version: body.version.unwrap_or(0),
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    Ok(Json(flag_json))
}

/// `POST /v1/projects/{project_id}/flags/{flag_id}/restore`
pub async fn restore_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<FlagMutateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    let flag = FeatureFlag {
        key: flag_key,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: String::new(),
        project_id,
        kind: MutationKind::Restore as i32,
        flag: Some(flag),
        version: body.version.unwrap_or(0),
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    Ok(Json(flag_json))
}

/// Request body for replacing a flag's variant list.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReplaceVariantsBody {
    pub variants: Vec<VariantBody>,
    pub version: u64,
}

/// `PUT /v1/projects/{project_id}/flags/{flag_id}/variants`
#[utoipa::path(
    put,
    path = "/v1/projects/{project_id}/flags/{flag_id}/variants",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project / environment ID"),
        ("flag_id" = String, Path, description = "Flag key"),
    ),
    request_body = ReplaceVariantsBody,
    responses(
        (status = 200, description = "Updated flag with new variants", body = AdminFlagJson),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Version conflict"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_variants(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<ReplaceVariantsBody>,
) -> Result<impl IntoResponse, GatewayError> {
    // First fetch the current flag to get its metadata (enabled, name, etc.).
    let get_req = tonic::Request::new(GetFlagRequest {
        environment_id: String::new(),
        project_id: project_id.clone(),
        flag_key: flag_key.clone(),
    });
    let mut client = state.flag_client.lock().await;
    let current = client
        .get_flag(get_req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();

    // Boolean flags: only variant keys (names) may change; values must stay true/false.
    if current.value_type == (stitchd_proto::flags::v1::FlagValueType::Bool as i32) {
        if body.variants.len() != 2 {
            return Err(GatewayError::BadRequest(
                "Boolean flags must have exactly 2 variants".to_string(),
            ));
        }
        let has_true = body
            .variants
            .iter()
            .any(|v| matches!(v.value, serde_json::Value::Bool(true)));
        let has_false = body
            .variants
            .iter()
            .any(|v| matches!(v.value, serde_json::Value::Bool(false)));
        if !has_true || !has_false {
            return Err(GatewayError::BadRequest(
                "Boolean flag variants must have values true and false".to_string(),
            ));
        }
    }

    // Validate values match the flag's declared type.
    let declared_type = stitchd_proto::flags::v1::FlagValueType::try_from(current.value_type)
        .unwrap_or(stitchd_proto::flags::v1::FlagValueType::Unspecified);
    if let Some(err) = validate_variant_values(&body.variants, declared_type) {
        return Err(GatewayError::BadRequest(err));
    }

    // Build an Update mutation carrying the new variant list.
    let proto_variants = body
        .variants
        .into_iter()
        .map(variant_body_to_proto)
        .collect();
    let flag = FeatureFlag {
        key: flag_key,
        enabled: current.enabled,
        name: current.name,
        description: current.description,
        value_type: current.value_type,
        variants: proto_variants,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: String::new(),
        project_id,
        kind: MutationKind::Update as i32,
        flag: Some(flag),
        version: body.version,
    });
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    Ok(Json(flag_json))
}

/// A single rule in a replace-rules request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RuleBody {
    /// Optional `feature_flag_rules.id` UUID. When the client preserves the
    /// rule's existing UUID across an edit the backend round-trips it through;
    /// otherwise the DB-side `gen_random_uuid()` mints a fresh one.
    #[serde(default)]
    pub rule_id: Option<String>,
    /// Optional human-readable label; ignored by the evaluator.
    pub name: Option<String>,
    /// ConditionExpr as a JSON value.
    pub condition: serde_json::Value,
    /// Output:
    /// - `{"variant_key": "..."}`
    /// - `{"allocation": {"hash_inputs": [{"kind":"context_key","context_type":"user"}], "buckets": [...]}}`
    ///
    /// The new `hash_inputs` selector list (Phase 4 of
    /// `flag_eval_unify_20260522`) is the preferred wire shape. The legacy
    /// `hash_targets` array remains accepted on input for backwards
    /// compatibility but is no longer the authoritative source — see
    /// `extract_hash_inputs_from_allocation`.
    pub output: serde_json::Value,
}

/// Request body for replacing a flag's rule list.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReplaceRulesBody {
    pub rules: Vec<RuleBody>,
    pub version: u64,
}

/// Extract `hash_inputs` from a rule's `allocation` JSON value.
///
/// Recognises two shapes (in order of preference):
/// 1. `"hash_inputs": [{"kind": "context_key", ...}, ...]` — the new
///    Phase 4 (`flag_eval_unify_20260522`) wire shape.
/// 2. `"hash_targets": [{"context_type": "user", "field": "key"}, ...]` —
///    legacy shape used by the admin UI before Phase 4. Each `field == "key"`
///    becomes a `ContextKey` selector; otherwise the value is treated as a
///    parameter name on the named context type.
///
/// Returns `None` when neither field is present; the caller decides whether
/// to default (legacy behaviour: `user.key`) or 400.
fn extract_hash_inputs_from_allocation(alloc: &serde_json::Value) -> Option<Vec<HashSelectorJson>> {
    if let Some(arr) = alloc.get("hash_inputs").and_then(|v| v.as_array()) {
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let selector: HashSelectorJson = serde_json::from_value(item.clone()).ok()?;
            out.push(selector);
        }
        return Some(out);
    }
    if let Some(targets) = alloc.get("hash_targets").and_then(|v| v.as_array()) {
        let mut out = Vec::with_capacity(targets.len());
        for target in targets {
            let ctx_type = target
                .get("context_type")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let field = target
                .get("field")
                .and_then(|v| v.as_str())
                .unwrap_or("key");
            out.push(if field == "key" {
                HashSelectorJson::ContextKey {
                    context_type: ctx_type,
                }
            } else {
                HashSelectorJson::ContextParameter {
                    context_type: ctx_type,
                    parameter: field.to_string(),
                }
            });
        }
        return Some(out);
    }
    None
}

/// Convert a single `RuleBody` JSON to its proto representation. Validates
/// `hash_inputs` before populating the proto. Returns a structured error
/// when validation fails so the caller can map to HTTP 400.
fn rule_body_to_proto(
    r: RuleBody,
    index: usize,
) -> Result<stitchd_proto::flags::v1::FlagRule, String> {
    use stitchd_proto::flags::v1::{AllocationBucket, PercentageAllocation, flag_rule::Output};

    let rule_payload = serde_json::to_vec(&r.condition).unwrap_or_default();

    let output = if let Some(key) = r.output.get("variant_key").and_then(|v| v.as_str()) {
        Some(Output::VariantKey(key.to_string()))
    } else if let Some(alloc_val) = r.output.get("allocation") {
        // Legacy bare-array shape: `{"allocation": [...]}` — treated as user.key.
        let (selectors, buckets_val) = if alloc_val.is_array() {
            (
                vec![HashSelectorJson::ContextKey {
                    context_type: "user".to_string(),
                }],
                alloc_val.clone(),
            )
        } else {
            let buckets_val = alloc_val
                .get("buckets")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            // Prefer `hash_inputs` when present; fall back to `hash_targets`
            // (legacy admin UI shape). If neither is present, default to
            // single `user.key` selector (preserves pre-Phase-4 behaviour).
            let selectors = extract_hash_inputs_from_allocation(alloc_val).unwrap_or_else(|| {
                vec![HashSelectorJson::ContextKey {
                    context_type: "user".to_string(),
                }]
            });
            (selectors, buckets_val)
        };

        // Validate the selector list (FR-8 of flag_eval_unify_20260522).
        validate_hash_inputs(&selectors).map_err(|e| format!("rule[{index}]: {e}"))?;

        let buckets: Vec<AllocationBucket> = buckets_val
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| {
                Some(AllocationBucket {
                    variant_key: b.get("variant_key")?.as_str()?.to_string(),
                    weight_milli: b.get("weight_milli")?.as_u64()? as u32,
                })
            })
            .collect();

        let proto_hash_inputs: Vec<_> = selectors.iter().map(HashSelectorJson::to_proto).collect();

        Some(Output::Allocation(PercentageAllocation {
            context_hash_specs: Default::default(),
            buckets,
            hash_inputs: proto_hash_inputs,
        }))
    } else {
        None
    };

    let _ = index; // index tracked by the caller for logging if needed
    Ok(stitchd_proto::flags::v1::FlagRule {
        rule_payload,
        output,
        name: r.name.unwrap_or_default(),
        rule_id: r.rule_id.unwrap_or_default(),
    })
}

/// `PUT /v1/projects/{project_id}/flags/{flag_id}/rules`
#[utoipa::path(
    put,
    path = "/v1/projects/{project_id}/flags/{flag_id}/rules",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project / environment ID"),
        ("flag_id" = String, Path, description = "Flag key"),
    ),
    request_body = ReplaceRulesBody,
    responses(
        (status = 200, description = "Updated flag with new rules", body = AdminFlagJson),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Version conflict"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_rules(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<ReplaceRulesBody>,
) -> Result<impl IntoResponse, GatewayError> {
    // Validate the rule list BEFORE touching the upstream. `hash_inputs` and
    // legacy `hash_targets` shapes both run through `validate_hash_inputs`.
    // Validation errors surface as HTTP 400 (Phase 4 / FR-8 of
    // `flag_eval_unify_20260522`).
    let proto_rules: Vec<_> = body
        .rules
        .into_iter()
        .enumerate()
        .map(|(i, r)| rule_body_to_proto(r, i))
        .collect::<Result<Vec<_>, _>>()
        .map_err(GatewayError::BadRequest)?;

    // Fetch current flag to carry over metadata.
    let get_req = tonic::Request::new(GetFlagRequest {
        environment_id: String::new(),
        project_id: project_id.clone(),
        flag_key: flag_key.clone(),
    });
    let mut client = state.flag_client.lock().await;
    let current = client
        .get_flag(get_req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();

    let flag = FeatureFlag {
        key: flag_key,
        enabled: current.enabled,
        name: current.name,
        description: current.description,
        value_type: current.value_type,
        rules: proto_rules,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: String::new(),
        project_id,
        kind: MutationKind::Update as i32,
        flag: Some(flag),
        version: body.version,
    });
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    Ok(Json(flag_json))
}

/// `PUT /v1/projects/{project_id}/flags/{flag_id}/hashing`
#[utoipa::path(
    put,
    path = "/v1/projects/{project_id}/flags/{flag_id}/hashing",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project / environment ID"),
        ("flag_id" = String, Path, description = "Flag key"),
    ),
    request_body = UpdateHashingBody,
    responses(
        (status = 200, description = "Updated hashing configuration", body = UpdateHashingResponse),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_flag_hashing(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<UpdateHashingBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let configs: Vec<FlagHashingConfig> = body
        .configs
        .into_iter()
        .map(|c| FlagHashingConfig {
            parameter_key: c.parameter_key,
            parameter_type: c.parameter_type,
            order: c.order,
        })
        .collect();
    let req = tonic::Request::new(UpdateFlagHashingRequest {
        environment_id: project_id,
        flag_key,
        configs,
    });
    let mut client = state.flag_client.lock().await;
    let resp = client
        .update_flag_hashing(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    let configs_json: Vec<HashingConfigJson> = inner
        .configs
        .iter()
        .map(|c| HashingConfigJson {
            parameter_key: c.parameter_key.clone(),
            parameter_type: c.parameter_type.clone(),
            order: c.order,
        })
        .collect();
    Ok(Json(UpdateHashingResponse {
        flag: flag_json,
        configs: configs_json,
    }))
}

// ─── Default-rule distribution (Phase 7 Task 5) ───────────────────────────────

/// Single allocation entry in the default-rule distribution body.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct DefaultRuleAllocationBody {
    pub variant_key: String,
    pub percentage: f64,
}

/// Request body for `POST /v1/projects/{project_id}/flags/{flag_key}/default-rule-distribution`.
///
/// `None` / empty `distribution.allocations` clears the distribution (reverts
/// the default-rule path to single-default-variant behaviour). When `Some`,
/// the server validates the distribution via
/// `RolloutDistribution::validate` (allocations non-empty, each percentage in
/// `(0, 100]`, no duplicate variant keys, sum ≈ 100.0).
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetDefaultRuleDistributionBody {
    /// `None` or empty `allocations` clears the distribution.
    #[serde(default)]
    pub distribution: Option<DefaultRuleDistributionBody>,
    /// Optimistic-locking version (matches the flag's current `version`).
    pub version: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DefaultRuleDistributionBody {
    pub allocations: Vec<DefaultRuleAllocationBody>,
    /// Optional ordered selector list driving the default-rule percentage
    /// hash. Mirrors the `hash_inputs` field on percentage-allocation rule
    /// outputs. When omitted, the server falls back to the flag's
    /// `default_rule_hash_inputs` column (Phase 5/6 wiring); for now, the
    /// gateway validates the shape but does not yet plumb it through the
    /// `SetDefaultRuleDistribution` RPC (proto-level cutover lands in
    /// Phase 5/6 of `flag_eval_unify_20260522`).
    #[serde(default)]
    pub hash_inputs: Option<Vec<HashSelectorJson>>,
}

/// Response body — echoes the updated flag and its new version.
#[derive(Debug, Serialize, ToSchema)]
pub struct SetDefaultRuleDistributionResponseJson {
    pub flag: AdminFlagJson,
    pub version: u64,
}

/// `POST /v1/projects/{project_id}/flags/{flag_key}/default-rule-distribution`
///
/// Sets (or clears) the flag's percentage distribution for the default-rule
/// fall-through. Returns 409 when the flag is locked by a running/paused
/// experiment, 422 for invalid distributions.
#[utoipa::path(
    post,
    path = "/v1/projects/{project_id}/flags/{flag_key}/default-rule-distribution",
    tag = "flags",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("flag_key" = String, Path, description = "Flag key"),
    ),
    request_body = SetDefaultRuleDistributionBody,
    responses(
        (status = 200, description = "Distribution updated", body = SetDefaultRuleDistributionResponseJson),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Flag locked by an experiment"),
        (status = 422, description = "Invalid distribution"),
        (status = 502, description = "Flag service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn set_default_rule_distribution(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<SetDefaultRuleDistributionBody>,
) -> Result<impl IntoResponse, GatewayError> {
    use stitchd_proto::flags::v1::{DefaultRuleAllocation, SetDefaultRuleDistributionRequest};

    // Validate the optional `hash_inputs` selector list BEFORE touching the
    // upstream. Phase 4 / FR-8 of `flag_eval_unify_20260522`. (The proto-
    // level plumbing of `hash_inputs` through `SetDefaultRuleDistribution`
    // lands in Phase 5/6 — for now the gateway validates the shape and
    // discards it.)
    if let Some(dist) = body.distribution.as_ref()
        && let Some(selectors) = dist.hash_inputs.as_ref()
    {
        validate_hash_inputs(selectors).map_err(GatewayError::BadRequest)?;
    }

    let allocations: Vec<DefaultRuleAllocation> = body
        .distribution
        .map(|d| {
            d.allocations
                .into_iter()
                .map(|a| DefaultRuleAllocation {
                    variant_key: a.variant_key,
                    percentage: a.percentage,
                })
                .collect()
        })
        .unwrap_or_default();

    let req = tonic::Request::new(SetDefaultRuleDistributionRequest {
        project_id,
        flag_key,
        allocations,
        version: body.version,
    });

    let mut client = state.flag_client.lock().await;
    let resp = match client.set_default_rule_distribution(req).await {
        Ok(r) => r,
        Err(s) => {
            // The flag-service returns `INVALID_ARGUMENT` with a message
            // prefixed `invalid_distribution:` when the distribution fails
            // validation. The gateway rewrites that into a structured 422
            // body the admin UI can branch on. Lock-precondition errors
            // are already handled by `GatewayError::from(tonic::Status)`.
            if s.code() == tonic::Code::InvalidArgument
                && s.message().starts_with("invalid_distribution:")
            {
                return Err(GatewayError::InvalidDistribution(
                    s.message()
                        .trim_start_matches("invalid_distribution:")
                        .trim()
                        .to_string(),
                ));
            }
            return Err(GatewayError::from(s));
        }
    };
    let inner = resp.into_inner();
    let flag_json = inner
        .flag
        .as_ref()
        .map(flag_to_admin_json)
        .unwrap_or_else(|| flag_to_admin_json(&FeatureFlag::default()));
    Ok(Json(SetDefaultRuleDistributionResponseJson {
        flag: flag_json,
        version: inner.version,
    }))
}

// ─── Evaluate preview ─────────────────────────────────────────────────────────

/// Request body for `POST /v1/projects/{project_id}/flags/{flag_key}/evaluate-preview`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EvaluatePreviewBody {
    /// JSON array of `EvaluationContext` objects.
    pub contexts: Vec<serde_json::Value>,
    /// Optional environment ID for eval-log scoping. Empty string means unknown.
    #[serde(default)]
    pub environment_id: String,
}

/// A single per-context evaluation result returned by the preview endpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewResultJson {
    pub context_index: usize,
    /// Primary key from the first sub-context in the evaluation context.
    pub context_key: String,
    pub variant_key: String,
    pub variant_value: serde_json::Value,
    /// True when the flag itself is disabled (default rule fired for all contexts).
    pub disabled: bool,
    pub fired_rule_index: Option<usize>,
    pub fired_rule_name: Option<String>,
    pub rule_traces: Vec<RuleTraceJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollout_debug: Option<RolloutDebugJson>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RuleTraceJson {
    pub rule_index: usize,
    pub rule_name: Option<String>,
    pub outcome: String,
    pub conditions: Vec<ConditionTraceJson>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConditionTraceJson {
    pub predicate: String,
    pub result: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RolloutDebugJson {
    pub hash_input: String,
    pub bucket: u32,
    pub variant_ranges: Vec<VariantRangeJson>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VariantRangeJson {
    pub variant_key: String,
    pub from: u32,
    pub to: u32,
}

/// Response body for the evaluate-preview endpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct EvaluatePreviewResponse {
    pub flag_enabled: bool,
    pub results: Vec<PreviewResultJson>,
}

pub async fn evaluate_preview(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<EvaluatePreviewBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let environment_id = body.environment_id;
    // Translate simplified UI format → EvaluationContext format.
    //
    // UI sends a FLAT list of sub-contexts:
    //   [{"_type":"user","key":"alice","parameters":{...}},
    //    {"_type":"device","key":"d42","parameters":{...}}, ...]
    //
    // Bug fix `feature-flag-utp`: the entire flat list represents ONE
    // bundle (one EvaluationContext containing multiple sub-contexts) —
    // not N independent single-sub-context bundles. This preserves
    // cross-context hashing in preview (FR-2 of `flag_eval_unify_20260522`):
    // a percentage rule whose `hash_inputs` mix `user.key`, `device.os`,
    // and `application.key` must resolve against the FULL bundle.
    //
    // The core `evaluate_preview` emits one `ContextPreviewResult` per
    // sub-context in the bundle, so the UI still gets a per-context row
    // (one row per input sub-context, all sharing the same hash_input).
    //
    // Backward compat: if a caller passes already-shaped EvaluationContext
    // objects (each with its own `"contexts"` array), each becomes its own
    // bundle. We split UI-shape items from EC-shape items and combine the
    // UI items into a single bundle, preserving EC items as-is.
    let mut ui_subcontexts: Vec<serde_json::Value> = Vec::new();
    let mut explicit_bundles: Vec<serde_json::Value> = Vec::new();
    for item in body.contexts.into_iter() {
        if item.get("contexts").is_some() {
            // Already in EvaluationContext shape — preserve as an
            // independent bundle.
            explicit_bundles.push(item);
        } else {
            // Simplified shape — flatten into the shared bundle.
            let context_type = item
                .get("_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let key = item
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let parameters = item
                .get("parameters")
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            ui_subcontexts.push(serde_json::json!({
                "context_type": context_type,
                "key": key,
                "parameters": parameters,
                "private_parameters": []
            }));
        }
    }

    let mut evaluation_contexts: Vec<serde_json::Value> = Vec::new();
    if !ui_subcontexts.is_empty() {
        evaluation_contexts.push(serde_json::json!({ "contexts": ui_subcontexts }));
    }
    evaluation_contexts.extend(explicit_bundles);

    let contexts_json = serde_json::to_string(&evaluation_contexts)
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;

    let req = tonic::Request::new(EvaluatePreviewRequest {
        project_id,
        flag_key,
        contexts_json,
        environment_id,
    });
    let mut client = state.flag_client.lock().await;
    let resp = client
        .evaluate_preview(req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();

    let flag_enabled = resp.flag_enabled;

    let raw_results: Vec<serde_json::Value> = serde_json::from_str(&resp.results_json)
        .map_err(|e| GatewayError::Upstream(e.to_string()))?;

    // Flatten the input evaluation_contexts into a single list of sub-contexts
    // in iteration order — `context_index` on each result is a GLOBAL
    // sub-context index across all input EvaluationContext bundles (see
    // `feature-flag-utp` fix in core `evaluate_preview`).
    let flat_subcontexts: Vec<&serde_json::Value> = evaluation_contexts
        .iter()
        .flat_map(|ec| {
            ec["contexts"]
                .as_array()
                .map(|arr| arr.iter().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();

    let results: Vec<PreviewResultJson> = raw_results
        .into_iter()
        .map(|v| {
            let context_index = v["context_index"].as_u64().unwrap_or(0) as usize;
            // Extract context_key from the flat sub-context list at the
            // matching global index. Falls back to empty for out-of-range
            // (defensive — should not happen).
            let context_key = flat_subcontexts
                .get(context_index)
                .and_then(|c| c["key"].as_str())
                .unwrap_or("")
                .to_string();
            let variant_key = v["variant_key"].as_str().unwrap_or("").to_string();
            let variant_value = v["variant_value"].clone();
            let fired_rule_index = v["fired_rule_index"].as_u64().map(|n| n as usize);
            let fired_rule_name = v["fired_rule_name"].as_str().map(str::to_string);
            let rule_traces = v["rule_traces"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|t| RuleTraceJson {
                            rule_index: t["rule_index"].as_u64().unwrap_or(0) as usize,
                            rule_name: t["rule_name"].as_str().map(str::to_string),
                            outcome: t["outcome"].as_str().unwrap_or("no_match").to_string(),
                            conditions: t["conditions"]
                                .as_array()
                                .map(|cs| {
                                    cs.iter()
                                        .map(|c| ConditionTraceJson {
                                            predicate: c["predicate"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string(),
                                            result: c["result"].as_bool().unwrap_or(false),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let rollout_debug = v.get("rollout_debug").and_then(|rd| {
                if rd.is_null() {
                    None
                } else {
                    Some(RolloutDebugJson {
                        hash_input: rd["hash_input"].as_str().unwrap_or("").to_string(),
                        bucket: rd["bucket"].as_u64().unwrap_or(0) as u32,
                        variant_ranges: rd["variant_ranges"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .map(|r| VariantRangeJson {
                                        variant_key: r["variant_key"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string(),
                                        from: r["from"].as_u64().unwrap_or(0) as u32,
                                        to: r["to"].as_u64().unwrap_or(0) as u32,
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                }
            });
            PreviewResultJson {
                context_index,
                context_key,
                variant_key,
                variant_value,
                disabled: !flag_enabled,
                fired_rule_index,
                fired_rule_name,
                rule_traces,
                rollout_debug,
            }
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(EvaluatePreviewResponse {
            flag_enabled: resp.flag_enabled,
            results,
        }),
    ))
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

/// Build a minimal router for unit testing.
#[cfg(test)]
pub fn test_router(_client: Arc<GatewayState>, state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]
    use axum::routing::{delete, get, post, put};
    let _ = _client;
    axum::Router::new()
        .route(
            "/v1/projects/{project_id}/flags",
            get(list_flags).post(create_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}",
            get(get_flag).put(update_flag).delete(delete_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/archive",
            post(archive_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/restore",
            post(restore_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/variants",
            put(update_variants),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/rules",
            put(update_rules),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/hashing",
            put(update_flag_hashing),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_key}/default-rule-distribution",
            post(set_default_rule_distribution),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/evaluate-preview",
            post(evaluate_preview),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    use crate::tests::helpers::{make_stub_state, make_stub_state_with_flag};

    #[tokio::test]
    async fn list_flags_returns_200() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/projects/env-1/flags")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Stub returns NotFound for list — maps to 404, but empty list is 200
        // The stub returns empty flags → 200.
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn get_flag_not_found_returns_404_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/projects/env-1/flags/missing-flag")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // gRPC connection refused → 502 or flag not found → 404
        assert!(
            resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_flag_returns_201_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/projects/env-1/flags")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"key":"my-flag","enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_flag_returns_204_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/projects/env-1/flags/my-flag")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn update_variants_returns_200_or_404_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/projects/env-1/flags/flag-key/variants")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"variants":[{"key":"on","value":true}],"version":1}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn update_rules_returns_200_or_404_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/projects/env-1/flags/flag-key/rules")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"rules":[],"version":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn update_flag_hashing_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/projects/env-1/flags/flag-key/hashing")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"configs":[{"parameter_key":"user_id","parameter_type":"string","order":0}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn update_flag_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/projects/env-1/flags/my-flag")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"enabled":false,"version":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    // Keeps the compiler happy — make_stub_state_with_flag exported for other tests
    #[allow(dead_code)]
    fn _use_with_flag() {
        let _ = make_stub_state_with_flag;
    }

    #[tokio::test]
    async fn archive_flag_returns_200_or_404_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/projects/env-1/flags/my-flag/archive")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"version":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[test]
    fn flag_to_admin_json_maps_all_fields() {
        use stitchd_proto::flags::v1::{FlagValueType, VariantValue, variant_value::Value};

        let flag = FeatureFlag {
            key: "my-flag".to_string(),
            enabled: true,
            value_type: FlagValueType::Bool as i32,
            name: "My Flag".to_string(),
            description: "A test flag".to_string(),
            flag_id: "abc-123".to_string(),
            version: 3,
            default_variant_key: "on".to_string(),
            created_at_ms: 1_000_000,
            updated_at_ms: 2_000_000,
            archived: false,
            variants: vec![stitchd_proto::flags::v1::Variant {
                key: "on".to_string(),
                value: Some(VariantValue {
                    value: Some(Value::BoolValue(true)),
                }),
            }],
            rules: vec![],
            locked_by_experiment_id: String::new(),
        };

        let admin = flag_to_admin_json(&flag);

        assert_eq!(admin.flag_id, "abc-123");
        assert_eq!(admin.key, "my-flag");
        assert_eq!(admin.name, "My Flag");
        assert_eq!(admin.description, "A test flag");
        assert_eq!(admin.flag_type, "bool");
        assert!(admin.enabled);
        assert_eq!(admin.status, "active");
        assert_eq!(admin.version, 3);
        assert_eq!(admin.default_variant_key, Some("on".to_string()));
        assert_eq!(admin.variants.len(), 1);
        assert_eq!(admin.variants[0].key, "on");
        assert_eq!(admin.variants[0].value, serde_json::Value::Bool(true));
        assert!(admin.created_at.is_some());
        assert!(admin.updated_at.is_some());
        // Empty proto field means we omit the JSON `locked_by_experiment_id`
        // key entirely (skip_serializing_if = Option::is_none).
        assert!(admin.locked_by_experiment_id.is_none());
    }

    #[test]
    fn flag_to_admin_json_surfaces_rule_id_name_and_lock_state() {
        use stitchd_proto::flags::v1::{FlagRule, flag_rule::Output};

        let exp_uuid = "11111111-2222-3333-4444-555555555555";
        let rule_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

        let flag = FeatureFlag {
            key: "k".to_string(),
            flag_id: "fid".to_string(),
            rules: vec![FlagRule {
                rule_payload: b"null".to_vec(),
                output: Some(Output::VariantKey("on".to_string())),
                name: "premium-cohort".to_string(),
                rule_id: rule_uuid.to_string(),
            }],
            locked_by_experiment_id: exp_uuid.to_string(),
            ..Default::default()
        };

        let admin = flag_to_admin_json(&flag);
        assert_eq!(admin.locked_by_experiment_id.as_deref(), Some(exp_uuid));
        assert_eq!(admin.rules.len(), 1);
        assert_eq!(admin.rules[0].rule_id, rule_uuid);
        assert_eq!(admin.rules[0].name.as_deref(), Some("premium-cohort"));
    }

    #[test]
    fn flag_to_admin_json_archived_status() {
        let flag = FeatureFlag {
            key: "archived-flag".to_string(),
            archived: true,
            ..Default::default()
        };
        let admin = flag_to_admin_json(&flag);
        assert_eq!(admin.status, "archived");
    }

    // ─── Phase 4 (flag_eval_unify_20260522): hash_inputs validation ───────────

    /// Helper: PUT a rules-replace body and return the response status code.
    async fn put_rules_status(body: &str) -> StatusCode {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/projects/env-1/flags/flag-key/rules")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        resp.status()
    }

    /// Happy path — a valid `hash_inputs` payload SHOULD bypass the validator
    /// (returns OK / NOT_FOUND / BAD_GATEWAY depending on stub behaviour).
    /// MUST NOT return 400.
    #[tokio::test]
    async fn update_rules_accepts_hash_inputs_happy_path() {
        let body = r#"{
            "rules": [{
                "condition": null,
                "output": {
                    "allocation": {
                        "hash_inputs": [
                            {"kind":"context_key","context_type":"user"},
                            {"kind":"context_parameter","context_type":"device","parameter":"os"}
                        ],
                        "buckets": [
                            {"variant_key":"on","weight_milli":500},
                            {"variant_key":"off","weight_milli":500}
                        ]
                    }
                }
            }],
            "version": 1
        }"#;
        let status = put_rules_status(body).await;
        assert_ne!(
            status,
            StatusCode::BAD_REQUEST,
            "happy-path body must not 400 (got {status})"
        );
    }

    #[tokio::test]
    async fn update_rules_rejects_empty_hash_inputs() {
        let body = r#"{
            "rules": [{
                "condition": null,
                "output": {
                    "allocation": {
                        "hash_inputs": [],
                        "buckets": [{"variant_key":"on","weight_milli":1000}]
                    }
                }
            }],
            "version": 1
        }"#;
        let status = put_rules_status(body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "empty hash_inputs must return 400"
        );
    }

    #[tokio::test]
    async fn update_rules_rejects_duplicate_selectors() {
        let body = r#"{
            "rules": [{
                "condition": null,
                "output": {
                    "allocation": {
                        "hash_inputs": [
                            {"kind":"context_key","context_type":"user"},
                            {"kind":"context_key","context_type":"user"}
                        ],
                        "buckets": [{"variant_key":"on","weight_milli":1000}]
                    }
                }
            }],
            "version": 1
        }"#;
        let status = put_rules_status(body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "duplicate hash_inputs selectors must return 400"
        );
    }

    #[tokio::test]
    async fn update_rules_rejects_duplicate_parameter_selectors() {
        let body = r#"{
            "rules": [{
                "condition": null,
                "output": {
                    "allocation": {
                        "hash_inputs": [
                            {"kind":"context_parameter","context_type":"user","parameter":"plan"},
                            {"kind":"context_parameter","context_type":"user","parameter":"plan"}
                        ],
                        "buckets": [{"variant_key":"on","weight_milli":1000}]
                    }
                }
            }],
            "version": 1
        }"#;
        let status = put_rules_status(body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "duplicate parameter selectors must return 400"
        );
    }

    #[tokio::test]
    async fn update_rules_rejects_parameter_selector_missing_parameter() {
        // `kind: context_parameter` MUST have a non-empty `parameter` field.
        let body = r#"{
            "rules": [{
                "condition": null,
                "output": {
                    "allocation": {
                        "hash_inputs": [
                            {"kind":"context_parameter","context_type":"user","parameter":""}
                        ],
                        "buckets": [{"variant_key":"on","weight_milli":1000}]
                    }
                }
            }],
            "version": 1
        }"#;
        let status = put_rules_status(body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "context_parameter selector with empty parameter must return 400"
        );
    }

    /// Helper: POST a default-rule-distribution body and return the status.
    async fn post_default_rule_dist_status(body: &str) -> StatusCode {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/projects/env-1/flags/flag-key/default-rule-distribution")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        resp.status()
    }

    /// Happy path: default-rule distribution carrying `hash_inputs` is
    /// accepted (validator does not 400 it).
    #[tokio::test]
    async fn default_rule_distribution_accepts_hash_inputs_happy_path() {
        let body = r#"{
            "distribution": {
                "allocations": [
                    {"variant_key":"on","percentage":50.0},
                    {"variant_key":"off","percentage":50.0}
                ],
                "hash_inputs": [
                    {"kind":"context_key","context_type":"user"}
                ]
            },
            "version": 1
        }"#;
        let status = post_default_rule_dist_status(body).await;
        assert_ne!(
            status,
            StatusCode::BAD_REQUEST,
            "happy-path default-rule distribution body must not 400 (got {status})"
        );
    }

    #[tokio::test]
    async fn default_rule_distribution_rejects_empty_hash_inputs() {
        let body = r#"{
            "distribution": {
                "allocations": [{"variant_key":"on","percentage":100.0}],
                "hash_inputs": []
            },
            "version": 1
        }"#;
        let status = post_default_rule_dist_status(body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "default-rule with empty hash_inputs must return 400"
        );
    }

    #[tokio::test]
    async fn default_rule_distribution_rejects_duplicate_hash_inputs() {
        let body = r#"{
            "distribution": {
                "allocations": [{"variant_key":"on","percentage":100.0}],
                "hash_inputs": [
                    {"kind":"context_key","context_type":"user"},
                    {"kind":"context_key","context_type":"user"}
                ]
            },
            "version": 1
        }"#;
        let status = post_default_rule_dist_status(body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "default-rule with duplicate selectors must return 400"
        );
    }

    #[tokio::test]
    async fn default_rule_distribution_rejects_parameter_selector_missing_parameter() {
        let body = r#"{
            "distribution": {
                "allocations": [{"variant_key":"on","percentage":100.0}],
                "hash_inputs": [
                    {"kind":"context_parameter","context_type":"user","parameter":""}
                ]
            },
            "version": 1
        }"#;
        let status = post_default_rule_dist_status(body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "default-rule with empty parameter must return 400"
        );
    }
}
