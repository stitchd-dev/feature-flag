//! Candidate-pair enumeration for cross-experiment interaction detection.
//!
//! Before the stats service runs the (expensive) interaction-detection query
//! (`queries::interaction`) for an environment, it must decide *which* pairs of
//! experiments are even worth testing. Testing every unordered pair is O(n²)
//! and mostly wasted: two experiments can only meaningfully interact when they
//!
//!   1. live on **distinct flags** (the same flag's experiments are mutually
//!      exclusive by construction — a context is in at most one),
//!   2. were **active at the same time** (their `[started_at, ended_at)`
//!      windows overlap; non-overlapping experiments never share a live
//!      context),
//!   3. share **at least one metric** (no shared metric ⇒ nothing to
//!      cross-tabulate an effect on), and
//!   4. are **not in the same exclusion group** (an exclusion group already
//!      guarantees a context is bucketed into at most one of its members, so
//!      they can never co-occur — an interaction is structurally impossible).
//!
//! [`candidate_pairs`] applies all four filters to a slice of
//! [`ExperimentMeta`] and returns the surviving unordered pairs. It is a pure
//! function over a DB-free input struct so it is exhaustively unit-testable
//! without ClickHouse or PostgreSQL.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Experiment identifier.
pub type ExpId = Uuid;

/// The minimal metadata [`candidate_pairs`] needs about one experiment to
/// decide whether it can interact with another.
///
/// This is a local, DB-free input shape: the scheduler (Phase 6) is
/// responsible for projecting the gRPC `RunningExperiment` / iteration rows
/// into `ExperimentMeta` before calling [`candidate_pairs`]. Keeping it local
/// keeps this module a pure, testable unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentMeta {
    /// Experiment UUID.
    pub id: ExpId,
    /// Feature-flag UUID the experiment drives. Two experiments on the SAME
    /// flag are mutually exclusive (a context is bucketed by at most one of
    /// them) and so can never interact.
    pub flag_id: Uuid,
    /// When the experiment's active window opened.
    pub started_at: DateTime<Utc>,
    /// When the active window closed; `None` means still running (open-ended
    /// upper bound).
    pub ended_at: Option<DateTime<Utc>>,
    /// Metric definition UUIDs attached to the experiment. The interaction is
    /// only computed over metrics the two experiments share.
    pub metric_ids: Vec<Uuid>,
    /// Exclusion-group UUID, if the experiment belongs to one. Members of the
    /// same group are mutually exclusive at assignment time, so a same-group
    /// pair can never share a context.
    pub exclusion_group_id: Option<Uuid>,
}

/// Enumerate the unordered experiment pairs worth testing for interaction.
///
/// A pair `(a, b)` survives iff ALL of the following hold:
///
/// - `a.flag_id != b.flag_id` — same-flag experiments are mutually exclusive.
/// - their active windows overlap — see [`windows_overlap`].
/// - `a.metric_ids ∩ b.metric_ids` is non-empty.
/// - they are not both in the same exclusion group (`Some(g) == Some(g)`).
///   Two experiments each in a *different* group, or with `None` group(s), are
///   not excluded by this rule.
///
/// Each surviving pair is returned exactly once, ordered so the first element's
/// `id` sorts before the second's — this gives a stable, dedupe-friendly
/// `(min, max)` ordering regardless of input order.
#[must_use]
pub fn candidate_pairs(experiments: &[ExperimentMeta]) -> Vec<(ExpId, ExpId)> {
    let mut pairs = Vec::new();
    for (i, a) in experiments.iter().enumerate() {
        for b in &experiments[i + 1..] {
            if can_interact(a, b) {
                let (lo, hi) = if a.id <= b.id {
                    (a.id, b.id)
                } else {
                    (b.id, a.id)
                };
                pairs.push((lo, hi));
            }
        }
    }
    pairs
}

/// Whether two experiments satisfy every interaction-candidacy rule.
fn can_interact(a: &ExperimentMeta, b: &ExperimentMeta) -> bool {
    a.flag_id != b.flag_id
        && windows_overlap(a, b)
        && shares_metric(a, b)
        && !same_exclusion_group(a, b)
}

/// Half-open active-window overlap: `[a.start, a.end) ∩ [b.start, b.end) != ∅`.
///
/// `ended_at == None` (still running) is treated as an open-ended upper bound,
/// so a running experiment overlaps anything that started before "now or
/// later". Two windows overlap iff each one started strictly before the other
/// ended.
fn windows_overlap(a: &ExperimentMeta, b: &ExperimentMeta) -> bool {
    let a_before_b_ends = b.ended_at.is_none_or(|b_end| a.started_at < b_end);
    let b_before_a_ends = a.ended_at.is_none_or(|a_end| b.started_at < a_end);
    a_before_b_ends && b_before_a_ends
}

/// Whether the two experiments' metric-id sets intersect.
fn shares_metric(a: &ExperimentMeta, b: &ExperimentMeta) -> bool {
    a.metric_ids.iter().any(|m| b.metric_ids.contains(m))
}

