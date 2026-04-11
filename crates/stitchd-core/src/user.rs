//! User identity and RBAC types.
//!
//! Models users, project-scoped roles, and fine-grained permissions.
//! `Permission::matches` supports wildcard patterns (`*`, `payments-*`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{OrganisationId, ProjectId, RoleId, UserId};

/// The category of resource a permission applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    /// A deployment environment.
    Environment,
    /// A feature flag.
    Flag,
    /// A segment.
    Segment,
}

/// The operation a permission allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Read-only access.
    Read,
    /// Create / update access.
    Write,
    /// Publish (enable/disable) access.
    Publish,
    /// Full administrative access.
    Admin,
}

/// A single permission entry granting an `action` on resources matching
/// `resource_pattern` of type `resource_type`.
///
/// Pattern semantics:
/// - `"*"` — matches every resource name.
/// - `"prefix-*"` — matches any resource name that starts with `"prefix-"`.
/// - Any other string — exact match only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Permission {
    /// The category of resource.
    pub resource_type: ResourceType,
    /// Glob-style pattern matched against the resource's name/key.
    pub resource_pattern: String,
    /// The operation this permission grants.
    pub action: Action,
}

impl Permission {
    /// Returns `true` if this permission covers `(resource_type, resource_name)`.
    ///
    /// # Pattern rules
    ///
    /// | Pattern     | Matches                                  |
    /// |-------------|------------------------------------------|
    /// | `"*"`       | every name                               |
    /// | `"foo-*"`   | any name starting with `"foo-"`          |
    /// | `"exact"`   | only the name `"exact"`                  |
    ///
    /// # Examples
    ///
    /// ```
    /// use stitchd_core::user::{Action, Permission, ResourceType};
    ///
    /// let perm = Permission {
    ///     resource_type: ResourceType::Flag,
    ///     resource_pattern: "payments-*".to_string(),
    ///     action: Action::Read,
    /// };
    ///
    /// assert!(perm.matches(&ResourceType::Flag, "payments-checkout"));
    /// assert!(!perm.matches(&ResourceType::Flag, "auth-login"));
    /// assert!(!perm.matches(&ResourceType::Segment, "payments-checkout"));
    /// ```
    #[must_use]
    pub fn matches(&self, resource_type: &ResourceType, resource_name: &str) -> bool {
        if self.resource_type != *resource_type {
            return false;
        }
        self.pattern_matches(resource_name)
    }

    fn pattern_matches(&self, name: &str) -> bool {
        let pattern = &self.resource_pattern;
        if pattern == "*" {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return name.starts_with(prefix);
        }
        pattern == name
    }
}

/// A platform user, scoped to an [`Organisation`](crate::tenant::Organisation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    /// Unique identifier.
    pub id: UserId,
    /// Email address — unique within the organisation.
    pub email: String,
    /// Parent organisation.
    pub organisation_id: OrganisationId,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
}

/// A project-scoped role bundling a set of permissions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Role {
    /// Unique identifier.
    pub id: RoleId,
    /// The project this role belongs to.
    pub project_id: ProjectId,
    /// Display name (e.g. `"developer"`, `"viewer"`).
    pub name: String,
    /// Permissions granted to holders of this role.
    pub permissions: Vec<Permission>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag_perm(pattern: &str) -> Permission {
        Permission {
            resource_type: ResourceType::Flag,
            resource_pattern: pattern.to_string(),
            action: Action::Read,
        }
    }

    // --- Wildcard `*` ---

    #[test]
    fn wildcard_matches_any_name() {
        let perm = flag_perm("*");
        assert!(perm.matches(&ResourceType::Flag, "anything"));
        assert!(perm.matches(&ResourceType::Flag, "payments-checkout"));
        assert!(perm.matches(&ResourceType::Flag, ""));
    }

    #[test]
    fn wildcard_does_not_match_wrong_resource_type() {
        let perm = flag_perm("*");
        assert!(!perm.matches(&ResourceType::Segment, "anything"));
        assert!(!perm.matches(&ResourceType::Environment, "anything"));
    }

    // --- Prefix wildcard `prefix-*` ---

    #[test]
    fn prefix_wildcard_matches_names_starting_with_prefix() {
        let perm = flag_perm("payments-*");
        assert!(perm.matches(&ResourceType::Flag, "payments-checkout"));
        assert!(perm.matches(&ResourceType::Flag, "payments-refund"));
        assert!(perm.matches(&ResourceType::Flag, "payments-")); // prefix only, still matches
    }

    #[test]
    fn prefix_wildcard_does_not_match_other_prefixes() {
        let perm = flag_perm("payments-*");
        assert!(!perm.matches(&ResourceType::Flag, "auth-login"));
        assert!(!perm.matches(&ResourceType::Flag, "payment-checkout")); // no 's'
    }

    #[test]
    fn prefix_wildcard_does_not_match_wrong_resource_type() {
        let perm = flag_perm("payments-*");
        assert!(!perm.matches(&ResourceType::Segment, "payments-checkout"));
    }

    // --- Exact match ---

    #[test]
    fn exact_pattern_matches_only_that_name() {
        let perm = flag_perm("dark-mode");
        assert!(perm.matches(&ResourceType::Flag, "dark-mode"));
    }

    #[test]
    fn exact_pattern_does_not_match_similar_names() {
        let perm = flag_perm("dark-mode");
        assert!(!perm.matches(&ResourceType::Flag, "dark-mode-v2"));
        assert!(!perm.matches(&ResourceType::Flag, "dark"));
        assert!(!perm.matches(&ResourceType::Flag, ""));
    }

    #[test]
    fn exact_pattern_does_not_match_wrong_resource_type() {
        let perm = flag_perm("dark-mode");
        assert!(!perm.matches(&ResourceType::Environment, "dark-mode"));
    }

    // --- Role construction ---

    #[test]
    fn role_with_multiple_permissions_serializes() {
        let role = Role {
            id: RoleId::new(),
            project_id: ProjectId::new(),
            name: "developer".to_string(),
            permissions: vec![
                Permission {
                    resource_type: ResourceType::Flag,
                    resource_pattern: "*".to_string(),
                    action: Action::Read,
                },
                Permission {
                    resource_type: ResourceType::Segment,
                    resource_pattern: "prod-*".to_string(),
                    action: Action::Write,
                },
            ],
        };
        let json = serde_json::to_string(&role).unwrap();
        let back: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "developer");
        assert_eq!(back.permissions.len(), 2);
    }
}
