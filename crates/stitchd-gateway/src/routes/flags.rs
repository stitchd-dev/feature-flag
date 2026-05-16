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
    /// Value type: "bool" | "int" | "double" | "string" | "json"
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

/// Lightweight JSON representation of a feature flag.
#[derive(Debug, Serialize, ToSchema)]
pub struct FlagJson {
    pub key: String,
    pub enabled: bool,
}

#[cfg(test)]
fn flag_to_json(f: &FeatureFlag) -> FlagJson {
    FlagJson {
        key: f.key.clone(),
        enabled: f.enabled,
    }
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
            // Serialise context_hash_specs → hash_targets array for the UI.
            // Each entry becomes { context_type, field } where field is "key"
            // (empty parameter_names) or the parameter name.
            let mut hash_targets: Vec<serde_json::Value> = Vec::new();
            for (ctx_type, spec) in &alloc.context_hash_specs {
                if spec.parameter_names.is_empty() {
                    hash_targets.push(serde_json::json!({
                        "context_type": ctx_type,
                        "field": "key"
                    }));
                } else {
                    for param in &spec.parameter_names {
                        hash_targets.push(serde_json::json!({
                            "context_type": ctx_type,
                            "field": param
                        }));
                    }
                }
            }
            // Default to user.key when no targets are stored (legacy data).
            if hash_targets.is_empty() {
                hash_targets.push(serde_json::json!({ "context_type": "user", "field": "key" }));
            }
            let buckets: Vec<_> = alloc
                .buckets
                .iter()
                .map(|b| serde_json::json!({ "variant_key": b.variant_key, "weight_milli": b.weight_milli }))
                .collect();
            serde_json::json!({ "allocation": { "hash_targets": hash_targets, "buckets": buckets } })
        }
        None => serde_json::Value::Null,
    };

    let mut segment_ids = Vec::new();
    collect_segment_ids(&condition, &mut segment_ids);
    // Deduplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    segment_ids.retain(|id| seen.insert(id.clone()));

    let name = if r.name.is_empty() { None } else { Some(r.name.clone()) };
    RuleJson {
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
    let inner = client.list_flags(req).await.map_err(GatewayError::from)?.into_inner();
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
    let flag = FeatureFlag {
        key: flag_key,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        enabled: body.enabled.unwrap_or(false),
        default_variant_key: body.default_variant_key.unwrap_or_default(),
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
    /// Optional human-readable label; ignored by the evaluator.
    pub name: Option<String>,
    /// ConditionExpr as a JSON value.
    pub condition: serde_json::Value,
    /// Output:
    /// - `{"variant_key": "..."}`
    /// - `{"allocation": {"hash_targets": [{"context_type": "user", "field": "key"}], "buckets": [...]}}`
    pub output: serde_json::Value,
}

/// Request body for replacing a flag's rule list.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReplaceRulesBody {
    pub rules: Vec<RuleBody>,
    pub version: u64,
}

fn rule_body_to_proto(r: RuleBody, index: usize) -> stitchd_proto::flags::v1::FlagRule {
    use stitchd_proto::flags::v1::{
        AllocationBucket, ContextHashSpec, PercentageAllocation, flag_rule::Output,
    };

    let rule_payload = serde_json::to_vec(&r.condition).unwrap_or_default();

    let output = if let Some(key) = r.output.get("variant_key").and_then(|v| v.as_str()) {
        Some(Output::VariantKey(key.to_string()))
    } else if let Some(alloc_val) = r.output.get("allocation") {
        // New format: { "allocation": { "hash_targets": [...], "buckets": [...] } }
        // Legacy format: { "allocation": [...] }  (bare array — treated as user.key)
        let (hash_targets_val, buckets_val) = if alloc_val.is_array() {
            (None, alloc_val)
        } else {
            (
                alloc_val.get("hash_targets"),
                alloc_val.get("buckets").unwrap_or(&serde_json::Value::Null),
            )
        };

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

        // Build context_hash_specs from hash_targets array.
        // Each target: { context_type, field } where field=="key" → empty parameter_names.
        let mut context_hash_specs: std::collections::HashMap<String, ContextHashSpec> =
            std::collections::HashMap::new();
        if let Some(targets) = hash_targets_val.and_then(|v| v.as_array()) {
            for target in targets {
                let ctx_type = target
                    .get("context_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user");
                let field = target
                    .get("field")
                    .and_then(|v| v.as_str())
                    .unwrap_or("key");
                let spec = context_hash_specs
                    .entry(ctx_type.to_string())
                    .or_insert_with(|| ContextHashSpec {
                        parameter_names: Vec::new(),
                    });
                if field != "key" {
                    spec.parameter_names.push(field.to_string());
                }
            }
        }
        // Default to user.key when no targets provided.
        if context_hash_specs.is_empty() {
            context_hash_specs.insert(
                "user".to_string(),
                ContextHashSpec {
                    parameter_names: Vec::new(),
                },
            );
        }

        Some(Output::Allocation(PercentageAllocation {
            context_hash_specs,
            buckets,
        }))
    } else {
        None
    };

    let _ = index; // index tracked by the caller for logging if needed
    stitchd_proto::flags::v1::FlagRule {
        rule_payload,
        output,
        name: r.name.unwrap_or_default(),
    }
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

    let proto_rules = body
        .rules
        .into_iter()
        .enumerate()
        .map(|(i, r)| rule_body_to_proto(r, i))
        .collect();

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
    // UI sends: [{"_type":"user","key":"alice","parameters":{...}}]
    // Core expects: [{"contexts":[{"context_type":"user","key":"alice","parameters":{...},"private_parameters":[]}]}]
    let evaluation_contexts: Vec<serde_json::Value> = body
        .contexts
        .into_iter()
        .map(|item| {
            if item.get("contexts").is_some() {
                // Already in EvaluationContext shape — pass through.
                item
            } else {
                // Simplified shape: lift into a single-sub-context EvaluationContext.
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
                serde_json::json!({
                    "contexts": [{
                        "context_type": context_type,
                        "key": key,
                        "parameters": parameters,
                        "private_parameters": []
                    }]
                })
            }
        })
        .collect();

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

    let raw_results: Vec<serde_json::Value> =
        serde_json::from_str(&resp.results_json)
            .map_err(|e| GatewayError::Upstream(e.to_string()))?;

    let results: Vec<PreviewResultJson> = raw_results
        .into_iter()
        .map(|v| {
            let context_index = v["context_index"].as_u64().unwrap_or(0) as usize;
            // Extract context_key from the first sub-context of the input evaluation context.
            let context_key = evaluation_contexts
                .get(context_index)
                .and_then(|ec| ec["contexts"].as_array())
                .and_then(|ctxs| ctxs.first())
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

    #[test]
    fn flag_to_json_maps_fields() {
        let f = FeatureFlag {
            key: "my-flag".to_string(),
            enabled: true,
            ..Default::default()
        };
        let j = flag_to_json(&f);
        assert_eq!(j.key, "my-flag");
        assert!(j.enabled);
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
}
