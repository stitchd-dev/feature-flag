//! Repository trait definitions — one per aggregate root.
//!
//! Each trait is `async_trait` and returns `Result<T, RepositoryError>`.
//! Concrete Postgres implementations live in `pg/`.

use async_trait::async_trait;
use uuid::Uuid;

use stitchd_core::{
    flag::Variant,
    id::{
        EnvironmentId, FlagId, FlagKey, OrganisationId, ProjectId, RoleId, SdkKeyId, SegmentId,
        UserId, VariantId,
    },
    tenant::{Environment, Organisation, Project, SdkKey},
    user::{Permission, Role, User},
};

use crate::RepositoryError;

pub mod pg;

// ---------------------------------------------------------------------------
// Organisation
// ---------------------------------------------------------------------------

/// CRUD operations for [`Organisation`] aggregates.
#[async_trait]
pub trait OrganisationRepository: Send + Sync {
    /// Fetch a single organisation by ID.
    async fn find_by_id(&self, id: OrganisationId) -> Result<Organisation, RepositoryError>;

    /// List all non-deleted organisations.
    async fn list_all(&self) -> Result<Vec<Organisation>, RepositoryError>;

    /// Persist a new organisation.
    async fn create(&self, org: &Organisation) -> Result<(), RepositoryError>;

    /// Update an existing organisation (optimistic concurrency via `version`).
    async fn update(&self, org: &Organisation) -> Result<Organisation, RepositoryError>;

