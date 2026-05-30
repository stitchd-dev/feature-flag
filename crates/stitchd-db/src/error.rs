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

    /// A foreign key constraint was violated (referenced entity does not exist).
    #[error("foreign key violation on constraint: {constraint}")]
    ForeignKeyViolation {
        /// Name of the FK constraint that was violated.
        constraint: String,
    },

    /// The operation is not permitted given the entity's current state.
    #[error("invalid state: {reason}")]
    InvalidState {
        /// Human-readable explanation.
        reason: String,
    },

    /// An underlying sqlx / Postgres error.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Any other unexpected error.
    #[error("unexpected error: {0}")]
    Unexpected(#[from] anyhow::Error),
}

#[cfg(feature = "tonic")]
impl From<RepositoryError> for tonic::Status {
    fn from(e: RepositoryError) -> Self {
        match e {
            RepositoryError::NotFound { id } => Self::not_found(format!("not found: {id}")),
            RepositoryError::VersionConflict { expected, actual } => Self::aborted(format!(
                "version conflict: expected {expected}, actual {actual}"
            )),
            RepositoryError::UniqueViolation { field } => {
                Self::already_exists(format!("unique violation on: {field}"))
            }
            RepositoryError::ForeignKeyViolation { constraint } => {
                Self::invalid_argument(format!("referenced entity does not exist: {constraint}"))
            }
            RepositoryError::InvalidState { reason } => Self::failed_precondition(reason),
            RepositoryError::Database(_) | RepositoryError::Unexpected(_) => {
                Self::internal("internal error")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_display() {
        let err = RepositoryError::NotFound {
            id: "abc-123".to_string(),
        };
        assert_eq!(err.to_string(), "not found: abc-123");
    }

    #[test]
    fn version_conflict_display() {
        let err = RepositoryError::VersionConflict {
            expected: 1,
            actual: 3,
        };
        assert_eq!(err.to_string(), "version conflict: expected 1, actual 3");
    }

    #[test]
    fn unique_violation_display() {
        let err = RepositoryError::UniqueViolation {
            field: "email".to_string(),
        };
        assert_eq!(err.to_string(), "unique violation on field: email");
    }

    #[test]
    fn unexpected_from_anyhow() {
        let anyhow_err = anyhow::anyhow!("something went wrong");
        let err = RepositoryError::from(anyhow_err);
        assert!(err.to_string().contains("something went wrong"));
    }

    #[test]
    fn debug_format_not_found() {
        let err = RepositoryError::NotFound {
            id: "test-id".to_string(),
        };
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("NotFound"));
        assert!(debug_str.contains("test-id"));
    }

    #[test]
    fn debug_format_version_conflict() {
        let err = RepositoryError::VersionConflict {
            expected: 5,
            actual: 7,
        };
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("VersionConflict"));
    }
}
