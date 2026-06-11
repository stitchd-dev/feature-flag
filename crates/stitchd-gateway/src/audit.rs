//! Gateway-edge audit capture (audit_log_20260611).
//!
//! The gateway is the single choke point for every authenticated admin
//! mutation and already holds an [`RbacContext`] (actor = `subject`, org =
//! `tenant_id`) on each request. Rather than thread an actor id through every
//! backend service, we record audit entries here — the same edge-state pattern
//! as the idempotency middleware: a narrowly-scoped [`PgPool`], opt-in via
//! `STITCHD_DATABASE_URL`, layered outside the router, and **fail-open** (an
//! audit-write error never breaks the request).
//!
//! v1 is intentionally lossy-but-honest: `resource_type` + `action` are derived
//! from the request path + method via an explicit map; `resource_ref` is the
//! path's id/key segment (UUID or string key) and `resource_id` is set only when
//! that segment parses as a UUID. Field-level diffs and capturing the
//! created-resource id from response bodies are explicit follow-ups — we never
//! fabricate a value.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use sqlx::PgPool;
use uuid::Uuid;

use stitchd_proto::auth::v1::RbacContext;

/// A resolved audit action for a mutation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditAction {
    /// Singular resource discriminant, e.g. `"flag"`, `"experiment"`.
    pub resource_type: &'static str,
    /// `{resource_type}.{verb}`, e.g. `"flag.update"`, `"experiment.transition"`.
    pub action: String,
    /// The path id/key segment (UUID or string key); `None` for collection-level
    /// creates that carry no id in the path.
    pub resource_ref: Option<String>,
    /// `resource_ref` parsed as a UUID, when it is one.
    pub resource_id: Option<Uuid>,
}

/// Path segment → resource_type for the resource *collections* we audit.
/// The LAST matching collection segment in the path names the resource.
const RESOURCE_COLLECTIONS: &[(&str, &str)] = &[
    ("flags", "flag"),
    ("segments", "segment"),
    ("experiments", "experiment"),
    ("exclusion-groups", "exclusion_group"),
    ("bandit-campaigns", "bandit_campaign"),
    ("metrics", "metric"),
    ("event-definitions", "event_definition"),
    ("events", "event"),
    ("users", "member"),
    ("sdk-keys", "sdk_key"),
    ("projects", "project"),
    ("environments", "environment"),
    ("auth-providers", "auth_provider"),
    ("schedules", "schedule"),
];

/// Trailing path segments that name an explicit mutation verb (override the
/// method-derived verb).
const ACTION_VERBS: &[&str] = &[
    "archive",
    "restore",
    "variants",
    "rules",
    "default-rule-distribution",
    "hashing",
    "prerequisites",
    "transitions",
    "exclusion-group",
    "stop",
    "cancel",
    "pause",
    "resume",
    "entries",
];

/// Trailing segments that mark a POST as a non-mutating compute / read / fire —
/// never audited.
const SKIP_TRAILING: &[&str] = &[
    "preview",
    "evaluate-preview",
    "recompute",
    "track",
    "lookup",
    "firings",
    "stats",
    "results",
    "iterations",
    "exposures",
    "interactions",
    "timeseries",
    "bandit",
    "history",
    "dependencies",
    "permissions",
    "refresh",
    "authorize",
    "callback",
    "sso",
    "metadata",
];

/// Whether a (method, status) pair is an audit-worthy successful mutation.
#[must_use]
pub fn should_record(method: &Method, status: StatusCode) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) && status.is_success()
}

fn verb_for_method(method: &Method) -> &'static str {
    match *method {
        Method::POST => "create",
        Method::DELETE => "delete",
        _ => "update", // PUT / PATCH
    }
}

