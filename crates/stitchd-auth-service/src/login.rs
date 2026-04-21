//! LoginWithPassword gRPC handler.

use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::instrument;

use stitchd_core::auth::{
    crypto::verify_password,
    jwt::JwtEngine,
};
use stitchd_core::id::OrganisationId;
use uuid::Uuid;
use stitchd_db::{AuthUserRepository, OrgMembershipRepository, OrganisationRepository, RefreshTokenRepository};
use stitchd_proto::auth::v1::{LoginRequest, LoginResponse};

/// Authenticate email + password and issue a JWT + refresh token.
#[instrument(skip_all, fields(email = %req.get_ref().email))]
pub async fn login_with_password(
    req: Request<LoginRequest>,
    auth_user_repo:   &Arc<dyn AuthUserRepository>,
    membership_repo:  &Arc<dyn OrgMembershipRepository>,
    org_repo:         &Arc<dyn OrganisationRepository>,
    refresh_token_repo: &Arc<dyn RefreshTokenRepository>,
) -> Result<Response<LoginResponse>, Status> {
    let r = req.into_inner();

    let user = auth_user_repo
        .find_by_email(&r.email)
        .await
        .map_err(|e| Status::internal(e.to_string()))?
        .ok_or_else(|| Status::unauthenticated("invalid credentials"))?;

    let hash = user
        .password_hash
        .as_deref()
        .ok_or_else(|| Status::unauthenticated("password auth not configured for this user"))?;

    let ok = verify_password(&r.password, hash)
        .map_err(|e| Status::internal(e.to_string()))?;
    if !ok {
        return Err(Status::unauthenticated("invalid credentials"));
    }

    // Pick org — caller may scope to a specific org, otherwise first membership.
    let orgs = membership_repo
        .list_orgs_for_user(user.id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    let membership = if r.org_id.is_empty() {
        orgs.into_iter().next()
    } else {
        let target = OrganisationId::from_uuid(
            Uuid::parse_str(&r.org_id)
                .map_err(|_| Status::invalid_argument("org_id is not a valid UUID"))?,
        );
        orgs.into_iter().find(|m| m.org_id == target)
    }
    .ok_or_else(|| Status::unauthenticated("user is not a member of any organisation"))?;

    // Look up the org to stamp is_system in the JWT claims.
    let org = org_repo
        .find_by_id(membership.org_id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    let access_token = JwtEngine::issue(
        user.id,
        membership.org_id,
        &user.email,
        membership.role.clone(),
        org.is_system,
        &user.token_secret,
    )
    .map_err(|e| Status::internal(e.to_string()))?;

    let (_rt, raw_refresh) = refresh_token_repo
        .create(user.id, membership.org_id, None, 30)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    Ok(Response::new(LoginResponse {
        access_token,
        refresh_token: raw_refresh,
        expires_in: 3600,
        user_id: user.id.to_string(),
        org_id: membership.org_id.to_string(),
    }))
}
