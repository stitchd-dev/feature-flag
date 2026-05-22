//! Error types for the flag service.

use stitchd_core::id::ExperimentId;
use thiserror::Error;
use tonic::Status;

/// Sentinel prefix used to encode the locking experiment ID into the
/// `tonic::Status::failed_precondition` message so the gateway's
/// `IntoResponse` mapper can rebuild the structured 409 error body
/// (`{ "error": "flag_locked_by_experiment", "experiment_id": "<uuid>" }`).
///
/// Format: `"flag_locked_by_experiment:<uuid>"`.
pub const FLAG_LOCKED_STATUS_PREFIX: &str = "flag_locked_by_experiment:";

/// Errors that can occur in the flag service.
#[derive(Debug, Error)]
pub enum FlagServiceError {
    /// The requested flag was not found.
    #[error("flag not found: {0}")]
    NotFound(String),

    /// A version conflict was detected (optimistic locking failure).
    #[error("version conflict: expected {expected}, actual {actual}")]
    VersionConflict { expected: u64, actual: u64 },

    /// A unique constraint was violated (e.g., duplicate flag key).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Invalid input was provided.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The targeted flag is locked because an experiment in
    /// `running`/`paused` status is bound to it. Every admin mutation on
    /// such a flag must be rejected until the experiment is stopped.
    ///
    /// Encoded on the wire as `tonic::Status::failed_precondition`
    /// with message `"flag_locked_by_experiment:<experiment_id>"`.
    #[error("flag is locked by experiment {experiment_id}")]
    FlagLocked { experiment_id: ExperimentId },

    /// An internal/database error occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<stitchd_db::RepositoryError> for FlagServiceError {
    fn from(e: stitchd_db::RepositoryError) -> Self {
        match e {
            stitchd_db::RepositoryError::NotFound { id } => Self::NotFound(id),
            stitchd_db::RepositoryError::VersionConflict { expected, actual } =>
            {
                #[allow(clippy::cast_sign_loss)]
                Self::VersionConflict {
                    expected: expected as u64,
                    actual: actual as u64,
                }
            }
            stitchd_db::RepositoryError::UniqueViolation { field } => {
                Self::Conflict(format!("unique violation on field: {field}"))
            }
            stitchd_db::RepositoryError::ForeignKeyViolation { constraint } => {
                Self::InvalidArgument(format!("referenced entity does not exist: {constraint}"))
            }
            other => Self::Internal(other.to_string()),
        }
    }
}

impl From<FlagServiceError> for Status {
    fn from(e: FlagServiceError) -> Self {
        match e {
            FlagServiceError::NotFound(msg) => Self::not_found(msg),
            FlagServiceError::VersionConflict { expected, actual } => Self::aborted(format!(
                "version conflict: expected {expected}, actual {actual}"
            )),
            FlagServiceError::Conflict(msg) => Self::already_exists(msg),
            FlagServiceError::InvalidArgument(msg) => Self::invalid_argument(msg),
            FlagServiceError::FlagLocked { experiment_id } => {
                Self::failed_precondition(format!("{FLAG_LOCKED_STATUS_PREFIX}{experiment_id}"))
            }
            FlagServiceError::Internal(msg) => Self::internal(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_error_maps_to_not_found_status() {
        let err = FlagServiceError::NotFound("flag-123".to_string());
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    #[test]
    fn version_conflict_maps_to_aborted_status() {
        let err = FlagServiceError::VersionConflict {
            expected: 1,
            actual: 5,
        };
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::Aborted);
    }

    #[test]
    fn conflict_maps_to_already_exists_status() {
        let err = FlagServiceError::Conflict("key exists".to_string());
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::AlreadyExists);
    }

    #[test]
    fn invalid_argument_maps_to_invalid_argument_status() {
        let err = FlagServiceError::InvalidArgument("bad key".to_string());
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn internal_maps_to_internal_status() {
        let err = FlagServiceError::Internal("db error".to_string());
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::Internal);
    }

    #[test]
    fn repository_not_found_converts_to_not_found() {
        let repo_err = stitchd_db::RepositoryError::NotFound {
            id: "abc".to_string(),
        };
        let err: FlagServiceError = repo_err.into();
        assert!(matches!(err, FlagServiceError::NotFound(_)));
    }

    #[test]
    fn repository_version_conflict_converts_correctly() {
        let repo_err = stitchd_db::RepositoryError::VersionConflict {
            expected: 2,
            actual: 3,
        };
        let err: FlagServiceError = repo_err.into();
        assert!(matches!(
            err,
            FlagServiceError::VersionConflict {
                expected: 2,
                actual: 3
            }
        ));
    }

    #[test]
    fn repository_unique_violation_converts_to_conflict() {
        let repo_err = stitchd_db::RepositoryError::UniqueViolation {
            field: "key".to_string(),
        };
        let err: FlagServiceError = repo_err.into();
        assert!(matches!(err, FlagServiceError::Conflict(_)));
    }

    #[test]
    fn flag_locked_maps_to_failed_precondition_with_id_in_message() {
        let exp_id = ExperimentId::new();
        let err = FlagServiceError::FlagLocked {
            experiment_id: exp_id,
        };
        let status: Status = err.into();
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert!(
            status.message().starts_with(FLAG_LOCKED_STATUS_PREFIX),
            "status message missing sentinel prefix: {}",
            status.message()
        );
        assert!(
            status.message().contains(&exp_id.to_string()),
            "status message missing experiment id: {}",
            status.message()
        );
    }
}
