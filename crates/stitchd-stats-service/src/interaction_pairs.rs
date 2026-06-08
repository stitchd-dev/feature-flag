//! Candidate-pair enumeration for cross-experiment interaction detection.
//!
//! Before the stats service runs the (expensive) per-metric interaction cell
//! queries (`queries::interaction_metric`) for an environment, it must decide
//! *which* pairs (and triples) of experiments are even worth testing. Testing
//! every unordered pair is O(n²)
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
    // Thin wrapper over the arity-general [`candidate_tuples`] at order 2.
    candidate_tuples(experiments, 2)
        .into_iter()
        .map(|t| (t[0], t[1]))
        .collect()
}

/// Enumerate the unordered experiment **triples** worth testing for a three-way
/// interaction.
///
/// A triple `(a, b, c)` survives iff every constituent pair satisfies
/// [`can_interact`] (distinct flags, overlapping windows, not same exclusion
/// group) AND all three share at least one **common** metric (`a ∩ b ∩ c`).
///
/// Two notes on why pairwise checks suffice for the windows:
/// - active windows are 1-D intervals, so by Helly's theorem pairwise overlap
///   implies a non-empty *common* live window — no separate triple-window check
///   is needed.
/// - the common-metric requirement is stricter than pairwise `shares_metric`:
///   the 3-way term is computed over a metric all three carry, so we test the
///   three-way intersection explicitly.
///
/// Each surviving triple is returned once, sorted ascending by id (`lo, mid, hi`)
/// for a stable, dedupe-friendly ordering regardless of input order.
#[must_use]
pub fn candidate_triples(experiments: &[ExperimentMeta]) -> Vec<(ExpId, ExpId, ExpId)> {
    // Thin wrapper over the arity-general [`candidate_tuples`] at order 3.
    candidate_tuples(experiments, 3)
        .into_iter()
        .map(|t| (t[0], t[1], t[2]))
        .collect()
}

/// Enumerate the unordered experiment **tuples** of a given `order` worth
/// testing for an `order`-way interaction — the arity-generalised
/// [`candidate_pairs`] (order 2) / [`candidate_triples`] (order 3).
///
/// A tuple survives iff **every** constituent pair satisfies [`can_interact`]
/// (distinct flags, overlapping windows, not same exclusion group) AND all
/// `order` experiments share at least one **common** metric (the interaction
/// term is computed over a metric every participant carries).
///
/// Two notes carry over from [`candidate_triples`] to arbitrary order:
/// - active windows are 1-D intervals, so by **Helly's theorem** pairwise
///   overlap implies a non-empty *common* live window — no separate tuple-window
///   check is needed at any order.
/// - the common-metric requirement is the `order`-way metric intersection, which
///   is stricter than pairwise `shares_metric`.
///
/// `order < 2` yields no tuples. Each surviving tuple is returned once, with its
/// ids sorted ascending, in a stable lexicographic order over the sorted ids.
#[must_use]
pub fn candidate_tuples(experiments: &[ExperimentMeta], order: usize) -> Vec<Vec<ExpId>> {
    if order < 2 || experiments.len() < order {
        return Vec::new();
    }
    let n = experiments.len();
    let mut out = Vec::new();
    // Iterate k-combinations of indices via an index stack (ascending), so the
    // emitted tuples are already in lexicographic index order.
    let mut combo: Vec<usize> = (0..order).collect();
    loop {
        let members: Vec<&ExperimentMeta> = combo.iter().map(|&i| &experiments[i]).collect();
        // Every constituent pair must be able to interact …
        let all_pairs_ok =
            (0..order).all(|i| ((i + 1)..order).all(|j| can_interact(members[i], members[j])));
        if all_pairs_ok && shares_metric_all(&members) {
            let mut ids: Vec<ExpId> = members.iter().map(|m| m.id).collect();
            ids.sort();
            out.push(ids);
        }

        // Advance to the next ascending index combination (k-combination of n).
        let mut i = order;
        loop {
            if i == 0 {
                return out;
            }
            i -= 1;
            if combo[i] != i + n - order {
                combo[i] += 1;
                for j in (i + 1)..order {
                    combo[j] = combo[j - 1] + 1;
                }
                break;
            }
        }
    }
}

