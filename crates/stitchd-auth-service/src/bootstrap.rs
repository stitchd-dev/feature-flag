//! Superadmin bootstrap — runs once on auth-service startup.
//!
//! If `SUPERADMIN_EMAIL` and `SUPERADMIN_PASSWORD` are set and no user with
//! that email exists, a platform superadmin user is created in the database
//! together with a "System" organisation and an `org_admin` membership so the
//! account can immediately call `POST /v1/auth/login` and manage resources.
//! This is idempotent: repeated restarts skip already-existing accounts.

use std::sync::Arc;

use chrono::Utc;
use tracing::{info, warn};
use uuid::Uuid;

use stitchd_core::{
    auth::{OrgRole, crypto::hash_password},
    id::OrganisationId,
    tenant::Organisation,
};
use stitchd_db::{AuthUserRepository, OrgMembershipRepository, OrganisationRepository};

/// Seed the superadmin user and a "System" organisation from environment
/// variables.
///
/// - `SUPERADMIN_EMAIL`    — the admin's email / username
/// - `SUPERADMIN_PASSWORD` — plaintext password; hashed with Argon2id before storage
///
/// Does nothing if either variable is unset or empty.
pub async fn seed_superadmin(
    user_repo:       &Arc<dyn AuthUserRepository>,
    org_repo:        &Arc<dyn OrganisationRepository>,
    membership_repo: &Arc<dyn OrgMembershipRepository>,
) -> anyhow::Result<()> {
    let email = match std::env::var("SUPERADMIN_EMAIL").ok().filter(|v| !v.is_empty()) {
        Some(e) => e,
        None => {
            info!("SUPERADMIN_EMAIL not set — skipping superadmin bootstrap");
            return Ok(());
        }
    };
    let password = match std::env::var("SUPERADMIN_PASSWORD").ok().filter(|v| !v.is_empty()) {
        Some(p) => p,
        None => {
            warn!("SUPERADMIN_EMAIL is set but SUPERADMIN_PASSWORD is missing — skipping bootstrap");
            return Ok(());
        }
    };

    // Idempotent: skip if user already exists.
    if user_repo.find_by_email(&email).await?.is_some() {
        info!(email, "Superadmin already exists — skipping bootstrap");
        return Ok(());
    }

    let hash = hash_password(&password).map_err(|e| anyhow::anyhow!("{e}"))?;
    let user = user_repo.create(&email, "Admin", Some(&hash)).await?;
    info!(email, "Superadmin created successfully");

    // Create the System organisation.
    let now = Utc::now();
    let org = Organisation {
        id:         OrganisationId::from_uuid(Uuid::new_v4()),
        name:       "System".to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version:    1,
        is_system:  true,
    };
    org_repo.create(&org).await?;
    info!(org_id = %org.id, "System organisation created");

    // Add superadmin as org_admin of the System org.
    membership_repo.add_member(user.id, org.id, OrgRole::OrgAdmin).await?;
    info!(email, org_id = %org.id, "Superadmin added to System org as org_admin");

    Ok(())
}