    /// Soft-delete an organisation by setting `deleted_at`.
    async fn soft_delete(&self, id: OrganisationId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

/// CRUD operations for [`Project`] aggregates.
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// Fetch a single project by ID.
    async fn find_by_id(&self, id: ProjectId) -> Result<Project, RepositoryError>;

    /// List all non-deleted projects belonging to an organisation.
    async fn list_by_organisation(
        &self,
        organisation_id: OrganisationId,
    ) -> Result<Vec<Project>, RepositoryError>;

    /// Persist a new project.
    async fn create(&self, project: &Project) -> Result<(), RepositoryError>;

    /// Update an existing project.
    async fn update(&self, project: &Project) -> Result<Project, RepositoryError>;

    /// Soft-delete a project.
    async fn soft_delete(&self, id: ProjectId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// CRUD operations for [`Environment`] aggregates.
#[async_trait]
pub trait EnvironmentRepository: Send + Sync {
    /// Fetch a single environment by ID.
    async fn find_by_id(&self, id: EnvironmentId) -> Result<Environment, RepositoryError>;

    /// List all non-deleted environments belonging to a project.
    async fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<Environment>, RepositoryError>;

    /// Persist a new environment.
    async fn create(&self, env: &Environment) -> Result<(), RepositoryError>;

    /// Update an existing environment.
    async fn update(&self, env: &Environment) -> Result<Environment, RepositoryError>;

    /// Soft-delete an environment.
    async fn soft_delete(&self, id: EnvironmentId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// SdkKey
// ---------------------------------------------------------------------------

/// Operations for [`SdkKey`] records scoped to an environment.
#[async_trait]
pub trait SdkKeyRepository: Send + Sync {
    /// Fetch a single SDK key by ID.
    async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError>;

    /// List all SDK keys for an environment (active and revoked).
    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<SdkKey>, RepositoryError>;

    /// Persist a new SDK key.
    async fn create(&self, key: &SdkKey) -> Result<(), RepositoryError>;

    /// Revoke a key by setting `is_active = false` and `revoked_at = now()`.
    ///
    /// Returns [`RepositoryError::UniqueViolation`] if revoking this key
    /// would leave the environment with zero active keys.
    async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

/// Operations for [`User`] records.
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Fetch a single user by ID.
    async fn find_by_id(&self, id: UserId) -> Result<User, RepositoryError>;

    /// Fetch a user by email within an organisation.
    async fn find_by_email(
        &self,
        email: &str,
        organisation_id: OrganisationId,
    ) -> Result<User, RepositoryError>;

    /// List all non-deleted users in an organisation.
    async fn list_by_organisation(
        &self,
        organisation_id: OrganisationId,
    ) -> Result<Vec<User>, RepositoryError>;

    /// Persist a new user.
    async fn create(&self, user: &User) -> Result<(), RepositoryError>;

    /// Update an existing user.
    async fn update(&self, user: &User) -> Result<User, RepositoryError>;

    /// Resolve all permissions a user has within a specific project.
    ///
    /// Joins `user_project_roles → roles → permissions`.
    async fn find_permissions_for_user(
        &self,
        user_id: UserId,
        project_id: ProjectId,
    ) -> Result<Vec<Permission>, RepositoryError>;
}

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// Operations for [`Role`] records.
#[async_trait]
pub trait RoleRepository: Send + Sync {
    /// Fetch a single role by ID.
    async fn find_by_id(&self, id: RoleId) -> Result<Role, RepositoryError>;

    /// List all roles for a project.
    async fn list_by_project(&self, project_id: ProjectId) -> Result<Vec<Role>, RepositoryError>;

    /// Persist a new role.
    async fn create(&self, role: &Role) -> Result<(), RepositoryError>;

    /// Update an existing role.
    async fn update(&self, role: &Role) -> Result<Role, RepositoryError>;

    /// Soft-delete a role.
    async fn soft_delete(&self, id: RoleId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Flag
// ---------------------------------------------------------------------------

/// Operations for feature flag definitions.
#[async_trait]
pub trait FlagRepository: Send + Sync {
    /// Fetch a flag by ID.
    async fn find_by_id(
        &self,
        id: FlagId,
    ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError>;

    /// Fetch a flag by its string key within a project.
    async fn find_by_key(
        &self,
        key: &FlagKey,
        project_id: ProjectId,
    ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError>;

    /// List all non-deleted flags in a project.
    async fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError>;

    /// List all non-deleted flags in an environment.
    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError>;

    /// Persist a new flag.
    async fn create(&self, flag: &stitchd_core::flag::FlagRecord) -> Result<(), RepositoryError>;

    /// Update an existing flag.
    async fn update(
        &self,
        flag: &stitchd_core::flag::FlagRecord,
    ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError>;

    /// Soft-delete a flag.
    async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError>;

    /// Fetch the hashing configuration for a flag.
    async fn find_hashing_config(
        &self,
        flag_id: FlagId,
    ) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, RepositoryError>;

    /// Upsert the hashing configuration for a flag (replaces existing).
    async fn upsert_hashing_config(
        &self,
        flag_id: FlagId,
        config: &[stitchd_core::flag::FlagHashingConfig],
    ) -> Result<(), RepositoryError>;

    /// Fetch all rules for a flag, ordered by `rule_index`.
    async fn find_rules(
        &self,
        flag_id: FlagId,
    ) -> Result<Vec<stitchd_core::flag::FlagRule>, RepositoryError>;

    /// Upsert rules for a flag (replaces existing).
    async fn upsert_rules(
        &self,
        flag_id: FlagId,
        rules: &[stitchd_core::flag::FlagRule],
    ) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Variant
// ---------------------------------------------------------------------------

/// Operations for flag variants.
#[async_trait]
pub trait VariantRepository: Send + Sync {
    /// List all variants for a flag.
    async fn find_by_flag(&self, flag_id: FlagId) -> Result<Vec<Variant>, RepositoryError>;

    /// Persist a new variant.
    async fn create(&self, flag_id: FlagId, variant: &Variant) -> Result<(), RepositoryError>;

    /// Update an existing variant.
    async fn update(&self, variant: &Variant) -> Result<Variant, RepositoryError>;

    /// Delete a variant permanently (variants don't soft-delete).
    async fn delete(&self, id: VariantId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Segment
// ---------------------------------------------------------------------------

/// Operations for segment definitions.
#[async_trait]
pub trait SegmentRepository: Send + Sync {
    /// Fetch a segment by ID.
    async fn find_by_id(
        &self,
        id: SegmentId,
    ) -> Result<stitchd_core::segment::Segment, RepositoryError>;

    /// Fetch a segment by key within an environment.
    async fn find_by_key(
        &self,
        key: &str,
        environment_id: EnvironmentId,
    ) -> Result<stitchd_core::segment::Segment, RepositoryError>;

    /// List all non-deleted segments in an environment.
    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError>;

    /// Persist a new segment.
    async fn create(&self, segment: &stitchd_core::segment::Segment)
    -> Result<(), RepositoryError>;

    /// Update an existing segment.
    async fn update(
        &self,
        segment: &stitchd_core::segment::Segment,
    ) -> Result<stitchd_core::segment::Segment, RepositoryError>;

    /// Fetch a rule-based segment definition.
    async fn find_with_rules(
        &self,
        id: SegmentId,
    ) -> Result<stitchd_core::segment::RuleBasedSegment, RepositoryError>;

    /// Fetch a list-based segment definition.
    async fn find_with_list(
        &self,
        id: SegmentId,
    ) -> Result<stitchd_core::segment::ListBasedSegment, RepositoryError>;

    /// Upsert rule definitions for a segment (replaces all existing rules).
    async fn upsert_rules(
        &self,
        id: SegmentId,
        rules: &[stitchd_core::rule_engine::types::Rule],
    ) -> Result<(), RepositoryError>;

    /// Replace list entries for a specific context type within a segment.
    async fn set_list_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        include: &[String],
        exclude: &[String],
    ) -> Result<(), RepositoryError>;

    /// Soft-delete a segment.
    async fn soft_delete(&self, id: SegmentId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// AuditLogger
// ---------------------------------------------------------------------------

/// Writes append-only audit log entries for all repository mutations.
#[async_trait]
pub trait AuditLogger: Send + Sync {
    /// Record a mutation event.
    ///
    /// - `actor_id`: the user who performed the action (`None` for system actions).
    /// - `resource_type`: string discriminant, e.g. `"organisation"`, `"flag"`.
    /// - `resource_id`: UUID of the affected entity.
    /// - `action`: verb, e.g. `"create"`, `"update"`, `"soft_delete"`.
    /// - `diff`: JSON snapshot of changed fields (may be `{}`).
    async fn log(
        &self,
        actor_id: Option<UserId>,
        resource_type: &str,
        resource_id: Uuid,
        action: &str,
        diff: serde_json::Value,
    ) -> Result<(), RepositoryError>;
}
