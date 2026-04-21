//! Handler-level tests for `/auth/*` endpoints.
//!
//! Uses in-memory stub repositories to avoid a real database.
//! Each test exercises a single scenario end-to-end via `tower::ServiceExt::oneshot`.

#![cfg(test)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt as _;

use stitchd_core::{
    auth::{OrgMembership, OrgRole, RefreshToken, User, UserStatus, jwt::JwtEngine},
    id::{OrganisationId, RefreshTokenId, UserId},
};
use stitchd_db::{
    AuthUserRepository, MfaRepository, OrgMembershipRepository, RefreshTokenRepository,
    RepositoryError,
};

use crate::{
    AppState,
    api::router::build_api_router,
};

// ---------------------------------------------------------------------------
// In-memory stub repositories
// ---------------------------------------------------------------------------

/// In-memory user store for tests.
#[derive(Default)]
struct InMemAuthUserRepo {
    users: Mutex<HashMap<UserId, User>>,
}

impl InMemAuthUserRepo {
    fn with_user(user: User) -> Self {
        let mut map = HashMap::new();
        map.insert(user.id, user);
        Self { users: Mutex::new(map) }
    }
}

#[async_trait::async_trait]
impl AuthUserRepository for InMemAuthUserRepo {
    async fn create(&self, email: &str, display_name: &str, password_hash: Option<&str>) -> Result<User, RepositoryError> {
        let user = User {
            id: UserId::new(),
            email: email.to_string(),
            display_name: display_name.to_string(),
            avatar_url: None,
            password_hash: password_hash.map(str::to_string),
            token_secret: uuid::Uuid::new_v4(),
            totp_secret: None,
            totp_enabled: false,
            status: UserStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.users.lock().unwrap().insert(user.id, user.clone());
        Ok(user)
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, RepositoryError> {
        Ok(self.users.lock().unwrap().values().find(|u| u.email == email).cloned())
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn rotate_token_secret(&self, user_id: UserId) -> Result<uuid::Uuid, RepositoryError> {
        let new_secret = uuid::Uuid::new_v4();
        self.users.lock().unwrap()
            .get_mut(&user_id)
            .map(|u| u.token_secret = new_secret)
            .ok_or_else(|| RepositoryError::NotFound { id: user_id.to_string() })?;
        Ok(new_secret)
    }

    async fn update_status(&self, user_id: UserId, status: UserStatus) -> Result<(), RepositoryError> {
        self.users.lock().unwrap()
            .get_mut(&user_id)
            .map(|u| u.status = status)
            .ok_or_else(|| RepositoryError::NotFound { id: user_id.to_string() })
    }

    async fn update_password_hash(&self, user_id: UserId, hash: &str) -> Result<(), RepositoryError> {
        self.users.lock().unwrap()
            .get_mut(&user_id)
            .map(|u| u.password_hash = Some(hash.to_string()))
            .ok_or_else(|| RepositoryError::NotFound { id: user_id.to_string() })
    }

    async fn update_profile(&self, user_id: UserId, display_name: &str, avatar_url: Option<&str>) -> Result<(), RepositoryError> {
        let mut lock = self.users.lock().unwrap();
        let u = lock.get_mut(&user_id).ok_or_else(|| RepositoryError::NotFound { id: user_id.to_string() })?;
        u.display_name = display_name.to_string();
        u.avatar_url = avatar_url.map(str::to_string);
        Ok(())
    }
}

/// In-memory membership store.
#[derive(Default)]
struct InMemMembershipRepo {
    memberships: Mutex<Vec<OrgMembership>>,
}

impl InMemMembershipRepo {
    fn with(user_id: UserId, org_id: OrganisationId, role: OrgRole) -> Self {
        Self {
            memberships: Mutex::new(vec![OrgMembership { user_id, org_id, role, joined_at: Utc::now() }]),
        }
    }
}

#[async_trait::async_trait]
impl OrgMembershipRepository for InMemMembershipRepo {
    async fn add_member(&self, user_id: UserId, org_id: OrganisationId, role: OrgRole) -> Result<OrgMembership, RepositoryError> {
        let m = OrgMembership { user_id, org_id, role, joined_at: Utc::now() };
        self.memberships.lock().unwrap().push(m.clone());
        Ok(m)
    }
    async fn find_membership(&self, user_id: UserId, org_id: OrganisationId) -> Result<Option<OrgMembership>, RepositoryError> {
        Ok(self.memberships.lock().unwrap().iter().find(|m| m.user_id == user_id && m.org_id == org_id).cloned())
    }
    async fn list_orgs_for_user(&self, user_id: UserId) -> Result<Vec<OrgMembership>, RepositoryError> {
        Ok(self.memberships.lock().unwrap().iter().filter(|m| m.user_id == user_id).cloned().collect())
    }
    async fn remove_member(&self, user_id: UserId, org_id: OrganisationId) -> Result<(), RepositoryError> {
        self.memberships.lock().unwrap().retain(|m| !(m.user_id == user_id && m.org_id == org_id));
        Ok(())
    }
    async fn update_role(&self, user_id: UserId, org_id: OrganisationId, role: OrgRole) -> Result<(), RepositoryError> {
        let mut lock = self.memberships.lock().unwrap();
        let m = lock.iter_mut().find(|m| m.user_id == user_id && m.org_id == org_id)
            .ok_or_else(|| RepositoryError::NotFound { id: format!("{user_id}/{org_id}") })?;
        m.role = role;
        Ok(())
    }
}

/// In-memory refresh token store.
#[derive(Default)]
struct InMemRefreshTokenRepo {
    tokens: Mutex<Vec<RefreshToken>>,
}

#[async_trait::async_trait]
impl RefreshTokenRepository for InMemRefreshTokenRepo {
    async fn create(&self, user_id: UserId, org_id: OrganisationId, device_hint: Option<&str>, ttl_days: i64) -> Result<(RefreshToken, String), RepositoryError> {
        let (raw, hash) = stitchd_core::auth::crypto::generate_opaque_token();
        let token = RefreshToken {
            id: RefreshTokenId::from_uuid(uuid::Uuid::new_v4()),
            user_id,
            org_id,
            token_hash: hash,
            device_hint: device_hint.map(str::to_string),
            issued_at: Utc::now(),
            expires_at: Utc::now() + Duration::days(ttl_days),
            revoked_at: None,
            last_used_at: None,
        };
        self.tokens.lock().unwrap().push(token.clone());
        Ok((token, raw))
    }
    async fn find_by_hash(&self, hash: &str) -> Result<Option<RefreshToken>, RepositoryError> {
        Ok(self.tokens.lock().unwrap().iter().find(|t| {
            t.token_hash == hash && t.revoked_at.is_none() && t.expires_at > Utc::now()
        }).cloned())
    }
    async fn consume(&self, id: RefreshTokenId) -> Result<Option<RefreshToken>, RepositoryError> {
        let mut lock = self.tokens.lock().unwrap();
        if let Some(t) = lock.iter_mut().find(|t| t.id == id && t.revoked_at.is_none()) {
            t.revoked_at = Some(Utc::now());
            return Ok(Some(t.clone()));
        }
        Ok(None)
    }
    async fn revoke(&self, id: RefreshTokenId) -> Result<(), RepositoryError> {
        self.tokens.lock().unwrap().iter_mut()
            .filter(|t| t.id == id)
            .for_each(|t| t.revoked_at = Some(Utc::now()));
        Ok(())
    }
    async fn revoke_all_for_user(&self, user_id: UserId) -> Result<(), RepositoryError> {
        self.tokens.lock().unwrap().iter_mut()
            .filter(|t| t.user_id == user_id && t.revoked_at.is_none())
            .for_each(|t| t.revoked_at = Some(Utc::now()));
        Ok(())
    }
    async fn list_active(&self, user_id: UserId) -> Result<Vec<RefreshToken>, RepositoryError> {
        Ok(self.tokens.lock().unwrap().iter().filter(|t| {
            t.user_id == user_id && t.revoked_at.is_none() && t.expires_at > Utc::now()
        }).cloned().collect())
    }
}

// ---------------------------------------------------------------------------
// Stub for repos not under test
// ---------------------------------------------------------------------------

struct NullRepo;

#[async_trait::async_trait]
impl stitchd_db::UserRepository for NullRepo {
    async fn find_by_id(&self, id: UserId) -> Result<User, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn find_by_email(&self, email: &str) -> Result<User, RepositoryError> { Err(RepositoryError::NotFound { id: email.to_string() }) }
    async fn list_by_organisation(&self, _: OrganisationId) -> Result<Vec<User>, RepositoryError> { Ok(vec![]) }
    async fn create(&self, _: &User) -> Result<(), RepositoryError> { Ok(()) }
    async fn update(&self, u: &User) -> Result<User, RepositoryError> { Ok(u.clone()) }
    async fn find_permissions_for_user(&self, _: UserId, _: stitchd_core::id::ProjectId) -> Result<Vec<stitchd_core::user::Permission>, RepositoryError> { Ok(vec![]) }
}
#[async_trait::async_trait]
impl stitchd_db::FlagRepository for NullRepo {
    async fn find_by_id(&self, id: stitchd_core::id::FlagId) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn find_by_key(&self, key: &stitchd_core::id::FlagKey, _: stitchd_core::id::ProjectId) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> { Err(RepositoryError::NotFound { id: key.to_string() }) }
    async fn list_by_project(&self, _: stitchd_core::id::ProjectId) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> { Ok(vec![]) }
    async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> { Ok(vec![]) }
    async fn create(&self, _: &stitchd_core::flag::FlagRecord) -> Result<(), RepositoryError> { Ok(()) }
    async fn update(&self, f: &stitchd_core::flag::FlagRecord) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> { Ok(f.clone()) }
    async fn soft_delete(&self, id: stitchd_core::id::FlagId) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn find_hashing_config(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, RepositoryError> { Ok(vec![]) }
    async fn upsert_hashing_config(&self, _: stitchd_core::id::FlagId, _: &[stitchd_core::flag::FlagHashingConfig]) -> Result<(), RepositoryError> { Ok(()) }
    async fn find_rules(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::FlagRule>, RepositoryError> { Ok(vec![]) }
    async fn upsert_rules(&self, _: stitchd_core::id::FlagId, _: &[stitchd_core::flag::FlagRule]) -> Result<(), RepositoryError> { Ok(()) }
}
#[async_trait::async_trait]
impl stitchd_db::VariantRepository for NullRepo {
    async fn find_by_flag(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::Variant>, RepositoryError> { Ok(vec![]) }
    async fn create(&self, _: stitchd_core::id::FlagId, _: &stitchd_core::flag::Variant) -> Result<(), RepositoryError> { Ok(()) }
    async fn update(&self, v: &stitchd_core::flag::Variant) -> Result<stitchd_core::flag::Variant, RepositoryError> { Ok(v.clone()) }
    async fn delete(&self, _: stitchd_core::id::VariantId) -> Result<(), RepositoryError> { Ok(()) }
}
#[async_trait::async_trait]
impl stitchd_db::SegmentRepository for NullRepo {
    async fn find_by_id(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::Segment, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn find_by_key(&self, key: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::segment::Segment, RepositoryError> { Err(RepositoryError::NotFound { id: key.to_string() }) }
    async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError> { Ok(vec![]) }
    async fn create(&self, _: &stitchd_core::segment::Segment) -> Result<(), RepositoryError> { Ok(()) }
    async fn update(&self, s: &stitchd_core::segment::Segment) -> Result<stitchd_core::segment::Segment, RepositoryError> { Ok(s.clone()) }
    async fn find_with_rules(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::RuleBasedSegment, RepositoryError> { Ok(stitchd_core::segment::RuleBasedSegment { id, rules: vec![] }) }
    async fn find_with_list(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::ListBasedSegment, RepositoryError> { Ok(stitchd_core::segment::ListBasedSegment { id, lists: std::collections::HashMap::new() }) }
    async fn upsert_rules(&self, _: stitchd_core::id::SegmentId, _: &[stitchd_core::rule_engine::types::Rule]) -> Result<(), RepositoryError> { Ok(()) }
    async fn set_list_entries(&self, _: stitchd_core::id::SegmentId, _: &str, _: &[String], _: &[String]) -> Result<(), RepositoryError> { Ok(()) }
    async fn soft_delete(&self, _: stitchd_core::id::SegmentId) -> Result<(), RepositoryError> { Ok(()) }
    async fn check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &str, _: &str, keys: &[String]) -> Result<HashMap<String, bool>, RepositoryError> { Ok(keys.iter().map(|k| (k.clone(), false)).collect()) }
    async fn batch_check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &[(String, String)], _: &[String]) -> Result<Vec<stitchd_db::ContextMembership>, RepositoryError> { Ok(vec![]) }
}
#[async_trait::async_trait]
impl stitchd_db::SdkKeyRepository for NullRepo {
    async fn find_by_id(&self, id: stitchd_core::id::SdkKeyId) -> Result<stitchd_core::tenant::SdkKey, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, RepositoryError> { Ok(vec![]) }
    async fn create(&self, _: &stitchd_core::tenant::SdkKey) -> Result<(), RepositoryError> { Ok(()) }
    async fn revoke(&self, id: stitchd_core::id::SdkKeyId) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn find_active_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, RepositoryError> { Ok(vec![]) }
    async fn find_active_by_hash(&self, h: &str) -> Result<stitchd_core::tenant::SdkKey, RepositoryError> { Err(RepositoryError::NotFound { id: h.to_string() }) }
}
#[async_trait::async_trait]
impl stitchd_db::EventDefinitionRepository for NullRepo {
    async fn find_by_id(&self, id: stitchd_core::id::EventDefinitionId) -> Result<stitchd_core::event::EventDefinition, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn find_by_key(&self, key: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::event::EventDefinition, RepositoryError> { Err(RepositoryError::NotFound { id: key.to_string() }) }
    async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::event::EventDefinition>, RepositoryError> { Ok(vec![]) }
    async fn create(&self, _: &stitchd_core::event::EventDefinition) -> Result<(), RepositoryError> { Ok(()) }
    async fn update(&self, d: &stitchd_core::event::EventDefinition) -> Result<stitchd_core::event::EventDefinition, RepositoryError> { Ok(d.clone()) }
    async fn soft_delete(&self, id: stitchd_core::id::EventDefinitionId) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
}
#[async_trait::async_trait]
impl stitchd_db::ExperimentRepository for NullRepo {
    async fn find_by_id(&self, id: stitchd_core::id::ExperimentId) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId, _: Option<stitchd_core::experimentation::ExperimentStatus>) -> Result<Vec<stitchd_core::experimentation::Experiment>, RepositoryError> { Ok(vec![]) }
    async fn create(&self, _: &stitchd_core::experimentation::Experiment) -> Result<(), RepositoryError> { Ok(()) }
    async fn update(&self, e: &stitchd_core::experimentation::Experiment) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> { Ok(e.clone()) }
    async fn soft_delete(&self, id: stitchd_core::id::ExperimentId) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
    async fn list_iterations(&self, _: stitchd_core::id::ExperimentId) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, RepositoryError> { Ok(vec![]) }
    async fn apply_transition(&self, id: stitchd_core::id::ExperimentId, _: stitchd_core::experimentation::ExperimentStatus, _: Option<UserId>) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
}
struct NullResults;
#[async_trait::async_trait]
impl stitchd_db::experiment_results::ExperimentResultsRepository for NullResults {
    async fn upsert(&self, _: &stitchd_db::experiment_results::UpsertResultRow) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error> { Err(sqlx::Error::RowNotFound) }
    async fn fetch_latest(&self, _: uuid::Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
    async fn fetch_by_iteration(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
    async fn is_stale(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<bool, sqlx::Error> { Ok(false) }
}
#[async_trait::async_trait]
impl stitchd_db::AuthProviderRepository for NullRepo {
    async fn create(&self, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::ProviderType, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::Unexpected(anyhow::anyhow!("stub"))) }
    async fn find_by_id(&self, _: stitchd_core::id::AuthProviderId) -> Result<Option<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> { Ok(None) }
    async fn list_for_org(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> { Ok(vec![]) }
    async fn update(&self, id: stitchd_core::id::AuthProviderId, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
    async fn delete(&self, _: stitchd_core::id::AuthProviderId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
}

struct NullMfaRepo;
#[async_trait::async_trait]
impl MfaRepository for NullMfaRepo {
    async fn create_challenge(&self, _: stitchd_core::id::UserId, _: i64) -> Result<(stitchd_core::id::MfaChallengeId, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: "stub".to_string() }) }
    async fn consume_challenge(&self, _: &str) -> Result<Option<stitchd_core::id::MfaChallengeId>, stitchd_db::RepositoryError> { Ok(None) }
    async fn enable_totp(&self, _: stitchd_core::id::UserId, _: Vec<u8>, _: Vec<String>) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
    async fn disable_totp(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
    async fn get_totp_secret(&self, _: stitchd_core::id::UserId) -> Result<Option<Vec<u8>>, stitchd_db::RepositoryError> { Ok(None) }
    async fn consume_recovery_code(&self, _: stitchd_core::id::UserId, _: &str) -> Result<bool, stitchd_db::RepositoryError> { Ok(false) }
    async fn store_pending_totp_secret(&self, _: stitchd_core::id::UserId, _: Vec<u8>) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
    async fn get_user_id_for_challenge(&self, _: &str) -> Result<Option<stitchd_core::id::UserId>, stitchd_db::RepositoryError> { Ok(None) }
}

/// In-memory MFA repository for handler tests.
#[derive(Default)]
struct InMemMfaRepo {
    totp_secrets: Mutex<HashMap<UserId, Vec<u8>>>,
    totp_enabled: Mutex<HashMap<UserId, bool>>,
    recovery_codes: Mutex<HashMap<UserId, Vec<String>>>,
    /// Map challenge_token_hash -> user_id for verify tests.
    challenges: Mutex<HashMap<String, (UserId, bool)>>, // (user_id, consumed)
}

impl InMemMfaRepo {
    /// Pre-seed an encrypted TOTP secret for `user_id`.
    fn with_secret(user_id: UserId, encrypted_secret: Vec<u8>) -> Self {
        let mut secrets = HashMap::new();
        secrets.insert(user_id, encrypted_secret);
        Self {
            totp_secrets: Mutex::new(secrets),
            totp_enabled: Mutex::new(HashMap::new()),
            recovery_codes: Mutex::new(HashMap::new()),
            challenges: Mutex::new(HashMap::new()),
        }
    }

    /// Pre-seed a live challenge token hash for a user.
    fn with_challenge(user_id: UserId, encrypted_secret: Vec<u8>, token_hash: String) -> Self {
        let mut secrets = HashMap::new();
        secrets.insert(user_id, encrypted_secret);
        let mut challenges = HashMap::new();
        challenges.insert(token_hash, (user_id, false));
        Self {
            totp_secrets: Mutex::new(secrets),
            totp_enabled: Mutex::new(HashMap::new()),
            recovery_codes: Mutex::new(HashMap::new()),
            challenges: Mutex::new(challenges),
        }
    }
}

#[async_trait::async_trait]
impl MfaRepository for InMemMfaRepo {
    async fn create_challenge(
        &self,
        _: UserId,
        _: i64,
    ) -> Result<(stitchd_core::id::MfaChallengeId, String), RepositoryError> {
        Err(RepositoryError::NotFound { id: "stub".to_string() })
    }

    async fn consume_challenge(
        &self,
        token_hash: &str,
    ) -> Result<Option<stitchd_core::id::MfaChallengeId>, RepositoryError> {
        let mut lock = self.challenges.lock().unwrap();
        if let Some(entry) = lock.get_mut(token_hash) {
            if entry.1 {
                // Already consumed.
                return Ok(None);
            }
            entry.1 = true;
            return Ok(Some(stitchd_core::id::MfaChallengeId::from_uuid(
                uuid::Uuid::new_v4(),
            )));
        }
        Ok(None)
    }

    async fn enable_totp(
        &self,
        user_id: UserId,
        encrypted_secret: Vec<u8>,
        recovery_code_hashes: Vec<String>,
    ) -> Result<(), RepositoryError> {
        self.totp_secrets.lock().unwrap().insert(user_id, encrypted_secret);
        self.totp_enabled.lock().unwrap().insert(user_id, true);
        self.recovery_codes.lock().unwrap().insert(user_id, recovery_code_hashes);
        Ok(())
    }

    async fn disable_totp(&self, user_id: UserId) -> Result<(), RepositoryError> {
        self.totp_secrets.lock().unwrap().remove(&user_id);
        self.totp_enabled.lock().unwrap().insert(user_id, false);
        self.recovery_codes.lock().unwrap().remove(&user_id);
        Ok(())
    }

    async fn get_totp_secret(
        &self,
        user_id: UserId,
    ) -> Result<Option<Vec<u8>>, RepositoryError> {
        Ok(self.totp_secrets.lock().unwrap().get(&user_id).cloned())
    }

    async fn consume_recovery_code(
        &self,
        _: UserId,
        _: &str,
    ) -> Result<bool, RepositoryError> {
        Ok(false)
    }

    async fn store_pending_totp_secret(
        &self,
        user_id: UserId,
        encrypted_secret: Vec<u8>,
    ) -> Result<(), RepositoryError> {
        self.totp_secrets.lock().unwrap().insert(user_id, encrypted_secret);
        Ok(())
    }

    async fn get_user_id_for_challenge(
        &self,
        token_hash: &str,
    ) -> Result<Option<UserId>, RepositoryError> {
        Ok(self
            .challenges
            .lock()
            .unwrap()
            .get(token_hash)
            .map(|(uid, _)| *uid))
    }
}

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Create a test user with a known password hash.
fn make_test_user(status: UserStatus) -> (User, String) {
    let password = "correct-horse-battery".to_string();
    let hash = stitchd_core::auth::crypto::hash_password(&password).unwrap();
    let user = User {
        id: UserId::new(),
        email: "test@example.com".to_string(),
        display_name: "Test User".to_string(),
        avatar_url: None,
        password_hash: Some(hash),
        token_secret: uuid::Uuid::new_v4(),
        totp_secret: None,
        totp_enabled: false,
        status,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    (user, password)
}

/// Build an AppState wired with in-memory repos.
fn make_state(
    user: User,
    org_id: OrganisationId,
    role: OrgRole,
    token_repo: Arc<InMemRefreshTokenRepo>,
) -> AppState {
    let null = Arc::new(NullRepo);
    AppState {
        db: sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
            .expect("lazy pool"),
        metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
        user_repo: null.clone(),
        auth_user_repo: Arc::new(InMemAuthUserRepo::with_user(user.clone())),
        membership_repo: Arc::new(InMemMembershipRepo::with(user.id, org_id, role)),
        refresh_token_repo: token_repo,
        mfa_repo: Arc::new(NullMfaRepo),
        auth_provider_repo: null.clone(),
        segment_repo: null.clone(),
        flag_repo: null.clone(),
        variant_repo: null.clone(),
        sdk_key_repo: null.clone(),
        event_definition_repo: null.clone(),
        experiment_repo: null.clone(),
        results_repo: Arc::new(NullResults),
        ch_client: None,
        event_writer: None,
        oidc_state_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    }
}

fn app_from_state(state: AppState) -> Router {
    build_api_router().with_state(state)
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_success_single_org_returns_tokens() {
    let (user, password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();
    let tokens = Arc::new(InMemRefreshTokenRepo::default());
    let app = app_from_state(make_state(user, org_id, OrgRole::OrgMember, tokens));

    let body = serde_json::json!({ "email": "test@example.com", "password": password });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["access_token"].is_string());
    assert!(json["refresh_token"].is_string());
    assert_eq!(json["token_type"], "Bearer");
}

#[tokio::test]
async fn login_wrong_password_returns_401() {
    let (user, _) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();
    let tokens = Arc::new(InMemRefreshTokenRepo::default());
    let app = app_from_state(make_state(user, org_id, OrgRole::OrgMember, tokens));

    let body = serde_json::json!({ "email": "test@example.com", "password": "wrongpassword" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_deactivated_user_returns_403() {
    let (user, password) = make_test_user(UserStatus::Deactivated);
    let org_id = OrganisationId::new();
    let tokens = Arc::new(InMemRefreshTokenRepo::default());
    let app = app_from_state(make_state(user, org_id, OrgRole::OrgMember, tokens));

    let body = serde_json::json!({ "email": "test@example.com", "password": password });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn refresh_success_rotates_token() {
    let (user, password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();
    let token_repo = Arc::new(InMemRefreshTokenRepo::default());
    let state = make_state(user.clone(), org_id, OrgRole::OrgMember, token_repo.clone());
    let app = app_from_state(state);

    // Login first.
    let login_body = serde_json::json!({ "email": "test@example.com", "password": password });
    let login_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_refresh = login_json["refresh_token"].as_str().unwrap().to_string();

    // Refresh.
    let refresh_body = serde_json::json!({ "refresh_token": raw_refresh });
    let refresh_resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("Content-Type", "application/json")
                .body(Body::from(refresh_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refresh_resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(refresh_resp.into_body(), usize::MAX).await.unwrap();
    let refresh_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let new_refresh = refresh_json["refresh_token"].as_str().unwrap();
    assert_ne!(new_refresh, raw_refresh, "new token should differ from old");
}

#[tokio::test]
async fn refresh_expired_token_returns_401() {
    let (user, _) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();
    let tokens = Arc::new(InMemRefreshTokenRepo::default());
    let app = app_from_state(make_state(user, org_id, OrgRole::OrgMember, tokens));

    let body = serde_json::json!({ "refresh_token": "notavalidtoken" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoke_all_sessions_returns_204() {
    let (user, password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();
    let token_repo = Arc::new(InMemRefreshTokenRepo::default());
    let state = make_state(user.clone(), org_id, OrgRole::OrgMember, token_repo.clone());
    let app = app_from_state(state);

    // Login to get a JWT.
    let login_body = serde_json::json!({ "email": "test@example.com", "password": password });
    let login_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let access_token = login_json["access_token"].as_str().unwrap().to_string();

    // Sign out all.
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/auth/sessions")
                .header("Authorization", bearer(&access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn switch_org_success_returns_new_tokens() {
    let (user, password) = make_test_user(UserStatus::Active);
    let org1 = OrganisationId::new();
    let org2 = OrganisationId::new();

    let token_repo = Arc::new(InMemRefreshTokenRepo::default());
    let null = Arc::new(NullRepo);

    // Set up two org memberships.
    let membership_repo = Arc::new(InMemMembershipRepo::default());
    {
        let mut lock = membership_repo.memberships.lock().unwrap();
        lock.push(OrgMembership { user_id: user.id, org_id: org1, role: OrgRole::OrgMember, joined_at: Utc::now() });
        lock.push(OrgMembership { user_id: user.id, org_id: org2, role: OrgRole::OrgAdmin, joined_at: Utc::now() });
    }

    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
            .expect("lazy pool"),
        metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
        user_repo: null.clone(),
        auth_user_repo: Arc::new(InMemAuthUserRepo::with_user(user.clone())),
        membership_repo,
        refresh_token_repo: token_repo,
        mfa_repo: Arc::new(NullMfaRepo),
        auth_provider_repo: null.clone(),
        segment_repo: null.clone(),
        flag_repo: null.clone(),
        variant_repo: null.clone(),
        sdk_key_repo: null.clone(),
        event_definition_repo: null.clone(),
        experiment_repo: null.clone(),
        results_repo: Arc::new(NullResults),
        ch_client: None,
        event_writer: None,
        oidc_state_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    let app = app_from_state(state);

    // Login to org1.
    let login_body = serde_json::json!({
        "email": "test@example.com",
        "password": password,
        "org_id": org1.as_uuid()
    });
    let login_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::OK, "login should succeed");
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let access_token = login_json["access_token"].as_str().unwrap().to_string();

    // Switch to org2.
    let switch_body = serde_json::json!({ "org_id": org2.as_uuid() });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/switch-org")
                .header("Authorization", bearer(&access_token))
                .header("Content-Type", "application/json")
                .body(Body::from(switch_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let switch_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // Verify the new JWT contains org2.
    let new_access = switch_json["access_token"].as_str().unwrap();
    let claims = JwtEngine::decode_unverified(new_access).unwrap();
    assert_eq!(
        claims.org_id,
        org2.as_uuid().to_string(),
        "token should be scoped to org2"
    );
}

#[tokio::test]
async fn switch_org_not_member_returns_403() {
    let (user, password) = make_test_user(UserStatus::Active);
    let org1 = OrganisationId::new();
    let unknown_org = OrganisationId::new();
    let tokens = Arc::new(InMemRefreshTokenRepo::default());
    let app = app_from_state(make_state(user.clone(), org1, OrgRole::OrgMember, tokens));

    // Login.
    let login_body = serde_json::json!({ "email": "test@example.com", "password": password });
    let login_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let access_token = login_json["access_token"].as_str().unwrap().to_string();

    // Try switching to unknown org.
    let switch_body = serde_json::json!({ "org_id": unknown_org.as_uuid() });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/switch-org")
                .header("Authorization", bearer(&access_token))
                .header("Content-Type", "application/json")
                .body(Body::from(switch_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// MFA handler tests
// ---------------------------------------------------------------------------

/// Set `AUTH_ENCRYPTION_KEY` to a stable test value (32 zero bytes, base64).
/// Returns a [`CryptoKey`] backed by the same bytes for symmetric round-trips.
fn set_test_encryption_key() -> stitchd_core::auth::crypto::CryptoKey {
    // base64(32 × 0x00) — acceptable for tests only.
    // SAFETY: single-threaded test setup; no other thread reads this env var
    //         until after this function returns.
    unsafe {
        std::env::set_var(
            "AUTH_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
    stitchd_core::auth::crypto::CryptoKey::from_bytes([0u8; 32])
}

/// Build an `AppState` with `InMemMfaRepo` and the given `user` / `org`.
fn make_mfa_state(
    user: User,
    org_id: OrganisationId,
    mfa_repo: Arc<InMemMfaRepo>,
) -> AppState {
    let null = Arc::new(NullRepo);
    AppState {
        db: sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
            .expect("lazy pool"),
        metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
        user_repo: null.clone(),
        auth_user_repo: Arc::new(InMemAuthUserRepo::with_user(user.clone())),
        membership_repo: Arc::new(InMemMembershipRepo::with(user.id, org_id, OrgRole::OrgMember)),
        refresh_token_repo: Arc::new(InMemRefreshTokenRepo::default()),
        mfa_repo,
        auth_provider_repo: null.clone(),
        segment_repo: null.clone(),
        flag_repo: null.clone(),
        variant_repo: null.clone(),
        sdk_key_repo: null.clone(),
        event_definition_repo: null.clone(),
        experiment_repo: null.clone(),
        results_repo: Arc::new(NullResults),
        ch_client: None,
        event_writer: None,
        oidc_state_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    }
}

#[tokio::test]
async fn mfa_setup_returns_otpauth_uri() {
    let crypto = set_test_encryption_key();
    let (user, password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();

    // Login to get an access token.
    let token_repo = Arc::new(InMemRefreshTokenRepo::default());
    let login_app =
        app_from_state(make_state(user.clone(), org_id, OrgRole::OrgMember, token_repo));
    let login_body = serde_json::json!({ "email": "test@example.com", "password": password });
    let login_resp = login_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let access_token = login_json["access_token"].as_str().unwrap().to_string();

    // Call setup.
    let mfa_repo = Arc::new(InMemMfaRepo::default());
    let app = app_from_state(make_mfa_state(user.clone(), org_id, mfa_repo.clone()));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/users/me/mfa/setup")
                .header("Authorization", bearer(&access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let uri = json["otpauth_uri"].as_str().unwrap();
    assert!(uri.starts_with("otpauth://totp/"), "must be otpauth URI");
    assert!(json["secret_base32"].is_string(), "must include base32 secret");
    // Verify the pending secret was stored in the repo.
    let stored = mfa_repo.get_totp_secret(user.id).await.unwrap();
    assert!(stored.is_some(), "pending secret must be stored");
    // Round-trip decrypt to confirm it's valid ciphertext.
    let _ = crypto.decrypt(&stored.unwrap()).expect("encrypted secret must decrypt");
}

#[tokio::test]
async fn mfa_confirm_valid_code_enables_mfa_and_returns_recovery_codes() {
    let crypto = set_test_encryption_key();
    let (user, password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();

    // Generate a real TOTP secret and encrypt it to pre-seed the repo.
    let (secret_bytes, _uri) =
        stitchd_core::auth::TotpEngine::generate_secret(&user.email).unwrap();
    let encrypted = crypto.encrypt(&secret_bytes).unwrap();

    // Build current TOTP code.
    let totp = totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,
        30,
        secret_bytes.clone(),
        Some("Stitchd".to_owned()),
        user.email.clone(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let totp_code = totp.generate(now);

    // Pre-seed repo with the pending (unconfirmed) secret.
    let mfa_repo = Arc::new(InMemMfaRepo::with_secret(user.id, encrypted));

    // Login to get access token.
    let token_repo = Arc::new(InMemRefreshTokenRepo::default());
    let login_app =
        app_from_state(make_state(user.clone(), org_id, OrgRole::OrgMember, token_repo));
    let login_body = serde_json::json!({ "email": "test@example.com", "password": password });
    let login_resp = login_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let access_token = login_json["access_token"].as_str().unwrap().to_string();

    // Call confirm.
    let app = app_from_state(make_mfa_state(user.clone(), org_id, mfa_repo.clone()));
    let confirm_body = serde_json::json!({ "totp_code": totp_code });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/users/me/mfa/confirm")
                .header("Authorization", bearer(&access_token))
                .header("Content-Type", "application/json")
                .body(Body::from(confirm_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let codes = json["recovery_codes"].as_array().unwrap();
    assert_eq!(codes.len(), 10, "must return 10 recovery codes");
    // MFA should now be enabled in the repo.
    let enabled = mfa_repo.totp_enabled.lock().unwrap().get(&user.id).copied();
    assert_eq!(enabled, Some(true), "totp_enabled must be set to true");
}

#[tokio::test]
async fn mfa_confirm_invalid_code_returns_400() {
    let crypto = set_test_encryption_key();
    let (user, password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();

    let (secret_bytes, _) =
        stitchd_core::auth::TotpEngine::generate_secret(&user.email).unwrap();
    let encrypted = crypto.encrypt(&secret_bytes).unwrap();
    let mfa_repo = Arc::new(InMemMfaRepo::with_secret(user.id, encrypted));

    // Login.
    let token_repo = Arc::new(InMemRefreshTokenRepo::default());
    let login_app =
        app_from_state(make_state(user.clone(), org_id, OrgRole::OrgMember, token_repo));
    let login_body = serde_json::json!({ "email": "test@example.com", "password": password });
    let login_resp = login_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let access_token = login_json["access_token"].as_str().unwrap().to_string();

    // Confirm with a deliberately wrong code.
    // "000000" is almost certainly wrong (1-in-1,000,000 chance of collision).
    let app = app_from_state(make_mfa_state(user, org_id, mfa_repo));
    let confirm_body = serde_json::json!({ "totp_code": "000000" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/users/me/mfa/confirm")
                .header("Authorization", bearer(&access_token))
                .header("Content-Type", "application/json")
                .body(Body::from(confirm_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mfa_verify_valid_totp_returns_tokens() {
    let crypto = set_test_encryption_key();
    let (user, _password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();

    // Create TOTP secret and encrypt.
    let (secret_bytes, _) =
        stitchd_core::auth::TotpEngine::generate_secret(&user.email).unwrap();
    let encrypted = crypto.encrypt(&secret_bytes).unwrap();

    // Build the current TOTP code.
    let totp = totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,
        30,
        secret_bytes.clone(),
        Some("Stitchd".to_owned()),
        user.email.clone(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let totp_code = totp.generate(now);

    // Create a fake raw challenge token and compute its hash.
    let raw_token = "aabbccdd".repeat(8); // 64-char hex-like string
    let token_hash = stitchd_db::challenge_token_hash(&raw_token);

    // Pre-seed InMemMfaRepo with the encrypted secret and the challenge.
    let mfa_repo = Arc::new(InMemMfaRepo::with_challenge(
        user.id,
        encrypted,
        token_hash,
    ));

    let app = app_from_state(make_mfa_state(user, org_id, mfa_repo));
    let verify_body = serde_json::json!({
        "challenge_token": raw_token,
        "totp_code": totp_code,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/mfa/verify")
                .header("Content-Type", "application/json")
                .body(Body::from(verify_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["access_token"].is_string(), "must return access_token");
    assert!(json["refresh_token"].is_string(), "must return refresh_token");
}

#[tokio::test]
async fn mfa_verify_expired_challenge_returns_401() {
    set_test_encryption_key();
    let (user, _) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();

    // No challenge pre-seeded — consume_challenge returns None.
    let mfa_repo = Arc::new(InMemMfaRepo::default());
    let app = app_from_state(make_mfa_state(user, org_id, mfa_repo));

    let verify_body =
        serde_json::json!({ "challenge_token": "nonexistent", "totp_code": "000000" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/mfa/verify")
                .header("Content-Type", "application/json")
                .body(Body::from(verify_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mfa_disable_valid_code_returns_204() {
    let crypto = set_test_encryption_key();
    let (user, password) = make_test_user(UserStatus::Active);
    let org_id = OrganisationId::new();

    let (secret_bytes, _) =
        stitchd_core::auth::TotpEngine::generate_secret(&user.email).unwrap();
    let encrypted = crypto.encrypt(&secret_bytes).unwrap();

    // Build current TOTP code.
    let totp = totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,
        30,
        secret_bytes.clone(),
        Some("Stitchd".to_owned()),
        user.email.clone(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let totp_code = totp.generate(now);

    let mfa_repo = Arc::new(InMemMfaRepo::with_secret(user.id, encrypted));

    // Login to get access token.
    let token_repo = Arc::new(InMemRefreshTokenRepo::default());
    let login_app =
        app_from_state(make_state(user.clone(), org_id, OrgRole::OrgMember, token_repo));
    let login_body = serde_json::json!({ "email": "test@example.com", "password": password });
    let login_resp = login_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(login_resp.into_body(), usize::MAX).await.unwrap();
    let login_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let access_token = login_json["access_token"].as_str().unwrap().to_string();

    // Disable MFA with valid TOTP code.
    let app = app_from_state(make_mfa_state(user.clone(), org_id, mfa_repo.clone()));
    let disable_body = serde_json::json!({ "totp_code": totp_code });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/users/me/mfa/disable")
                .header("Authorization", bearer(&access_token))
                .header("Content-Type", "application/json")
                .body(Body::from(disable_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    // Secret should be cleared.
    let stored = mfa_repo.get_totp_secret(user.id).await.unwrap();
    assert!(stored.is_none(), "secret must be cleared after disable");
}
