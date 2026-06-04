//! Flag prerequisite gate types.
//!
//! A [`PrerequisiteGate`] is the eval-time dependency check applied to a flag
//! *before* its own rules run: every listed [`FlagPrerequisite`] must resolve
//! to its `required_variant_id`, otherwise the dependent flag returns the
//! configured `fallback_variant_id` (or its off/disabled variant when none is
//! set) and skips its rules.
//!
//! These are pure data types. The gate is *applied* by the evaluation
//! orchestrator (Phase 2); this module only defines the shapes that ride the
//! flag-definition snapshot and the admin/preview surfaces.

use crate::id::{FlagId, VariantId};
use serde::{Deserialize, Serialize};

/// A single flag→flag prerequisite edge: the dependent flag requires
/// `prerequisite_flag_id` to resolve to `required_variant_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlagPrerequisite {
    /// The flag this gate depends on.
    pub prerequisite_flag_id: FlagId,
    /// The variant the prerequisite flag must resolve to for the gate to pass.
    pub required_variant_id: VariantId,
}

/// The full prerequisite gate for a flag: the set of prerequisite edges plus
/// the fallback variant returned when any edge fails.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PrerequisiteGate {
    /// Prerequisite edges, evaluated in order. Empty = no gate (flag always
    /// proceeds to its own rules).
    pub prerequisites: Vec<FlagPrerequisite>,
    /// Variant returned when any prerequisite fails. `None` means "fall back to
    /// the flag's off/disabled variant".
    pub fallback_variant_id: Option<VariantId>,
}

impl PrerequisiteGate {
    /// True when no prerequisites are configured (the gate is a no-op).
    pub fn is_empty(&self) -> bool {
        self.prerequisites.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn gate_round_trips_through_serde() {
        let pre_flag = FlagId::from_uuid(Uuid::new_v4());
        let req_variant = VariantId::from_uuid(Uuid::new_v4());
        let fallback = VariantId::from_uuid(Uuid::new_v4());

        let gate = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: pre_flag,
                required_variant_id: req_variant,
            }],
            fallback_variant_id: Some(fallback),
        };

        let json = serde_json::to_string(&gate).expect("serialize");
        let back: PrerequisiteGate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(gate, back);
        assert_eq!(back.prerequisites.len(), 1);
        assert_eq!(back.prerequisites[0].prerequisite_flag_id, pre_flag);
        assert_eq!(back.prerequisites[0].required_variant_id, req_variant);
        assert_eq!(back.fallback_variant_id, Some(fallback));
    }

    #[test]
    fn empty_gate_round_trips_and_reports_empty() {
        let gate = PrerequisiteGate::default();
        assert!(gate.is_empty());
        let json = serde_json::to_string(&gate).expect("serialize");
        let back: PrerequisiteGate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(gate, back);
        assert!(back.is_empty());
        assert!(back.fallback_variant_id.is_none());
    }

    #[test]
    fn gate_without_fallback_uses_none() {
        let gate = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: FlagId::from_uuid(Uuid::new_v4()),
                required_variant_id: VariantId::from_uuid(Uuid::new_v4()),
            }],
            fallback_variant_id: None,
        };
        let json = serde_json::to_string(&gate).expect("serialize");
        let back: PrerequisiteGate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.fallback_variant_id, None);
        assert!(!back.is_empty());
    }
}
