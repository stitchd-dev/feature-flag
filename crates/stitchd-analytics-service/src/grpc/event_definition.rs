// `tonic::Status` is large; the gateway-side wrapper boxes it on the
// public API surface but this module returns `Result<_, Status>` directly
// to match the analytics-service convention (`handle_*` helpers). Boxing
// would change every callsite.
#![allow(clippy::result_large_err)]

//! Event-definition CRUD handlers (admin path).
//!
//! Closes the pre-existing gap (`feature-flag-wr4`) where the admin UI
//! assumed `/v1/events*` endpoints existed but the gateway had only stub
//! handlers.
//!
//! These RPCs are routed by `AnalyticsServiceImpl` (see `service.rs`) and
//! delegate to the Postgres-backed `EventDefinitionRepository`. Validation
//! semantics mirror the existing analytics-service handlers
//! (`InvalidArgument` for shape errors, `AlreadyExists` on duplicate key,
//! `Aborted` on optimistic-locking conflict).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tonic::{Request, Response, Status};

use stitchd_core::{
    event::{EventDefinition, EventValueType, MetricType},
    id::{EnvironmentId, EventDefinitionId},
};
use stitchd_db::{EventDefinitionRepository, RepositoryError};

use stitchd_proto::analytics::v1::{
    CreateEventDefinitionRequest, DeleteEventDefinitionRequest, DeleteEventDefinitionResponse,
    EventDefinitionMsg, GetEventDefinitionRequest, ListEventDefinitionsRequest,
    ListEventDefinitionsResponse, UpdateEventDefinitionRequest,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn parse_env_id(s: &str) -> Result<EnvironmentId, Status> {
    s.parse::<uuid::Uuid>()
        .map(EnvironmentId::from_uuid)
        .map_err(|_| Status::invalid_argument(format!("invalid environment_id: {s}")))
}

fn parse_event_id(s: &str) -> Result<EventDefinitionId, Status> {
    s.parse::<uuid::Uuid>()
        .map(EventDefinitionId::from_uuid)
        .map_err(|_| Status::invalid_argument(format!("invalid id: {s}")))
}

fn parse_value_type(s: &str) -> Result<EventValueType, Status> {
    match s {
        "bool" => Ok(EventValueType::Bool),
        "int" => Ok(EventValueType::Int),
        "double" => Ok(EventValueType::Double),
        other => Err(Status::invalid_argument(format!(
            "value_type must be one of bool/int/double (got: {other})"
        ))),
    }
}

fn parse_metric_type(s: &str) -> Result<MetricType, Status> {
    match s {
        "count" => Ok(MetricType::Count),
        "conversion" => Ok(MetricType::Conversion),
        "revenue" => Ok(MetricType::Revenue),
        "duration" => Ok(MetricType::Duration),
        "numeric" => Ok(MetricType::Numeric),
        "custom" => Ok(MetricType::Custom),
        other => Err(Status::invalid_argument(format!(
            "metric_type must be one of count/conversion/revenue/duration/numeric/custom \
             (got: {other})"
        ))),
    }
}

const fn value_type_str(v: EventValueType) -> &'static str {
    match v {
        EventValueType::Bool => "bool",
        EventValueType::Int => "int",
        EventValueType::Double => "double",
    }
}

const fn metric_type_str(m: MetricType) -> &'static str {
    match m {
        MetricType::Count => "count",
        MetricType::Conversion => "conversion",
        MetricType::Revenue => "revenue",
        MetricType::Duration => "duration",
        MetricType::Numeric => "numeric",
        MetricType::Custom => "custom",
    }
}

fn schema_json_to_value(json: &str) -> Result<Option<serde_json::Value>, Status> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|e| Status::invalid_argument(format!("invalid schema JSON: {e}")))
}

fn schema_value_to_json(v: Option<&serde_json::Value>) -> String {
    v.map(serde_json::Value::to_string).unwrap_or_default()
}

fn to_proto(def: EventDefinition) -> EventDefinitionMsg {
    EventDefinitionMsg {
        id: def.id.to_string(),
        environment_id: def.environment_id.to_string(),
        key: def.key,
        name: def.name,
        description: def.description,
        value_type: value_type_str(def.value_type).to_string(),
        metric_type: metric_type_str(def.metric_type).to_string(),
        schema_json: schema_value_to_json(def.schema.as_ref()),
        version: def.version,
        created_at: def.created_at.to_rfc3339(),
        updated_at: def.updated_at.to_rfc3339(),
        deleted_at: def.deleted_at.map(|t| t.to_rfc3339()),
        // Stats are populated post-hoc by `handle_list_event_definitions`;
        // single-item responses leave these as None.
        last_fired_at: None,
        count_24h: None,
    }
}

