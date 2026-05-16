//! Repository trait definitions — one per aggregate root.
//!
//! Each trait is `async_trait` and returns `Result<T, RepositoryError>`.
//! Concrete Postgres implementations live in `pg/`.

use async_trait::async_trait;
use uuid::Uuid;

use stitchd_core::{
    auth::User,
    event::EventDefinition,
    experimentation::{Experiment, ExperimentIteration, ExperimentStatus},
    flag::Variant,
    id::{
        EnvironmentId, EventDefinitionId, ExperimentId, FlagId, FlagKey, OrganisationId, ProjectId,
        RoleId, SdkKeyId, SegmentId, UserId, VariantId,
    },
    tenant::{Environment, Organisation, Project, SdkKey},
    user::{Permission, Role},
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

    /// List only active (non-revoked) SDK keys for an environment.
    async fn find_active_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<SdkKey>, RepositoryError>;

    /// Find an active SDK key by its stored hash (for gRPC auth where `env_id` is unknown).
    ///
    /// Returns [`RepositoryError::NotFound`] if no active key with that hash exists.
    async fn find_active_by_hash(&self, key_hash: &str) -> Result<SdkKey, RepositoryError>;
}

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

/// Operations for [`User`] records.
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Fetch a single user by ID.
    async fn find_by_id(&self, id: UserId) -> Result<User, RepositoryError>;

    /// Fetch a user by email (platform-wide — email is globally unique).
    async fn find_by_email(&self, email: &str) -> Result<User, RepositoryError>;

    /// List all users who are members of an organisation.
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
    /// Joins `user_project_roles` (new schema with direct `role` TEXT column).
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

    /// List non-deleted flags in a project with offset pagination.
    ///
    /// Returns `(page_items, total_count)` where `total_count` is the count of
    /// all non-deleted flags for the project (regardless of offset/limit).
    async fn list_by_project_paginated(
        &self,
        project_id: ProjectId,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<stitchd_core::flag::FlagRecord>, u64), RepositoryError>;

    /// List all flags in a project including soft-deleted (archived) ones.
    async fn list_by_project_all(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError>;

    /// List all non-deleted flags in an environment.
    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError>;

    /// List flags in an environment, optionally including archived (soft-deleted) ones.
    async fn list_by_environment_all(
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

    /// Replace all variants for a flag atomically (delete all, then insert).
    async fn replace_all_for_flag(
        &self,
        flag_id: FlagId,
        variants: &[Variant],
    ) -> Result<(), RepositoryError>;
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

    /// List non-deleted segments in an environment with offset pagination.
    ///
    /// Returns `(page_items, total_count)`.
    async fn list_by_environment_paginated(
        &self,
        environment_id: EnvironmentId,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<stitchd_core::segment::Segment>, u64), RepositoryError>;

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

    /// Fetch the raw ConditionExpr JSON for a rule-based segment (admin UI).
    /// Returns `None` if the segment has no condition set yet.
    async fn get_condition_expr(
        &self,
        id: SegmentId,
    ) -> Result<Option<serde_json::Value>, RepositoryError>;

    /// Persist (overwrite) the raw ConditionExpr JSON for a rule-based segment.
    /// Pass `None` to clear it.
    async fn set_condition_expr(
        &self,
        id: SegmentId,
        expr: Option<&serde_json::Value>,
    ) -> Result<(), RepositoryError>;

    /// Soft-delete a segment.
    async fn soft_delete(&self, id: SegmentId) -> Result<(), RepositoryError>;

    /// Check list-segment membership for one context across multiple segment keys.
    ///
    /// Returns a map from `segment_key → is_member`. Segments that do not exist
    /// or are not of type `list` are omitted from the result.
    /// Exclude entries take precedence over include entries.
    async fn check_list_membership(
        &self,
        environment_id: EnvironmentId,
        context_type: &str,
        context_key: &str,
        segment_keys: &[String],
    ) -> Result<std::collections::HashMap<String, bool>, RepositoryError>;

    /// Batch check list-segment membership for multiple contexts across multiple segment keys.
    ///
    /// Returns one entry per context, each with a map of `segment_key → is_member`.
    async fn batch_check_list_membership(
        &self,
        environment_id: EnvironmentId,
        contexts: &[(String, String)],
        segment_keys: &[String],
    ) -> Result<Vec<crate::ContextMembership>, RepositoryError>;

    // ---- N+1 elimination methods (FR-2) ----

    /// Fetch multiple segment records in one query.
    ///
    /// Returns all non-deleted segments whose IDs are in `ids`, in unspecified order.
    async fn find_batch_by_ids(
        &self,
        ids: &[SegmentId],
    ) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError>;

    /// Fetch rule definitions for multiple rule-based segments in one query.
    ///
    /// Returns a map from segment ID to its assembled [`RuleBasedSegment`].
    /// Segments with no `segment_rules` rows fall back to the `condition_expr` column.
    async fn find_rules_batch(
        &self,
        ids: &[SegmentId],
    ) -> Result<std::collections::HashMap<SegmentId, stitchd_core::segment::RuleBasedSegment>, RepositoryError>;

    /// Fetch list entries for multiple list-based segments in one query.
    ///
    /// Returns a map from segment ID to its assembled [`ListBasedSegment`].
    async fn find_lists_batch(
        &self,
        ids: &[SegmentId],
    ) -> Result<std::collections::HashMap<SegmentId, stitchd_core::segment::ListBasedSegment>, RepositoryError>;
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

// ---------------------------------------------------------------------------
// Experiment
// ---------------------------------------------------------------------------

/// CRUD operations for [`Experiment`] records scoped to an environment.
#[async_trait]
pub trait ExperimentRepository: Send + Sync {
    /// Fetch a single experiment by ID (excludes soft-deleted).
    async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError>;

    /// List all non-deleted experiments for an environment, optionally filtered by status.
    async fn list_by_environment(
        &self,
        env_id: EnvironmentId,
        status_filter: Option<ExperimentStatus>,
    ) -> Result<Vec<Experiment>, RepositoryError>;

    /// Persist a new experiment.
    async fn create(&self, experiment: &Experiment) -> Result<(), RepositoryError>;

    /// Update an existing experiment (optimistic locking via `version`).
    async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError>;

    /// Soft-delete an experiment.
    ///
    /// Returns [`RepositoryError::InvalidState`] if the experiment is currently running.
    async fn soft_delete(&self, id: ExperimentId) -> Result<(), RepositoryError>;

    /// List all iterations for an experiment, ordered by `iteration_number`.
    async fn list_iterations(
        &self,
        experiment_id: ExperimentId,
    ) -> Result<Vec<ExperimentIteration>, RepositoryError>;

    /// Apply a status transition to an experiment.
    ///
    /// On transition into `running`:
    /// - Creates a new `experiment_iterations` row (snapshot of current `metric_keys`,
    ///   `traffic_allocation`, `min_sample_size`)
    /// - Sets `feature_flag_rules.frozen = true` for the experiment's `flag_rule_id`
    ///
    /// On transition into `paused` or `stopped`:
    /// - Sets `feature_flag_rules.frozen = false` for the experiment's `flag_rule_id`
    /// - If ending an iteration (running→paused, running→stopped): sets `ended_at` = `now()` on the current active iteration
    ///
    /// Uniqueness: if another experiment is already running or paused on the same `flag_rule_id`,
    /// return `RepositoryError::UniqueViolation { field: "flag_rule_id" }`.
    ///
    /// Invalid transition: return `RepositoryError::InvalidState { reason }`.
    async fn apply_transition(
        &self,
        id: ExperimentId,
        to: ExperimentStatus,
        actor_id: Option<UserId>,
    ) -> Result<Experiment, RepositoryError>;
}

// ---------------------------------------------------------------------------
// ContextRegistry
// ---------------------------------------------------------------------------

/// Upsert-and-query operations for the context type / parameter registry.
///
/// All list methods apply a 90-day recency filter on `last_seen_at`.
#[async_trait]
pub trait ContextRegistryRepository: Send + Sync {
    /// Record or refresh a context type observation.
    ///
    /// Sets `first_seen_at` on insert; always bumps `last_seen_at = now()`.
    async fn upsert_context_type(
        &self,
        env_id: EnvironmentId,
        context_type: &str,
    ) -> Result<(), RepositoryError>;

    /// Record or refresh a context parameter observation.
    ///
    /// Sets `first_seen_at` on insert; always bumps `last_seen_at = now()` and
    /// updates `inferred_type` and `is_private`.
    async fn upsert_param(
        &self,
        env_id: EnvironmentId,
        context_type: &str,
        param_key: &str,
        inferred_type: stitchd_core::context::InferredType,
        is_private: bool,
    ) -> Result<(), RepositoryError>;

    /// List context types for an environment seen within the last 90 days,
    /// ordered by `last_seen_at DESC`.
    async fn list_types(
        &self,
        env_id: EnvironmentId,
    ) -> Result<Vec<stitchd_core::context::ContextTypeRecord>, RepositoryError>;

    /// List params for a context type in an environment seen within the last 90 days,
    /// ordered by `param_key ASC`.
    async fn list_params(
        &self,
        env_id: EnvironmentId,
        context_type: &str,
    ) -> Result<Vec<stitchd_core::context::ContextParamRecord>, RepositoryError>;

    /// Delete registry entries not observed since `older_than`.
    ///
    /// Purges params first, then types (FK-safe order without requiring an actual FK).
    async fn purge_stale(
        &self,
        older_than: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// EventDefinition
// ---------------------------------------------------------------------------

/// CRUD operations for [`EventDefinition`] records scoped to an environment.
#[async_trait]
pub trait EventDefinitionRepository: Send + Sync {
    /// Fetch a definition by ID.
    async fn find_by_id(&self, id: EventDefinitionId) -> Result<EventDefinition, RepositoryError>;

    /// Fetch a definition by its string key within an environment.
    async fn find_by_key(
        &self,
        key: &str,
        environment_id: EnvironmentId,
    ) -> Result<EventDefinition, RepositoryError>;

    /// List all non-deleted definitions for an environment.
    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<EventDefinition>, RepositoryError>;

    /// Persist a new definition.
    async fn create(&self, def: &EventDefinition) -> Result<(), RepositoryError>;

    /// Update an existing definition (optimistic locking via `version`).
    async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError>;

    /// Soft-delete a definition by setting `deleted_at`.
    async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError>;
}
