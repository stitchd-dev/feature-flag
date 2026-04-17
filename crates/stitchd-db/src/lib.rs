//! Database access layer: sqlx queries, migrations, and repository implementations.
//!
//! All SQL queries use compile-time checked `sqlx::query!` / `sqlx::query_as!` macros.
//! Schema changes are managed via migrations in `migrations/`.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod error;
pub mod repository;

pub use error::RepositoryError;
pub use repository::{
    AuditLogger, EnvironmentRepository, FlagRepository, OrganisationRepository, ProjectRepository,
    RoleRepository, SdkKeyRepository, SegmentRepository, UserRepository, VariantRepository,
    pg::{
        PgAuditLogger, PgEnvironmentRepository, PgFlagRepository, PgOrganisationRepository,
        PgProjectRepository, PgRoleRepository, PgSdkKeyRepository, PgSegmentRepository,
        PgUserRepository, PgVariantRepository,
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
