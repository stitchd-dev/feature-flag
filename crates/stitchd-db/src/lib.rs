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
pub mod stats_jobs;
pub mod stats_schedule;

pub use auth::{
    AuthProviderRepository, AuthUserRepository, InviteRepository, MfaChallengeRepository,
    MfaRepository, OrgMembershipRepository, OtpRepository, PgAuthProviderRepository,
    PgAuthUserRepository, PgInviteRepository, PgMfaChallengeRepository, PgMfaRepository,
    PgOrgMembershipRepository, PgOtpRepository, PgRefreshTokenRepository, RefreshTokenRepository,
    challenge_token_hash,
};
pub use error::RepositoryError;
pub use experiment_results::{
    ExperimentResultRow, ExperimentResultsRepository, PgExperimentResultsRepository,
    UpsertResultRow,
};
pub use repository::{
    AuditLogger, ContextRegistryRepository, EnvironmentRepository, EventDefinitionRepository,
    ExperimentRepository, FlagRepository, OrganisationRepository, ProjectRepository,
    RoleRepository, SdkKeyRepository, SegmentRepository, UserRepository, VariantRepository,
    pg::{
        PgAuditLogger, PgContextRegistryRepository, PgEnvironmentRepository,
        PgEventDefinitionRepository, PgExperimentRepository, PgFlagRepository,
        PgOrganisationRepository, PgProjectRepository, PgRoleRepository, PgSdkKeyRepository,
        PgSegmentRepository, PgUserRepository, PgVariantRepository,
    },
};
pub use stats_jobs::{
    CreateStatsJob, PgStatsJobRepository, StatsJobRepository, StatsJobRow, StatsJobStatus,
};
pub use stats_schedule::{
    ComputationStatus, PgStatsScheduleRepository, StatsScheduleRepository, StatsScheduleRow,
    UpsertStatsSchedule,
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

/// Membership result keyed by `SegmentId` (UUID) rather than segment key.
/// Used by the SDK-facing batch RPC, which references segments by their
/// stable UUIDs from the definition snapshot.
#[derive(Debug, Clone)]
pub struct SegmentIdMembership {
    /// Context type (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context key.
    pub context_key: String,
    /// Map from segment id (UUID) to membership boolean.
    pub memberships: std::collections::HashMap<stitchd_core::id::SegmentId, bool>,
}