/// Map a request path + method to an [`AuditAction`], or `None` for paths we do
/// not audit (unmapped, or non-mutating compute/read POSTs).
#[must_use]
pub fn resource_for(path: &str, method: &Method) -> Option<AuditAction> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // Only versioned API routes are auditable — excludes the unversioned
    // Prometheus `/metrics` endpoint (which would otherwise match the `metrics`
    // resource collection).
    if segs.first() != Some(&"v1") {
        return None;
    }
    let trailing = *segs.last().unwrap();
    if SKIP_TRAILING.contains(&trailing) {
        return None;
    }

    // Find the LAST resource-collection segment (e.g. .../projects/{p}/flags/{k}
    // → "flags" wins over "projects").
    let coll_idx = segs.iter().enumerate().rev().find_map(|(i, s)| {
        RESOURCE_COLLECTIONS
            .iter()
            .find(|(seg, _)| seg == s)
            .map(|(_, rt)| (i, *rt))
    });
    let (idx, resource_type) = coll_idx?;

    // Everything after the collection segment.
    let tail = &segs[idx + 1..];
    let explicit_verb = tail.iter().rev().find_map(|s| {
        ACTION_VERBS
            .iter()
            .find(|v| **v == *s)
            .map(|v| v.replace('-', "_"))
    });
    let verb = explicit_verb.unwrap_or_else(|| verb_for_method(method).to_string());

    // The id/key segment is the first tail segment that is not an action verb.
    let id_seg = tail
        .iter()
        .find(|s| !ACTION_VERBS.contains(s) && !SKIP_TRAILING.contains(s))
        .map(|s| (*s).to_string());

    let resource_id = id_seg.as_deref().and_then(|s| Uuid::parse_str(s).ok());

    Some(AuditAction {
        resource_type,
        action: format!("{resource_type}.{verb}"),
        resource_ref: id_seg,
        resource_id,
    })
}

/// Writes audit rows via the gateway's edge PgPool.
pub struct PgAuditWriter {
    pool: PgPool,
}

