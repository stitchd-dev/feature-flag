//! Database access layer: sqlx queries, migrations, and repository implementations.
//!
//! All SQL queries use compile-time checked `sqlx::query!` / `sqlx::query_as!` macros.
//! Schema changes are managed via migrations in `migrations/`.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod auth;
pub mod clickhouse;
pub mod error;
pub mod experiment_results;
pub mod repository;

pub use auth::{
    AuthUserRepository, MfaChallengeRepository, MfaRepository, OrgMembershipRepository,
    PgAuthUserRepository, PgMfaChallengeRepository, PgMfaRepository,
    PgOrgMembershipRepository, PgRefreshTokenRepository, RefreshTokenRepository,
    challenge_token_hash,
};
pub use error::RepositoryError;
pub use experiment_results::{
    ExperimentResultRow, ExperimentResultsRepository, PgExperimentResultsRepository,
    UpsertResultRow,
};
pub use repository::{
    AuditLogger, EnvironmentRepository, EventDefinitionRepository, ExperimentRepository,
    FlagRepository, OrganisationRepository, ProjectRepository, RoleRepository, SdkKeyRepository,
    SegmentRepository, UserRepository, VariantRepository,
    pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgExperimentRepository, PgFlagRepository, PgOrganisationRepository, PgProjectRepository,
        PgRoleRepository, PgSdkKeyRepository, PgSegmentRepository, PgUserRepository,
        PgVariantRepository,
    },
};

/// Membership result for a single evaluation context (batch list-check responses).
#[derive(Debug, Clone)]
pub struct ContextMembership {
    /// Context type (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context key.
    pub context_key: String,
    /// Map from segment key to membership boolean.
    pub memberships: std::collections::HashMap<String, bool>,
}