/// Whether every experiment in `members` shares at least one common metric
/// (`⋂ metric_ids ≠ ∅`) — required for an `order`-way interaction term.
fn shares_metric_all(members: &[&ExperimentMeta]) -> bool {
    let Some((first, rest)) = members.split_first() else {
        return false;
    };
    first
        .metric_ids
        .iter()
        .any(|m| rest.iter().all(|meta| meta.metric_ids.contains(m)))
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

    // ── candidate_triples (P3.T1) ────────────────────────────────────────────

    #[test]
    fn includes_valid_triple_with_common_metric() {
        let m = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), vec![m]);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), vec![m]);
        let c = meta(Uuid::from_u128(3), Uuid::new_v4(), vec![m]);
        let triples = candidate_triples(&[c, a, b]); // unsorted input
        assert_eq!(
            triples,
            vec![(Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3))]
        );
    }

    #[test]
    fn excludes_triple_without_a_common_metric() {
        // Each PAIR shares a metric, but a ∩ b ∩ c is empty.
        let m1 = Uuid::new_v4();
        let m2 = Uuid::new_v4();
        let m3 = Uuid::new_v4();
        let a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m1, m2]);
        let b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m1, m3]);
        let c = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m2, m3]);
        assert!(candidate_triples(&[a, b, c]).is_empty());
    }

    #[test]
    fn excludes_triple_when_two_share_a_flag() {
        let m = Uuid::new_v4();
        let flag = Uuid::new_v4();
        let a = meta(Uuid::new_v4(), flag, vec![m]);
        let b = meta(Uuid::new_v4(), flag, vec![m]);
        let c = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]);
        assert!(candidate_triples(&[a, b, c]).is_empty());
    }

    #[test]
    fn excludes_triple_when_two_share_an_exclusion_group() {
        let m = Uuid::new_v4();
        let g = Uuid::new_v4();
        let mut a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]);
        let mut b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]);
        let c = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]);
        a.exclusion_group_id = Some(g);
        b.exclusion_group_id = Some(g);
        assert!(candidate_triples(&[a, b, c]).is_empty());
    }

    #[test]
    fn excludes_triple_when_one_pair_windows_disjoint() {
        let m = Uuid::new_v4();
        let a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]); // days 1..30
        let b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]); // days 1..30
        let mut c = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]);
        c.started_at = ts(20);
        c.ended_at = Some(ts(30));
        let mut a2 = a.clone();
        a2.ended_at = Some(ts(10)); // a2 ends before c starts → (a2,c) disjoint
        assert!(candidate_triples(&[a2, b, c]).is_empty());
    }

    #[test]
    fn fewer_than_three_inputs_produce_no_triples() {
        let m = Uuid::new_v4();
        let a = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]);
        let b = meta(Uuid::new_v4(), Uuid::new_v4(), vec![m]);
        assert!(candidate_triples(&[]).is_empty());
        assert!(candidate_triples(std::slice::from_ref(&a)).is_empty());
        assert!(candidate_triples(&[a, b]).is_empty());
    }

    #[test]
    fn enumerates_all_valid_triples_among_four() {
        let m = Uuid::new_v4();
        let xs: Vec<ExperimentMeta> = (1..=4)
            .map(|i| meta(Uuid::from_u128(i), Uuid::new_v4(), vec![m]))
            .collect();
        // C(4,3) = 4 triples.
        assert_eq!(candidate_triples(&xs).len(), 4);
    }

    // ── candidate_tuples (Phase 10: order 4+) ─────────────────────────────────

    #[test]
    fn candidate_tuples_order2_matches_candidate_pairs() {
        let m = Uuid::new_v4();
        let xs: Vec<ExperimentMeta> = (1..=4)
            .map(|i| meta(Uuid::from_u128(i), Uuid::new_v4(), vec![m]))
            .collect();
        let pairs = candidate_pairs(&xs);
        let tuples = candidate_tuples(&xs, 2);
        let as_pairs: Vec<(ExpId, ExpId)> = tuples.iter().map(|t| (t[0], t[1])).collect();
        assert_eq!(pairs, as_pairs, "order-2 tuples must equal candidate_pairs");
    }

    #[test]
    fn candidate_tuples_order3_matches_candidate_triples() {
        let m = Uuid::new_v4();
        let xs: Vec<ExperimentMeta> = (1..=5)
            .map(|i| meta(Uuid::from_u128(i), Uuid::new_v4(), vec![m]))
            .collect();
        let triples = candidate_triples(&xs);
        let tuples = candidate_tuples(&xs, 3);
        let as_triples: Vec<(ExpId, ExpId, ExpId)> =
            tuples.iter().map(|t| (t[0], t[1], t[2])).collect();
        assert_eq!(triples, as_triples);
    }

    #[test]
    fn candidate_tuples_order4_enumerates_all_quadruples() {
        let m = Uuid::new_v4();
        // 5 distinct-flag experiments all sharing one metric, overlapping → C(5,4)=5.
        let xs: Vec<ExperimentMeta> = (1..=5)
            .map(|i| meta(Uuid::from_u128(i), Uuid::new_v4(), vec![m]))
            .collect();
        let quads = candidate_tuples(&xs, 4);
        assert_eq!(quads.len(), 5, "C(5,4) = 5 quadruples");
        // Every tuple has 4 sorted-ascending ids.
        for q in &quads {
            assert_eq!(q.len(), 4);
            let mut sorted = q.clone();
            sorted.sort();
            assert_eq!(*q, sorted, "ids must be sorted ascending");
        }
        // The single C(4,4) over the first four.
        assert_eq!(candidate_tuples(&xs[..4], 4).len(), 1);
    }

    #[test]
    fn candidate_tuples_order4_requires_common_metric_across_all_four() {
        // Each pair shares SOME metric but the 4-way intersection is empty.
        let m1 = Uuid::new_v4();
        let m2 = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), vec![m1]);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), vec![m1, m2]);
        let c = meta(Uuid::from_u128(3), Uuid::new_v4(), vec![m2]);
        let d = meta(Uuid::from_u128(4), Uuid::new_v4(), vec![m1, m2]);
        // {a,c} share no metric → not even all pairs interact, and no common metric.
        assert!(candidate_tuples(&[a, b, c, d], 4).is_empty());
    }

    #[test]
    fn candidate_tuples_order4_excluded_when_one_pair_shares_a_flag() {
        let m = Uuid::new_v4();
        let flag = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), flag, vec![m]);
        let b = meta(Uuid::from_u128(2), flag, vec![m]); // shares flag with a
        let c = meta(Uuid::from_u128(3), Uuid::new_v4(), vec![m]);
        let d = meta(Uuid::from_u128(4), Uuid::new_v4(), vec![m]);
        assert!(candidate_tuples(&[a, b, c, d], 4).is_empty());
    }

    #[test]
    fn candidate_tuples_below_order_or_too_few_inputs_empty() {
        let m = Uuid::new_v4();
        let xs: Vec<ExperimentMeta> = (1..=3)
            .map(|i| meta(Uuid::from_u128(i), Uuid::new_v4(), vec![m]))
            .collect();
        assert!(candidate_tuples(&xs, 1).is_empty(), "order < 2 → none");
        assert!(
            candidate_tuples(&xs, 4).is_empty(),
            "fewer inputs than order"
        );
        assert!(candidate_tuples(&[], 2).is_empty());
    }
}
