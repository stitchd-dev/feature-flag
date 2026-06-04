//! Flag-prerequisite write-time validation + cycle detection
//! (`flag_lifecycle_20260604`, Phase 4 Task 2 / Task 3).
//!
//! This module hosts the pure helpers the `FlagService` RPC handlers use to
//! validate a proposed prerequisite set before persisting it:
//!
//! - [`detect_prerequisite_cycle`] reuses the core dependency graph + Kahn's
//!   topological sort ([`stitchd_core::rule_engine`]) to reject any edge set
//!   that would form a cycle in the flag→flag prerequisite DAG, returning the
//!   offending flag path.
//! - [`DEPENDENCY_EXISTS_STATUS_PREFIX`] is the sentinel prefix stamped onto a
//!   `tonic::Status::failed_precondition` message when a flag delete/archive is
//!   blocked because another flag still references it as a prerequisite — it
//!   mirrors `error::FLAG_LOCKED_STATUS_PREFIX` exactly (the gateway decodes it
//!   into a structured `409 DEPENDENCY_EXISTS` body; see
//!   `stitchd-gateway/src/error.rs`).

use std::collections::HashMap;

use stitchd_core::id::{FlagId, VariantId};
use stitchd_core::prerequisite::FlagPrerequisite as CoreFlagPrerequisite;
use stitchd_core::rule_engine::dependency::topological_sort;
use stitchd_core::rule_engine::error::RuleEngineError;

/// Sentinel prefix used to encode the blocking dependents into a
/// `tonic::Status::failed_precondition` message so the gateway can rebuild the
/// structured `409 DEPENDENCY_EXISTS` body. Mirrors the
/// `flag_locked_by_experiment:<uuid>` convention from
/// [`crate::error::FLAG_LOCKED_STATUS_PREFIX`].
///
/// Format: `"dependency_exists:<comma-separated dependent flag ids>"`.
pub const DEPENDENCY_EXISTS_STATUS_PREFIX: &str = "dependency_exists:";

/// The result of a write-time prerequisite cycle check.
///
/// On rejection the `involved` flag IDs (the cycle path) are surfaced to the
/// caller so the RPC can name the offending flags in its `Status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrerequisiteCycle {
    /// Flags that participate in the detected cycle.
    pub involved: Vec<FlagId>,
}

/// Detect whether the **proposed** prerequisite edge set forms a cycle in the
/// flag→flag prerequisite DAG.
///
/// `edges` is the full, post-write picture: one `(flag, prerequisite_flags)`
/// entry per flag whose prerequisite set is known (the flag being edited plus
/// every other flag that currently has prerequisites). The graph is fed to the
/// same Kahn's-algorithm topological sort the evaluation orchestrator uses, so
/// a prerequisite cycle is detected identically to an eval-time
/// `FlagEvaluatedAs` cycle.
///
/// Returns `Ok(())` when the edge set is a DAG, or
/// `Err(PrerequisiteCycle { involved })` naming the flags in the cycle.
///
/// # Errors
/// Returns [`PrerequisiteCycle`] when the edges form a cycle.
pub fn detect_prerequisite_cycle(edges: &[(FlagId, Vec<FlagId>)]) -> Result<(), PrerequisiteCycle> {
    // Build the adjacency map directly: flag → set of prerequisite flags.
    // (We don't need the rule-edge merge here — write-time validation concerns
    // only the prerequisite graph — but we mirror `build_dependency_graph`'s
    // shape so the same `topological_sort` consumes it.)
    let mut graph: HashMap<FlagId, std::collections::HashSet<FlagId>> = HashMap::new();
    for (flag, prereqs) in edges {
        graph
            .entry(*flag)
            .or_default()
            .extend(prereqs.iter().copied());
        // Ensure prerequisite-only flags appear as nodes too.
        for p in prereqs {
            graph.entry(*p).or_default();
        }
    }

    match topological_sort(&graph) {
        Ok(_) => Ok(()),
        Err(RuleEngineError::CyclicFlagDependency { involved }) => {
            Err(PrerequisiteCycle { involved })
        }
        // topological_sort only ever returns CyclicFlagDependency; treat any
        // other (impossible) error as a conservative cycle rejection.
        Err(_) => Err(PrerequisiteCycle {
            involved: edges.iter().map(|(f, _)| *f).collect(),
        }),
    }
}

/// Convenience: build a core [`CoreFlagPrerequisite`] list from `(flag, variant)`
/// id pairs. Used by the snapshot/preview population paths.
#[must_use]
pub fn core_prerequisites(pairs: &[(FlagId, VariantId)]) -> Vec<CoreFlagPrerequisite> {
    pairs
        .iter()
        .map(|(flag, variant)| CoreFlagPrerequisite {
            prerequisite_flag_id: *flag,
            required_variant_id: *variant,
        })
        .collect()
}

/// Format the `dependency_exists:` sentinel message from the blocking dependent
/// flag IDs.
#[must_use]
pub fn dependency_exists_message(dependents: &[FlagId]) -> String {
    let ids = dependents
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("{DEPENDENCY_EXISTS_STATUS_PREFIX}{ids}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acyclic_chain_is_ok() {
        // a → b → c (a depends on b, b depends on c). No cycle.
        let a = FlagId::new();
        let b = FlagId::new();
        let c = FlagId::new();
        let edges = vec![(a, vec![b]), (b, vec![c]), (c, vec![])];
        assert!(detect_prerequisite_cycle(&edges).is_ok());
    }

    #[test]
    fn direct_two_node_cycle_is_rejected() {
        // a → b and b → a.
        let a = FlagId::new();
        let b = FlagId::new();
        let edges = vec![(a, vec![b]), (b, vec![a])];
        let err = detect_prerequisite_cycle(&edges).unwrap_err();
        assert!(err.involved.contains(&a) || err.involved.contains(&b));
    }

    #[test]
    fn self_cycle_is_rejected() {
        let a = FlagId::new();
        let edges = vec![(a, vec![a])];
        let err = detect_prerequisite_cycle(&edges).unwrap_err();
        assert!(err.involved.contains(&a));
    }

    #[test]
    fn transitive_cycle_is_rejected() {
        // a → b → c → a.
        let a = FlagId::new();
        let b = FlagId::new();
        let c = FlagId::new();
        let edges = vec![(a, vec![b]), (b, vec![c]), (c, vec![a])];
        let err = detect_prerequisite_cycle(&edges).unwrap_err();
        assert!(!err.involved.is_empty());
    }

    #[test]
    fn empty_edge_set_is_ok() {
        assert!(detect_prerequisite_cycle(&[]).is_ok());
    }

    #[test]
    fn dependency_exists_message_joins_ids() {
        let a = FlagId::new();
        let b = FlagId::new();
        let msg = dependency_exists_message(&[a, b]);
        assert!(msg.starts_with(DEPENDENCY_EXISTS_STATUS_PREFIX));
        assert!(msg.contains(&a.to_string()));
        assert!(msg.contains(&b.to_string()));
        assert!(msg.contains(','));
    }

    #[test]
    fn core_prerequisites_maps_pairs() {
        let f = FlagId::new();
        let v = VariantId::new();
        let out = core_prerequisites(&[(f, v)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].prerequisite_flag_id, f);
        assert_eq!(out[0].required_variant_id, v);
    }
}
