//! Repository error type shared across all repository implementations.

use thiserror::Error;

/// Errors that can arise from repository operations.
#[derive(Debug, Error)]
pub enum RepositoryError {
    /// The requested entity does not exist (or has been soft-deleted).
    #[error("not found: {id}")]
    NotFound {
        /// String representation of the missing entity's ID.
        id: String,
    },

    /// An optimistic-concurrency conflict: the entity was modified by another
    /// writer since it was last read.
    #[error("version conflict: expected {expected}, actual {actual}")]
    VersionConflict {
        /// The version the caller expected.
        expected: i64,
        /// The version currently stored in the database.
        actual: i64,
    },

    /// A uniqueness constraint was violated.
    #[error("unique violation on field: {field}")]
    UniqueViolation {
        /// Name of the field or constraint that was violated.
        field: String,
    },

    /// An underlying sqlx / Postgres error.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Any other unexpected error.
    #[error("unexpected error: {0}")]
    Unexpected(#[from] anyhow::Error),
}
