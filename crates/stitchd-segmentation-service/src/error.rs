//! Service-level error types.

use stitchd_db::RepositoryError;
use tonic::Status;

/// Errors returned by the segmentation service.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A version conflict occurred during an optimistic-concurrency update.
    #[error("version conflict: expected {expected}, actual {actual}")]
    VersionConflict {
        /// Expected version.
        expected: i64,
        /// Actual version in the database.
        actual: i64,
    },

    /// A unique constraint was violated.
    #[error("unique violation on: {field}")]
    UniqueViolation {
        /// The field that caused the violation.
        field: String,
    },

    /// Bad input from the caller.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// An internal database or serialization error.
    #[error("internal: {0}")]
    Internal(String),
}

impl From<RepositoryError> for ServiceError {
    fn from(e: RepositoryError) -> Self {
        match e {
            RepositoryError::NotFound { id } => Self::NotFound(id),
            RepositoryError::VersionConflict { expected, actual } => {
                Self::VersionConflict { expected, actual }
            }
            RepositoryError::UniqueViolation { field } => Self::UniqueViolation { field },
            other => Self::Internal(other.to_string()),
        }
    }
}

impl From<ServiceError> for Status {
    fn from(e: ServiceError) -> Self {
        match e {
            ServiceError::NotFound(msg) => Self::not_found(msg),
            ServiceError::VersionConflict { expected, actual } => {
                Self::aborted(format!("version conflict: expected {expected}, actual {actual}"))
            }
            ServiceError::UniqueViolation { field } => {
                Self::already_exists(format!("unique violation on: {field}"))
            }
            ServiceError::InvalidArgument(msg) => Self::invalid_argument(msg),
            ServiceError::Internal(msg) => Self::internal(msg),
        }
    }
}
