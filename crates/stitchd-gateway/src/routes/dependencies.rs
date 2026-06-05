//! Cross-entity dependency-graph read API
//! (`flag_lifecycle_20260604`, Phase 6 Task 2).
//!
//! A single read endpoint returns the **upstream** (what this entity depends
//! on) and **downstream** (what depends on it) dependency edges for a flag,
//! segment, or experiment, aggregated by the gateway fanning out to the owning
//! services' existing read RPCs:
//!
//! `GET /v1/projects/{project_id}/{entity_kind}/{entity_id}/dependencies`
//!
//! * **flag** (`entity_id` = flag key): upstream = its prerequisite flags
//!   (from the flag's gate) + the segments referenced by its rules; downstream
//!   = the flags that list it as a prerequisite (scanned via `ListFlags`).
//! * **segment** (`entity_id` = segment UUID): downstream = the flags whose
//!   rules reference it (`ListFlags` rule scan). Segment→segment nesting is
//!   env-scoped and not reachable from the project-scoped `ListFlags`/`GetFlag`
//!   surface, and the segment write path forbids segment-membership ops inside
//!   a segment's own condition, so it is reported as an empty set with a note.
//! * **experiment** (`entity_id` = experiment UUID): start-time prerequisites
//!   and reverse experiment→experiment edges are not exposed over any read RPC,
//!   so the graph is returned empty with an explanatory note.
//!
//! Each edge carries an `id` (and, when known, a `key`) plus an edge `kind`
//! describing the relationship (`prerequisite_flag`, `segment_ref`,
//! `dependent_flag`, …) so the Admin UI can render a typed graph.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::Serialize;
use utoipa::ToSchema;

use stitchd_proto::flags::v1::{GetFlagRequest, ListFlagsRequest};

use crate::error::GatewayError;
use crate::state::GatewayState;

/// One edge in the dependency graph.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DependencyEdge {
    /// Entity kind of the referenced entity: `flag` | `segment` | `experiment`.
    pub entity_kind: String,
    /// Stable identifier of the referenced entity (UUID or key, per kind).
    pub id: String,
    /// Human-readable key of the referenced entity, when known (else empty).
    pub key: String,
    /// Relationship kind, e.g. `prerequisite_flag`, `segment_ref`,
    /// `dependent_flag`.
    pub kind: String,
}

/// The dependency graph for a single entity.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DependencyGraphJson {
    /// Entity kind of the subject: `flag` | `segment` | `experiment`.
    pub entity_kind: String,
    /// Subject entity id (as supplied in the path).
    pub entity_id: String,
    /// Entities the subject depends on.
    pub upstream: Vec<DependencyEdge>,
    /// Entities that depend on the subject.
    pub downstream: Vec<DependencyEdge>,
    /// Optional note describing any edges that could not be computed from the
    /// available read RPCs (empty when the graph is complete).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Recursively collect segment UUIDs referenced by `InSegment`/`NotInSegment`
