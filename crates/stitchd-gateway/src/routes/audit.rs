//! Audit log read endpoint (audit_log_20260611).
//!
//! `GET /v1/orgs/{org_id}/audit` — org-scoped, keyset-paginated, newest first,
//! optionally filtered by `resource_type` / `action`. Reads the `audit_log` rows
//! captured by the gateway edge `audit_middleware` via the gateway's edge pool.
//! Authorisation: the caller's `RbacContext.tenant_id` must equal `{org_id}`
//! (or the caller is a System-org user).

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use stitchd_proto::auth::v1::RbacContext;

use crate::error::GatewayError;
use crate::pagination::{CursorPage, CursorParams, encode_cursor};
use crate::state::GatewayState;

/// One audit entry as returned to the Admin UI.
#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AuditEntryJson {
    pub id: Uuid,
    /// Acting user id; `None` for system actions.
    pub actor_id: Option<Uuid>,
    /// Acting user email (joined from `users`); `None` when unknown/system.
    pub actor_email: Option<String>,
    pub resource_type: String,
    /// UUID of the affected entity when the path carried one.
    pub resource_id: Option<Uuid>,
    /// Human path reference (UUID or string key); `None` for collection creates.
    pub resource_ref: Option<String>,
    pub action: String,
    /// RFC3339 UTC timestamp.
    pub created_at: String,
}

/// `?cursor=&limit=&resource_type=&action=`
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AuditListQuery {
    #[serde(flatten)]
    pub cursor: CursorParams,
    #[serde(default)]
    pub resource_type: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
}

/// Opaque keyset cursor over `(created_at, id)` descending.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AuditCursor {
    /// `created_at` rendered to microsecond RFC3339 (round-trips via `::timestamptz`).
    pub(crate) ts: String,
    pub(crate) id: Uuid,
}

/// Fetch up to `limit + 1` audit rows for `org_uuid`, newest first, applying the
/// optional filters + keyset cursor. The surplus row (if present) signals a next
/// page. Extracted from the handler so it is directly DB-testable.
pub(crate) async fn query_audit(
    pool: &sqlx::PgPool,
    org_uuid: Uuid,
    resource_type: Option<&str>,
    action: Option<&str>,
    cursor: Option<AuditCursor>,
    limit: u32,
) -> Result<Vec<AuditEntryJson>, sqlx::Error> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT a.id, a.actor_id, u.email AS actor_email, a.resource_type, \
         a.resource_id, a.resource_ref, a.action, \
         to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at \
         FROM audit_log a LEFT JOIN users u ON u.id = a.actor_id WHERE a.org_id = ",
    );
    qb.push_bind(org_uuid);
    if let Some(rt) = resource_type.filter(|s| !s.is_empty()) {
        qb.push(" AND a.resource_type = ").push_bind(rt.to_string());
    }
    if let Some(ac) = action.filter(|s| !s.is_empty()) {
        qb.push(" AND a.action = ").push_bind(ac.to_string());
    }
    if let Some(cur) = cursor {
        qb.push(" AND (a.created_at, a.id) < (")
            .push_bind(cur.ts)
            .push("::timestamptz, ")
            .push_bind(cur.id)
            .push("::uuid)");
    }
    qb.push(" ORDER BY a.created_at DESC, a.id DESC LIMIT ");
    qb.push_bind(i64::from(limit) + 1);

    qb.build_query_as::<AuditEntryJson>().fetch_all(pool).await
}