impl PgAuditWriter {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Best-effort INSERT of one audit entry. Returns an error for the caller to
    /// log; never panics.
    pub async fn record(
        &self,
        org_id: Option<Uuid>,
        actor_id: Option<Uuid>,
        action: &AuditAction,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            INSERT INTO audit_log
                (org_id, actor_id, resource_type, resource_id, resource_ref, action)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            org_id,
            actor_id,
            action.resource_type,
            action.resource_id,
            action.resource_ref,
            action.action,
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

/// Edge middleware that records successful admin mutations. Layered outside the
/// router (same as idempotency); skips non-mutating, unauthenticated, and
/// unmapped traffic; fail-open.
pub async fn audit_middleware(
    State(writer): State<Arc<PgAuditWriter>>,
    req: Request,
    next: Next,
) -> Response {
    // Capture what we need before the request is consumed by the handler.
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let rbac = req.extensions().get::<RbacContext>().map(|c| {
        (
            Uuid::parse_str(&c.subject).ok(),
            Uuid::parse_str(&c.tenant_id).ok(),
        )
    });

    let resp = next.run(req).await;

    if should_record(&method, resp.status())
        && let Some((actor_id, org_id)) = rbac
        && let Some(action) = resource_for(&path, &method)
    {
        // Fire-and-forget: never add latency or fail the request on audit.
        let writer = Arc::clone(&writer);
        tokio::spawn(async move {
            if let Err(e) = writer.record(org_id, actor_id, &action).await {
                tracing::warn!("audit write failed (best-effort): {e}");
            }
        });
    }

    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_record_only_successful_mutations() {
        assert!(should_record(&Method::POST, StatusCode::OK));
        assert!(should_record(&Method::DELETE, StatusCode::NO_CONTENT));
        assert!(should_record(&Method::PUT, StatusCode::CREATED));
        assert!(!should_record(&Method::GET, StatusCode::OK));
        assert!(!should_record(&Method::POST, StatusCode::BAD_REQUEST));
        assert!(!should_record(&Method::POST, StatusCode::BAD_GATEWAY));
    }

    fn act(path: &str, m: Method) -> Option<AuditAction> {
        resource_for(path, &m)
    }

    #[test]
    fn maps_flag_update_with_key_ref() {
        let a = act("/v1/projects/p1/flags/checkout", Method::PUT).unwrap();
        assert_eq!(a.resource_type, "flag");
        assert_eq!(a.action, "flag.update");
        assert_eq!(a.resource_ref.as_deref(), Some("checkout"));
        assert_eq!(a.resource_id, None); // key is not a UUID
    }

    #[test]
    fn maps_flag_create_no_ref() {
        let a = act("/v1/projects/p1/flags", Method::POST).unwrap();
        assert_eq!(a.action, "flag.create");
        assert_eq!(a.resource_ref, None);
    }

    #[test]
    fn maps_flag_archive_verb() {
        let a = act("/v1/projects/p1/flags/checkout/archive", Method::POST).unwrap();
        assert_eq!(a.action, "flag.archive");
        assert_eq!(a.resource_ref.as_deref(), Some("checkout"));
    }

    #[test]
    fn maps_experiment_transition() {
        let a = act(
            "/v1/environments/e1/experiments/exp1/transitions",
            Method::POST,
        )
        .unwrap();
        assert_eq!(a.resource_type, "experiment");
        assert_eq!(a.action, "experiment.transitions");
        assert_eq!(a.resource_ref.as_deref(), Some("exp1"));
    }

    #[test]
    fn parses_uuid_resource_id() {
        let id = "11111111-1111-1111-1111-111111111111";
        let a = act(&format!("/v1/segments/{id}"), Method::DELETE).unwrap();
        assert_eq!(a.resource_type, "segment");
        assert_eq!(a.action, "segment.delete");
        assert_eq!(a.resource_id, Some(Uuid::parse_str(id).unwrap()));
    }

    #[test]
    fn maps_member_create_and_delete() {
        assert_eq!(
            act("/v1/management/orgs/o1/users", Method::POST)
                .unwrap()
                .action,
            "member.create"
        );
        let d = act("/v1/management/orgs/o1/users/u1", Method::DELETE).unwrap();
        assert_eq!(d.action, "member.delete");
        assert_eq!(d.resource_ref.as_deref(), Some("u1"));
    }

    #[test]
    fn maps_sdk_key_and_auth_provider() {
        assert_eq!(
            act("/v1/management/environments/e1/sdk-keys", Method::POST)
                .unwrap()
                .resource_type,
            "sdk_key"
        );
        assert_eq!(
            act("/v1/orgs/o1/auth-providers/ap1", Method::PUT)
                .unwrap()
                .resource_type,
            "auth_provider"
        );
    }

    #[test]
    fn maps_bandit_campaign_stop() {
        let a = act("/v1/environments/e1/bandit-campaigns/c1/stop", Method::POST).unwrap();
        assert_eq!(a.resource_type, "bandit_campaign");
        assert_eq!(a.action, "bandit_campaign.stop");
    }

    #[test]
    fn skips_non_mutating_compute_posts() {
        assert!(
            act(
                "/v1/projects/p1/flags/checkout/evaluate-preview",
                Method::POST
            )
            .is_none()
        );
        assert!(act("/v1/metrics/m1/preview", Method::POST).is_none());
        assert!(act("/v1/environments/e1/experiments/e/recompute", Method::POST).is_none());
        assert!(act("/v1/events/track", Method::POST).is_none());
        assert!(act("/v1/admin/events/track", Method::POST).is_none());
    }

    #[test]
    fn skips_unmapped_paths() {
        assert!(act("/v1/auth/login", Method::POST).is_none());
        assert!(act("/v1/health", Method::POST).is_none());
        assert!(act("/metrics", Method::POST).is_none());
    }

    #[test]
    fn maps_schedule_lifecycle() {
        let a = act("/v1/environments/e1/schedules/s1/cancel", Method::POST).unwrap();
        assert_eq!(a.resource_type, "schedule");
        assert_eq!(a.action, "schedule.cancel");
    }
}
