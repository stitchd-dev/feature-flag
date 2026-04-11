use std::fmt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum FlagKeyError {
    #[error("Flag key cannot be empty")]
    Empty,
    #[error("Flag key is too long (max 255 chars)")]
    TooLong,
}

macro_rules! define_id {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            Serialize,
            Deserialize,
            sqlx::Type,
        )]
        #[sqlx(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Create a new v4 UUID ID.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing UUID.
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Get the inner UUID.
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

define_id!(OrganisationId);
define_id!(ProjectId);
define_id!(EnvironmentId);
define_id!(SdkKeyId);
define_id!(UserId);
define_id!(RoleId);
define_id!(FlagId);
define_id!(VariantId);
define_id!(SegmentId);
define_id!(RuleId);
define_id!(EventId);
define_id!(ExperimentId);
define_id!(MetricId);
define_id!(AuditLogId);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct FlagKey(String);

impl FlagKey {
    pub fn new(s: impl Into<String>) -> Result<Self, FlagKeyError> {
        let s = s.into();
        if s.is_empty() {
            return Err(FlagKeyError::Empty);
        }
        if s.len() > 255 {
            return Err(FlagKeyError::TooLong);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FlagKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for FlagKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_creation() {
        let id = OrganisationId::new();
        let uuid = id.as_uuid();
        let id2 = OrganisationId::from_uuid(uuid);
        assert_eq!(id, id2);
    }

    #[test]
    fn test_flag_key_validation() {
        assert_eq!(FlagKey::new("").unwrap_err(), FlagKeyError::Empty);
        assert_eq!(
            FlagKey::new("a".repeat(256)).unwrap_err(),
            FlagKeyError::TooLong
        );
        assert!(FlagKey::new("valid-key").is_ok());
    }
}
