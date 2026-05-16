//! [`RefreshToken`](stitchd_proto::auth::v1::auth_service_server::AuthService::refresh_token) gRPC handler.

use std::sync::Arc;

use hex;
use sha2::{Digest, Sha256};
use tonic::{Request, Response, Status};
use tracing::instrument;

use stitchd_core::auth::jwt::JwtEngine;
use stitchd_db::{
    AuthUserRepository, OrgMembershipRepository, OrganisationRepository, RefreshTokenRepository,
};
use stitchd_proto::auth::v1::{RefreshTokenRequest, RefreshTokenResponse};

/// Exchange a raw refresh token for a new access + refresh token pair.
///
/// Implements token rotation: the presented token is consumed (revoked) and a fresh
/// refresh token is issued alongside the new JWT.
#[instrument(skip_all)]
pub async fn refresh_token(
    req: Request<RefreshTokenRequest>,
    auth_user_repo: &Arc<dyn AuthUserRepository>,
    membership_repo: &Arc<dyn OrgMembershipRepository>,
    org_repo: &Arc<dyn OrganisationRepository>,
    refresh_token_repo: &Arc<dyn RefreshTokenRepository>,
) -> Result<Response<RefreshTokenResponse>, Status> {
    let raw = req.into_inner().refresh_token;

    // Compute the SHA-256 hash of the raw hex token (same derivation as generate_opaque_token).
    let raw_bytes = hex::decode(&raw)
        .map_err(|_| Status::unauthenticated("invalid refresh token"))?;
    let hash = hex::encode(Sha256::digest(&raw_bytes));

    // Look up the active token by hash.
    let rt = refresh_token_repo
        .find_by_hash(&hash)
        .await
        .map_err(|e| Status::internal(e.to_string()))?
        .ok_or_else(|| Status::unauthenticated("refresh token not found or expired"))?;

    // Consume it (one-time use; enables rotation detection).
    refresh_token_repo
        .consume(rt.id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    // Load the user to get token_secret for JWT signing.
    let user = auth_user_repo
        .find_by_id(rt.user_id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?
        .ok_or_else(|| Status::unauthenticated("user not found"))?;

    // Find the membership for the org scoped to this token.
    let orgs = membership_repo
        .list_orgs_for_user(rt.user_id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    let membership = orgs
        .into_iter()
        .find(|m| m.org_id == rt.org_id)
        .ok_or_else(|| Status::unauthenticated("user is no longer a member of the org"))?;

    let org = org_repo
        .find_by_id(membership.org_id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    let access_token = JwtEngine::issue(
        user.id,
        membership.org_id,
        &user.email,
        membership.role,
        org.is_system,
        &user.token_secret,
    )
    .map_err(|e| Status::internal(e.to_string()))?;

    let (_new_rt, new_raw_refresh) = refresh_token_repo
        .create(user.id, membership.org_id, None, 30)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    Ok(Response::new(RefreshTokenResponse {
        access_token,
        refresh_token: new_raw_refresh,
        expires_in: 3600,
    }))
}