fn map_repo_err(e: RepositoryError) -> Status {
    match e {
        RepositoryError::NotFound { id } => Status::not_found(format!("event_definition: {id}")),
        RepositoryError::UniqueViolation { field } => {
            Status::already_exists(format!("event_definition: duplicate {field}"))
        }
        RepositoryError::VersionConflict { expected, actual } => Status::aborted(format!(
            "version conflict: expected {expected}, actual {actual}"
        )),
        other => Status::internal(format!("event_definition repo error: {other}")),
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

pub async fn handle_create_event_definition(
    repo: &Arc<dyn EventDefinitionRepository>,
    request: Request<CreateEventDefinitionRequest>,
) -> Result<Response<EventDefinitionMsg>, Status> {
    let r = request.into_inner();
    if r.key.trim().is_empty() {
        return Err(Status::invalid_argument("key cannot be empty"));
    }
    // GL-13: when name is absent or empty, default to the event key. This
    // keeps the behaviour that was previously in the gateway's `create_event`
    // handler, where `body.name.unwrap_or_else(|| body.event_key.clone())`.
    let name = if r.name.trim().is_empty() {
        r.key.trim().to_string()
    } else {
        r.name.trim().to_string()
    };

    let env_id = parse_env_id(&r.environment_id)?;
    let metric_type = parse_metric_type(&r.metric_type)?;
    let value_type = match r.value_type {
        Some(s) if !s.is_empty() => parse_value_type(&s)?,
        _ => metric_type.default_value_type(),
    };
    let schema = match r.schema_json {
        Some(j) => schema_json_to_value(&j)?,
        None => None,
    };

    let now = Utc::now();
    let def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env_id,
        key: r.key.trim().to_string(),
        name,
        description: r.description.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }),
        value_type,
        metric_type,
        schema,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: 1,
    };
    repo.create(&def).await.map_err(map_repo_err)?;
    Ok(Response::new(to_proto(def)))
}

pub async fn handle_get_event_definition(
    repo: &Arc<dyn EventDefinitionRepository>,
    request: Request<GetEventDefinitionRequest>,
) -> Result<Response<EventDefinitionMsg>, Status> {
    let r = request.into_inner();
    let def = if let Some(id_str) = r.id.as_deref().filter(|s| !s.is_empty()) {
        let id = parse_event_id(id_str)?;
        repo.find_by_id(id).await.map_err(map_repo_err)?
    } else if let (Some(env), Some(key)) = (
        r.environment_id.as_deref().filter(|s| !s.is_empty()),
        r.key.as_deref().filter(|s| !s.is_empty()),
    ) {
        let env_id = parse_env_id(env)?;
        repo.find_by_key(key, env_id).await.map_err(map_repo_err)?
    } else {
        return Err(Status::invalid_argument(
            "must provide either `id` or both `environment_id` + `key`",
        ));
    };
    Ok(Response::new(to_proto(def)))
}

// ── ClickHouse stat helpers ───────────────────────────────────────────────────

/// One row returned by the "stats per metric_key" ClickHouse query.
#[derive(clickhouse::Row, Deserialize)]
struct EventStatRow {
    metric_key: String,
    /// Unix-millisecond timestamp of the most recent firing (`max(timestamp)`).
    /// `toUnixTimestamp64Milli` returns `Int64` in ClickHouse RowBinary.
    last_fired_at_ms: i64,
    /// Events fired in the trailing 24 h.
    /// `countIf` returns `UInt64` in ClickHouse RowBinary.
    count_24h: u64,
}

