//! Service-level error types.

use stitchd_db::RepositoryError;
use tonic::Status;

/// Errors returned by the segmentation service.
#[derive(Debug, thiserror::Error)]
pub enum SegmentationServiceError {
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

impl From<RepositoryError> for SegmentationServiceError {
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

impl From<SegmentationServiceError> for Status {
    fn from(e: SegmentationServiceError) -> Self {
        match e {
            SegmentationServiceError::NotFound(msg) => Self::not_found(msg),
            SegmentationServiceError::VersionConflict { expected, actual } => Self::aborted(format!(
                "version conflict: expected {expected}, actual {actual}"
            )),
            SegmentationServiceError::UniqueViolation { field } => {
                Self::already_exists(format!("unique violation on: {field}"))
            }
            SegmentationServiceError::InvalidArgument(msg) => Self::invalid_argument(msg),
            SegmentationServiceError::Internal(msg) => Self::internal(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_not_found_maps_to_service_not_found() {
        let repo_err = RepositoryError::NotFound {
            id: "segment-abc".to_string(),
        };
        let svc_err = SegmentationServiceError::from(repo_err);
        assert!(matches!(svc_err, SegmentationServiceError::NotFound(_)));
        assert!(svc_err.to_string().contains("segment-abc"));
    }

    #[test]
    fn repository_version_conflict_maps_to_service_version_conflict() {
        let repo_err = RepositoryError::VersionConflict {
            expected: 1,
            actual: 3,
        };
        let svc_err = SegmentationServiceError::from(repo_err);
        assert!(matches!(svc_err, SegmentationServiceError::VersionConflict { .. }));
    }

    #[test]
    fn repository_unique_violation_maps_to_service_unique_violation() {
        let repo_err = RepositoryError::UniqueViolation {
            field: "key".to_string(),
        };
        let svc_err = SegmentationServiceError::from(repo_err);
        assert!(matches!(svc_err, SegmentationServiceError::UniqueViolation { .. }));
    }

    #[test]
    fn repository_other_error_maps_to_service_internal() {
        let repo_err = RepositoryError::InvalidState {
            reason: "experiment is running".to_string(),
        };
        let svc_err = SegmentationServiceError::from(repo_err);
        assert!(matches!(svc_err, SegmentationServiceError::Internal(_)));
    }

    #[test]
    fn service_error_display_messages() {
        assert!(
            SegmentationServiceError::NotFound("x".to_string())
                .to_string()
                .contains('x')
        );
        assert!(
            SegmentationServiceError::InvalidArgument("bad".to_string())
                .to_string()
                .contains("bad")
        );
        assert!(
            SegmentationServiceError::Internal("oops".to_string())
                .to_string()
                .contains("oops")
        );
        assert!(
            SegmentationServiceError::UniqueViolation {
                field: "key".to_string()
            }
            .to_string()
            .contains("key")
        );
        let vc = SegmentationServiceError::VersionConflict {
            expected: 1,
            actual: 2,
        };
        assert!(vc.to_string().contains('1'));
    }
}