/// `GET /v1/orgs/{org_id}/audit`
#[utoipa::path(
    get,
    path = "/v1/orgs/{org_id}/audit",
    tag = "audit",
    params(
        ("org_id" = String, Path, description = "Organisation ID"),
        ("cursor" = Option<String>, Query, description = "Opaque keyset cursor"),
        ("limit" = Option<u32>, Query, description = "Page size (max 200)"),
        ("resource_type" = Option<String>, Query, description = "Filter by resource type"),
        ("action" = Option<String>, Query, description = "Filter by action"),
    ),
    responses(
        (status = 200, description = "Audit entries (newest first)", body = [AuditEntryJson]),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Cross-org access"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_audit(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Extension(rbac): Extension<RbacContext>,
    Query(q): Query<AuditListQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    // Authorisation: only the owning org (or a System-org user) may read.
    if !rbac.is_system && rbac.tenant_id != org_id {
        return Err(GatewayError::Unauthorized(
            "audit log is scoped to your organisation".to_string(),
        ));
    }
    let org_uuid = Uuid::parse_str(&org_id)
        .map_err(|_| GatewayError::BadRequest("invalid org_id".to_string()))?;

    // No edge pool configured → audit capture/read disabled; empty page.
    let Some(pool) = state.audit_pool.clone() else {
        return Ok(Json(CursorPage {
            items: Vec::<AuditEntryJson>::new(),
            next_cursor: None,
        }));
    };

    let limit = q.cursor.effective_limit();
    let cursor = q
        .cursor
        .decode::<AuditCursor>()
        .map_err(|_| GatewayError::BadRequest("invalid cursor".to_string()))?;

    let mut rows = query_audit(
        &pool,
        org_uuid,
        q.resource_type.as_deref(),
        q.action.as_deref(),
        cursor,
        limit,
    )
    .await
    .map_err(|e| GatewayError::Upstream(format!("audit query failed: {e}")))?;

    // Keyset: a surplus row means there's a next page.
    let next_cursor = if rows.len() > limit as usize {
        rows.truncate(limit as usize);
        rows.last().map(|last| {
            encode_cursor(&AuditCursor {
                ts: last.created_at.clone(),
                id: last.id,
            })
        })
    } else {
        None
    };

    Ok(Json(CursorPage {
        items: rows,
        next_cursor,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn insert(pool: &PgPool, org: Uuid, rt: &str, action: &str, actor: Option<Uuid>) {
        sqlx::query(
            "INSERT INTO audit_log (org_id, actor_id, resource_type, action) VALUES ($1, $2, $3, $4)",
        )
        .bind(org)
        .bind(actor)
        .bind(rt)
        .bind(action)
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn query_audit_is_org_scoped_ordered_and_filterable(pool: PgPool) {
        let org = Uuid::new_v4();
        let other = Uuid::new_v4();
        insert(&pool, org, "flag", "flag.create", None).await;
        insert(&pool, org, "flag", "flag.update", None).await;
        insert(&pool, org, "segment", "segment.delete", None).await;
        insert(&pool, other, "flag", "flag.create", None).await;

        let all = query_audit(&pool, org, None, None, None, 50).await.unwrap();
        assert_eq!(all.len(), 3, "only this org's rows");

        let flags = query_audit(&pool, org, Some("flag"), None, None, 50)
            .await
            .unwrap();
        assert_eq!(flags.len(), 2, "resource_type filter");

        let upd = query_audit(&pool, org, None, Some("flag.update"), None, 50)
            .await
            .unwrap();
        assert_eq!(upd.len(), 1, "action filter");

        let isolated = query_audit(&pool, other, None, None, None, 50)
            .await
            .unwrap();
        assert_eq!(isolated.len(), 1, "cross-org isolation");
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn query_audit_keyset_paginates_without_overlap(pool: PgPool) {
        let org = Uuid::new_v4();
        for i in 0..5 {
            insert(&pool, org, "flag", &format!("flag.a{i}"), None).await;
        }
        // limit 2 → fetch limit+1 = 3 rows (the surplus marks a next page).
        let p1 = query_audit(&pool, org, None, None, None, 2).await.unwrap();
        assert_eq!(p1.len(), 3);
        let boundary = &p1[1];
        let cur = AuditCursor {
            ts: boundary.created_at.clone(),
            id: boundary.id,
        };
        let p2 = query_audit(&pool, org, None, None, Some(cur), 2)
            .await
            .unwrap();
        let first_page_ids: Vec<Uuid> = vec![p1[0].id, p1[1].id];
        assert!(
            p2.iter().all(|r| !first_page_ids.contains(&r.id)),
            "page 2 must not overlap page 1"
        );
    }
}