/// Query ClickHouse for `last_fired_at` + `count_24h` per key in this env.
/// Returns an empty map when ClickHouse is unreachable — we degrade gracefully
/// (stats show as absent) rather than failing the whole list call.
async fn fetch_event_stats(
    ch_client: &Arc<clickhouse::Client>,
    env_uuid: uuid::Uuid,
) -> HashMap<String, (Option<String>, Option<i64>)> {
    let sql = r"
        SELECT
            metric_key,
            toUnixTimestamp64Milli(max(timestamp))             AS last_fired_at_ms,
            countIf(timestamp >= now() - INTERVAL 24 HOUR)     AS count_24h
        FROM events
        WHERE env_id = ?
        GROUP BY metric_key
    ";
    let rows: Vec<EventStatRow> = match ch_client
        .query(sql)
        .bind(env_uuid)
        .fetch_all::<EventStatRow>()
        .await
    {
        Ok(r) => r,
        Err(_) => return HashMap::new(), // degrade gracefully
    };

    rows.into_iter()
        .map(|r| {
            let last_fired_at = if r.last_fired_at_ms > 0 {
                DateTime::<Utc>::from_timestamp_millis(r.last_fired_at_ms).map(|dt| dt.to_rfc3339())
            } else {
                None
            };
            // Cast u64 → i64; realistic counts won't overflow i64.
            (r.metric_key, (last_fired_at, Some(r.count_24h as i64)))
        })
        .collect()
}

pub async fn handle_list_event_definitions(
    repo: &Arc<dyn EventDefinitionRepository>,
    ch_client: &Arc<clickhouse::Client>,
    request: Request<ListEventDefinitionsRequest>,
) -> Result<Response<ListEventDefinitionsResponse>, Status> {
    let r = request.into_inner();
    let env_id = parse_env_id(&r.environment_id)?;
    let after = stitchd_db::KeysetCursor::decode_opt(Some(&r.cursor))
        .map_err(|_| Status::invalid_argument("invalid cursor"))?;
    let limit = u64::from(stitchd_db::effective_limit(r.limit, 50, 200));
    let include_archived = r.include_archived.unwrap_or(false);
    let (items, next_cursor) = repo
        .list_by_environment_keyset(env_id, after, limit, include_archived)
        .await
        .map_err(map_repo_err)?;

    // Enrich with ClickHouse usage stats (best-effort — fails open).
    let stats = fetch_event_stats(ch_client, env_id.as_uuid()).await;

    let proto_items: Vec<EventDefinitionMsg> = items
        .into_iter()
        .map(|def| {
            let mut msg = to_proto(def);
            if let Some((last_fired_at, count_24h)) = stats.get(&msg.key) {
                msg.last_fired_at = last_fired_at.clone();
                msg.count_24h = *count_24h;
            }
            msg
        })
        .collect();

    Ok(Response::new(ListEventDefinitionsResponse {
        items: proto_items,
        next_cursor: next_cursor.unwrap_or_default(),
    }))
}

pub async fn handle_update_event_definition(
    repo: &Arc<dyn EventDefinitionRepository>,
    request: Request<UpdateEventDefinitionRequest>,
) -> Result<Response<EventDefinitionMsg>, Status> {
    let r = request.into_inner();
    let id = parse_event_id(&r.id)?;
    // Fetch existing, apply patches, then write back.
    let existing = repo.find_by_id(id).await.map_err(map_repo_err)?;
    if existing.version != r.expected_version {
        return Err(Status::aborted(format!(
            "version conflict: expected {}, actual {}",
            r.expected_version, existing.version
        )));
    }

    let mut next = existing.clone();
    next.version = r.expected_version;
    if let Some(name) = r.name {
        if name.trim().is_empty() {
            return Err(Status::invalid_argument("name cannot be empty"));
        }
        next.name = name.trim().to_string();
    }
    if let Some(desc) = r.description {
        let t = desc.trim().to_string();
        next.description = if t.is_empty() { None } else { Some(t) };
    }
    if let Some(vt) = r.value_type
        && !vt.is_empty()
    {
        next.value_type = parse_value_type(&vt)?;
    }
    if let Some(mt) = r.metric_type
        && !mt.is_empty()
    {
        next.metric_type = parse_metric_type(&mt)?;
    }
    if let Some(schema_json) = r.schema_json {
        next.schema = schema_json_to_value(&schema_json)?;
    }

    let updated = repo.update(&next).await.map_err(map_repo_err)?;
    Ok(Response::new(to_proto(updated)))
}

pub async fn handle_delete_event_definition(
    repo: &Arc<dyn EventDefinitionRepository>,
    request: Request<DeleteEventDefinitionRequest>,
) -> Result<Response<DeleteEventDefinitionResponse>, Status> {
    let r = request.into_inner();
    let id = parse_event_id(&r.id)?;
    repo.soft_delete(id).await.map_err(map_repo_err)?;
    Ok(Response::new(DeleteEventDefinitionResponse {}))
}