/// leaves in a serialized-`ConditionExpr` JSON tree.
fn collect_segment_ids(expr: &serde_json::Value, out: &mut Vec<String>) {
    match expr {
        serde_json::Value::Object(map) => {
            if let Some(leaf) = map.get("Leaf") {
                if let Some(id) = leaf.get("InSegment").and_then(serde_json::Value::as_str) {
                    out.push(id.to_string());
                } else if let Some(id) =
                    leaf.get("NotInSegment").and_then(serde_json::Value::as_str)
                {
                    out.push(id.to_string());
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

/// Parse a `FlagRule.rule_payload` (serialized `ConditionExpr`) and collect the
/// segment ids it references.
fn rule_segment_ids(rule: &stitchd_proto::flags::v1::FlagRule, out: &mut Vec<String>) {
    if rule.rule_payload.is_empty() {
        return;
    }
    if let Ok(expr) = serde_json::from_slice::<serde_json::Value>(&rule.rule_payload) {
        collect_segment_ids(&expr, out);
    }
}

/// `GET /v1/projects/{project_id}/{entity_kind}/{entity_id}/dependencies`
///
/// Returns the upstream + downstream dependency edges for the entity.
#[utoipa::path(
    get,
    path = "/v1/projects/{project_id}/{entity_kind}/{entity_id}/dependencies",
    tag = "dependencies",
    params(
        ("project_id" = String, Path, description = "Project ID (scope)"),
        ("entity_kind" = String, Path, description = "flags | segments | experiments"),
        ("entity_id" = String, Path, description = "Flag key / segment id / experiment id"),
    ),
    responses(
        (status = 200, description = "Dependency graph", body = DependencyGraphJson),
        (status = 400, description = "Unknown entity kind"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Entity not found"),
        (status = 502, description = "Owning service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_dependencies(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, entity_kind, entity_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    match entity_kind.as_str() {
        "flags" => flag_dependencies(&state, &project_id, &entity_id).await,
        "segments" => segment_dependencies(&state, &project_id, &entity_id).await,
        "experiments" => Ok(Json(experiment_dependencies(&entity_id))),
        other => Err(GatewayError::BadRequest(format!(
            "unknown dependency entity kind '{other}' (expected flags|segments|experiments)"
        ))),
    }
}

/// Build the dependency graph for a flag.
async fn flag_dependencies(
    state: &Arc<GatewayState>,
    project_id: &str,
    flag_key: &str,
) -> Result<Json<DependencyGraphJson>, GatewayError> {
    // ── Subject flag: prerequisites (upstream flags) + rules (upstream segments).
    let subject = {
        let mut client = state.flag_client.lock().await;
        client
            .get_flag(tonic::Request::new(GetFlagRequest {
                environment_id: String::new(),
                project_id: project_id.to_string(),
                flag_key: flag_key.to_string(),
            }))
            .await
            .map_err(GatewayError::from)?
            .into_inner()
    };

    let mut upstream: Vec<DependencyEdge> = Vec::new();
    for p in &subject.prerequisites {
        upstream.push(DependencyEdge {
            entity_kind: "flag".to_string(),
            id: p.prerequisite_flag_id.clone(),
            key: p.prerequisite_flag_key.clone(),
            kind: "prerequisite_flag".to_string(),
        });
    }
    let mut seg_ids: Vec<String> = Vec::new();
    for rule in &subject.rules {
        rule_segment_ids(rule, &mut seg_ids);
    }
    seg_ids.sort_unstable();
    seg_ids.dedup();
    for seg_id in seg_ids {
        upstream.push(DependencyEdge {
            entity_kind: "segment".to_string(),
            id: seg_id,
            key: String::new(),
            kind: "segment_ref".to_string(),
        });
    }

    // ── Downstream: flags that list this flag as a prerequisite.
    let all_flags = {
        let mut client = state.flag_client.lock().await;
        client
            .list_flags(tonic::Request::new(ListFlagsRequest {
                environment_id: String::new(),
                include_archived: false,
                project_id: project_id.to_string(),
                page: 0,
                per_page: 0,
            }))
            .await
            .map_err(GatewayError::from)?
            .into_inner()
            .flags
    };
    let mut downstream: Vec<DependencyEdge> = Vec::new();
    for f in &all_flags {
        if f.key == flag_key {
            continue;
        }
        if f.prerequisites
            .iter()
            .any(|p| p.prerequisite_flag_key == flag_key || p.prerequisite_flag_id == flag_key)
        {
            downstream.push(DependencyEdge {
                entity_kind: "flag".to_string(),
                id: f.flag_id.clone(),
                key: f.key.clone(),
                kind: "dependent_flag".to_string(),
            });
        }
    }

    Ok(Json(DependencyGraphJson {
        entity_kind: "flag".to_string(),
        entity_id: flag_key.to_string(),
        upstream,
        downstream,
        note: None,
    }))
}

/// Build the dependency graph for a segment: downstream flags whose rules
/// reference it. (Segment→segment nesting is env-scoped and unreachable here;
/// reported via the note.)
async fn segment_dependencies(
    state: &Arc<GatewayState>,
    project_id: &str,
    segment_id: &str,
) -> Result<Json<DependencyGraphJson>, GatewayError> {
    let all_flags = {
        let mut client = state.flag_client.lock().await;
        client
            .list_flags(tonic::Request::new(ListFlagsRequest {
                environment_id: String::new(),
                include_archived: false,
                project_id: project_id.to_string(),
                page: 0,
                per_page: 0,
            }))
            .await
            .map_err(GatewayError::from)?
            .into_inner()
            .flags
    };

    let mut downstream: Vec<DependencyEdge> = Vec::new();
    for f in &all_flags {
        let mut seg_ids: Vec<String> = Vec::new();
        for rule in &f.rules {
            rule_segment_ids(rule, &mut seg_ids);
        }
        if seg_ids.iter().any(|s| s == segment_id) {
            downstream.push(DependencyEdge {
                entity_kind: "flag".to_string(),
                id: f.flag_id.clone(),
                key: f.key.clone(),
                kind: "segment_ref".to_string(),
            });
        }
    }

    Ok(Json(DependencyGraphJson {
        entity_kind: "segment".to_string(),
        entity_id: segment_id.to_string(),
        upstream: Vec::new(),
        downstream,
        note: Some(
            "segment→segment nesting edges are env-scoped and not included in this \
             project-scoped view (and are forbidden by the segment write path)"
                .to_string(),
        ),
    }))
}

/// Dependency graph for an experiment. Start-time prerequisites and reverse
/// experiment→experiment edges are not exposed over any read RPC, so the graph
/// is empty with an explanatory note. The delete-block guard
/// (experimentation-service) still enforces the integrity authoritatively.
fn experiment_dependencies(experiment_id: &str) -> DependencyGraphJson {
    DependencyGraphJson {
        entity_kind: "experiment".to_string(),
        entity_id: experiment_id.to_string(),
        upstream: Vec::new(),
        downstream: Vec::new(),
        note: Some(
            "experiment start-prerequisite edges are not exposed over a read RPC; \
             referential integrity is enforced at delete time (409 dependency_exists)"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_segment_ids_walks_nested_tree() {
        let seg = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let expr = serde_json::json!({
            "And": [
                {"Leaf": {"InSegment": seg}},
                {"Not": {"Leaf": {"NotInSegment": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"}}}
            ]
        });
        let mut out = Vec::new();
        collect_segment_ids(&expr, &mut out);
        assert!(out.contains(&seg.to_string()));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn experiment_dependencies_carries_note() {
        let g = experiment_dependencies("exp-1");
        assert_eq!(g.entity_kind, "experiment");
        assert!(g.upstream.is_empty());
        assert!(g.downstream.is_empty());
        assert!(g.note.is_some());
    }
}