/// Whether both experiments belong to the *same* exclusion group.
fn same_exclusion_group(a: &ExperimentMeta, b: &ExperimentMeta) -> bool {
    match (a.exclusion_group_id, b.exclusion_group_id) {
        (Some(g_a), Some(g_b)) => g_a == g_b,
        _ => false,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 0, 0, 0).unwrap()
    }

    /// A builder that defaults to a "would-pass-everything-against-a-twin"
    /// shape so each test only overrides the field it is probing.
    fn meta(id: Uuid, flag_id: Uuid, metric_ids: Vec<Uuid>) -> ExperimentMeta {
        ExperimentMeta {
            id,
            flag_id,
            started_at: ts(1),
            ended_at: Some(ts(30)),
            metric_ids,
            exclusion_group_id: None,
        }
    }

    #[test]
    fn includes_valid_overlapping_pair() {
        let metric = Uuid::new_v4();
        let a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let pairs = candidate_pairs(&[a.clone(), b.clone()]);
        assert_eq!(pairs.len(), 1);
        let (lo, hi) = (a.id.min(b.id), a.id.max(b.id));
        assert_eq!(pairs[0], (lo, hi));
    }

    #[test]
    fn excludes_same_flag_pair() {
        let metric = Uuid::new_v4();
        let flag = Uuid::new_v4();
        let a = meta(Uuid::new_v4(), flag, vec![metric]);
        let b = meta(Uuid::new_v4(), flag, vec![metric]);
        assert!(candidate_pairs(&[a, b]).is_empty());
    }

    #[test]
    fn excludes_same_exclusion_group_pair() {
        let metric = Uuid::new_v4();
        let group = Uuid::new_v4();
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let mut b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        a.exclusion_group_id = Some(group);
        b.exclusion_group_id = Some(group);
        assert!(candidate_pairs(&[a, b]).is_empty());
    }

    #[test]
    fn includes_pair_in_different_exclusion_groups() {
        let metric = Uuid::new_v4();
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let mut b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        a.exclusion_group_id = Some(Uuid::new_v4());
        b.exclusion_group_id = Some(Uuid::new_v4());
        assert_eq!(candidate_pairs(&[a, b]).len(), 1);
    }

    #[test]
    fn includes_pair_when_only_one_has_exclusion_group() {
        let metric = Uuid::new_v4();
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        a.exclusion_group_id = Some(Uuid::new_v4());
        assert_eq!(candidate_pairs(&[a, b]).len(), 1);
    }

    #[test]
    fn excludes_non_overlapping_windows() {
        let metric = Uuid::new_v4();
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let mut b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        // A: days 1..10, B: days 20..30 — disjoint.
        a.started_at = ts(1);
        a.ended_at = Some(ts(10));
        b.started_at = ts(20);
        b.ended_at = Some(ts(30));
        assert!(candidate_pairs(&[a, b]).is_empty());
    }

    #[test]
    fn touching_windows_do_not_overlap() {
        // Half-open: A ends exactly when B starts ⇒ no shared instant.
        let metric = Uuid::new_v4();
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let mut b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        a.started_at = ts(1);
        a.ended_at = Some(ts(10));
        b.started_at = ts(10);
        b.ended_at = Some(ts(20));
        assert!(candidate_pairs(&[a, b]).is_empty());
    }

    #[test]
    fn running_experiment_overlaps_open_ended() {
        let metric = Uuid::new_v4();
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        let mut b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![metric]);
        // A still running (None end), B a closed window starting after A's start.
        a.started_at = ts(1);
        a.ended_at = None;
        b.started_at = ts(5);
        b.ended_at = Some(ts(10));
        assert_eq!(candidate_pairs(&[a, b]).len(), 1);
    }

    #[test]
    fn excludes_empty_metric_intersection() {
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![Uuid::new_v4()]);
        let mut b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![Uuid::new_v4()]);
        // Disjoint metric sets.
        a.metric_ids = vec![Uuid::new_v4()];
        b.metric_ids = vec![Uuid::new_v4()];
        assert!(candidate_pairs(&[a, b]).is_empty());
    }

    #[test]
    fn includes_partial_metric_overlap() {
        let shared = Uuid::new_v4();
        let a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![shared, Uuid::new_v4()]);
        let b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![Uuid::new_v4(), shared]);
        assert_eq!(candidate_pairs(&[a, b]).len(), 1);
    }

    #[test]
    fn empty_metric_lists_never_share() {
        let a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![]);
        let b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![]);
        assert!(candidate_pairs(&[a, b]).is_empty());
    }

    #[test]
    fn pair_ordering_is_stable_min_max() {
        let metric = Uuid::new_v4();
        let lo_id = Uuid::from_u128(1);
        let hi_id = Uuid::from_u128(2);
        let a = meta(hi_id, Uuid::new_v4(), vec![metric]);
        let b = meta(lo_id, Uuid::new_v4(), vec![metric]);
        // Input order is (hi, lo) but the emitted pair must be (lo, hi).
        let pairs = candidate_pairs(&[a, b]);
        assert_eq!(pairs, vec![(lo_id, hi_id)]);
    }

    #[test]
    fn enumerates_all_valid_pairs_among_three() {
        let metric = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), vec![metric]);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), vec![metric]);
        let c = meta(Uuid::from_u128(3), Uuid::new_v4(), vec![metric]);
        let pairs = candidate_pairs(&[a, b, c]);
        assert_eq!(pairs.len(), 3, "all three unordered pairs are candidates");
        assert!(pairs.contains(&(Uuid::from_u128(1), Uuid::from_u128(2))));
        assert!(pairs.contains(&(Uuid::from_u128(1), Uuid::from_u128(3))));
        assert!(pairs.contains(&(Uuid::from_u128(2), Uuid::from_u128(3))));
    }

    #[test]
    fn empty_and_singleton_inputs_produce_no_pairs() {
        assert!(candidate_pairs(&[]).is_empty());
        let solo = meta(Uuid::new_v4(), Uuid::new_v4(), vec![Uuid::new_v4()]);
        assert!(candidate_pairs(&[solo]).is_empty());
    }
}
