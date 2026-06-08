//! Database access layer: sqlx queries, migrations, and repository implementations.
//!
//! All SQL queries use compile-time checked `sqlx::query!` / `sqlx::query_as!` macros.
//! Schema changes are managed via migrations in `migrations/`.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod auth;
pub mod clickhouse;
pub mod error;
pub mod repository;
/// `ScyllaDB` client, migration applier, and list-segment repository.
pub mod scylla;

pub use scylla::segment::OrphanedGeneration;
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
pub use repository::{
    AuditLogger, ContextRegistryRepository, EnvironmentRepository, EventDefinitionRepository,
    ExperimentRepository, FlagRepository, ListContextCounts, ListSegmentSummary, MetricRepository,
    OrganisationRepository, ProjectRepository, RoleRepository, SdkKeyRepository, SegmentRepository,
    UserRepository, VariantRepository,
    composite::CompositeSegmentRepository,
    pg::{
        BanditAllocationRepository, BanditAllocationRunRow, BanditCampaignRepository,
        BanditConvergence, PgAuditLogger, PgBanditAllocationRepository, PgBanditCampaignRepository,
        PgContextRegistryRepository, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgExperimentRepository, PgFlagRepository, PgMetricRepository, PgOrganisationRepository,
        PgProjectRepository, PgRoleRepository, PgSdkKeyRepository, PgSegmentRepository,
        PgUserRepository, PgVariantRepository,
        prerequisites::{
            DEPENDENCY_KIND_PREREQUISITE, DependentRow, ENTITY_TYPE_FLAG, FlagPrerequisiteRow,
            NewFlagPrerequisite, PrerequisiteRepository,
        },
        scheduled_changes::{
            NewScheduledChange, RunOutcome, ScheduleKind, ScheduleStatus,
            ScheduledChangeRepository, ScheduledChangeRow, ScheduledChangeRunRow,
        },
    },
};
pub use stats_jobs::{
    CreateStatsJob, PgStatsJobRepository, StatsJobRepository, StatsJobRow, StatsJobStatus,
};
pub use stats_schedule::{
    ComputationStatus, PgStatsScheduleRepository, StatsScheduleRepository, StatsScheduleRow,
    UpsertStatsSchedule,
};
pub use stitchd_core::id::ExperimentIterationId;

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
