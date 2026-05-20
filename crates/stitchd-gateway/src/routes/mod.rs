//! Gateway route handler modules.

use crate::error::GatewayError;

/// Check that the caller has the given permission in their `RbacContext`.
pub fn require_permission(
    req: &axum::extract::Request,
    permission: &str,
) -> Result<(), GatewayError> {
    match req
        .extensions()
        .get::<stitchd_proto::auth::v1::RbacContext>()
    {
        Some(ctx) if ctx.permissions.iter().any(|p| p == permission) => Ok(()),
        Some(_) => Err(GatewayError::Unauthorized(format!(
            "missing permission: {permission}"
        ))),
        None => Err(GatewayError::Unauthorized(
            "missing credentials".to_string(),
        )),
    }
}

pub mod admin;
pub mod auth;
pub mod auth_providers;
pub mod context_intel;
pub mod eval_stats;
pub mod event_admin;
pub mod events;
pub mod experiments;
pub mod flags;
pub mod management;
pub mod metrics;
pub mod oidc;
pub mod saml;
pub mod sdk_backend;
pub mod segments;
pub mod stats;
