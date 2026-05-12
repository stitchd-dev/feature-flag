//! Flag metadata and configuration.
//!
//! A flag declares a [`FlagValueType`]; every [`Variant`] belonging to that flag
//! must carry a [`VariantValue`] whose discriminant matches the flag's type.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{FlagId, FlagKey, ProjectId, VariantId};
use crate::rule_engine::types::Rule;
pub use crate::variants::{FlagValueType, Variant, VariantValue};

/// A feature flag definition stored in the database.
///
/// Variants belonging to this flag are fetched separately via `VariantRepository`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlagRecord {
    /// Unique identifier.
    pub id: FlagId,
    /// The project this flag belongs to.
    pub project_id: ProjectId,
    /// URL-safe string key (unique within the project).
    pub key: FlagKey,
    /// Human-readable display name.
    pub name: String,
    /// Optional description visible in the admin UI.
    pub description: String,
    /// The type every variant value must match.
    pub value_type: FlagValueType,
    /// Whether evaluation is active (`true`) or the flag always returns its default.
    pub enabled: bool,
    /// The variant to return if no rules match.
    pub default_variant_id: Option<VariantId>,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
    /// Set when the flag is soft-deleted; `None` while active.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// Configures which context parameters are used for hashing a specific flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlagHashingConfig {
    /// The flag this config belongs to.
    pub flag_id: FlagId,
    /// The parameter key to hash.
    pub parameter_key: String,
    /// The context type (e.g. 'user', 'session').
    pub parameter_type: String,
    /// The order in which parameters are combined for hashing.
    pub order: i32,
}

/// A rule associated with a feature flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlagRule {
    /// The flag this rule belongs to.
    pub flag_id: FlagId,
    /// Evaluation priority (lower is higher priority).
    pub rule_index: i32,
    /// The rule definition (condition and output).
    pub rule: Rule,
}

/// A complete feature flag aggregate, including its rules, hashing config, and variants.
///
/// This is the primary domain model used for evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Flag {
    /// Core flag metadata.
    pub record: FlagRecord,
    /// Context parameters used for consistent hashing.
    pub hashing_config: Vec<FlagHashingConfig>,
    /// Ordered evaluation rules.
    pub rules: Vec<FlagRule>,
    /// All possible variants for this flag.
    pub variants: Vec<Variant>,
}

impl Flag {
    /// Returns the variant with the given ID, if it belongs to this flag.
    pub fn get_variant(&self, id: VariantId) -> Option<&Variant> {
        self.variants.iter().find(|v| v.id == id)
    }

    /// Returns the default variant for this flag, if configured and exists.
    pub fn get_default_variant(&self) -> Option<&Variant> {
        self.record
            .default_variant_id
            .and_then(|id| self.get_variant(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_get_variant_works() {
        let vid = VariantId::new();
        let flag = Flag {
            record: FlagRecord {
                id: FlagId::new(),
                project_id: ProjectId::new(),
                key: FlagKey::new("test").unwrap(),
                value_type: FlagValueType::Bool,
                enabled: true,
                default_variant_id: Some(vid),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                deleted_at: None,
                version: 1,
            },
            hashing_config: vec![],
            rules: vec![],
            variants: vec![Variant {
                id: vid,
                key: "default".to_string(),
                value: VariantValue::BoolValue(true),
            }],
        };

        assert!(flag.get_variant(vid).is_some());
        assert_eq!(flag.get_default_variant().unwrap().id, vid);
    }
}
