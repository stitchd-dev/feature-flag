//! Interaction-detection query builder.
//!
//! Two experiments A and B can interact when they share contexts: a context
//! bucketed into variant `a_v` of A *and* variant `b_v` of B may behave
//! differently than the additive sum of each treatment in isolation. To test
//! for that we need the cross-tabulation of *shared* contexts by the two
//! experiments' variant assignments — a contingency table of
//! `(a.variant_key, b.variant_key) -> count of shared contexts`.
//!
//! This module produces the two pure [`BuiltQuery`]s the dispatcher needs:
//!
//! - [`build_interaction_cells_query`] — the per-variant-combination
//!   contingency table (`a.variant_key, b.variant_key, count() AS cell_count`)
//!   over contexts that were assigned to BOTH experiments.
//! - [`build_shared_count_query`] — the scalar total number of shared
//!   contexts (`count()`), i.e. the grand total of the contingency table.
//!   Surfaced separately so the dispatcher can short-circuit on an empty /
//!   too-small overlap without materialising the full cell grid.
//!
//! ## Self-join shape
//!
//! `experiment_assignments` is a `ReplacingMergeTree(_version)` table: a
//! context that re-enters an experiment writes a second physical row, and the
//! engine only collapses duplicates at merge time. Readers MUST therefore use
//! `FINAL` (or an `argMin`/`argMax` collapse) to see one logical row per
//! `(experiment, iteration, context_type, context_key)`. Both builders read
//! `experiment_assignments FINAL` and self-join the table to itself —
//! `a` filtered to experiment A, `b` filtered to experiment B — on the shared
//! context identity `(env_id, context_type, context_key)`. The INNER JOIN
//! keeps only contexts present in *both* experiments, which is exactly the
//! shared-population we want to cross-tabulate.
//!
//! ## Window scoping
//!
//! Interaction is only meaningful while both experiments were live at the same
//! time, so each side is bounded by `assigned_at < interaction_end` (the upper
//! bound on the overlap window the caller computed from the two experiments'
//! active windows). The lower bound is implicit: a context can only appear in
//! both experiments if it was exposed to both, and the INNER JOIN enforces
//! that. The caller passes `interaction_end` because the iteration / window
//! tables live in PostgreSQL, not ClickHouse.
//!
//! ## Bind order
//!
//! `clickhouse-rs` binds `?` placeholders by SQL position, not by
//! `Vec<QueryBind>` index, so every push happens in left-to-right SQL
//! appearance order. Both builders emit binds in this sequence:
//!
//!   1. `env_id`          (`a.env_id = toUUID(?)`)
//!   2. `exp_a`           (`a.experiment_id = toUUID(?)`)
//!   3. `context_type`    (`a.context_type = ?`)
//!   4. `interaction_end` (`a.assigned_at < ?` — A side)
//!   5. `exp_b`           (`b.experiment_id = toUUID(?)`)
//!   6. `interaction_end` (`b.assigned_at < ?` — B side; re-bound)
//!
//! `env_id` and `context_type` are not re-bound for the B side: the JOIN `ON`
//! already equates `b.env_id`/`b.context_type` to the A side, so a single bind
//! constrains both legs.
//!
//! See the parent module ([`super`]) for [`BuiltQuery`] / [`QueryBind`] /
//! [`QueryBuildError`].
//!
//! [`BuiltQuery`]: super::BuiltQuery
//! [`QueryBind`]: super::QueryBind
//! [`QueryBuildError`]: super::QueryBuildError

use chrono::{DateTime, Utc};

use super::{BuiltQuery, QueryBind, QueryBuildError, push_bind};

// ── Public API ───────────────────────────────────────────────────────────────

/// Build the per-variant-combination contingency-table query for the
/// interaction between experiments `exp_a` and `exp_b` within `env_id`,
/// restricted to a single `context_type`.
///
/// The result rows are `(a_variant_key: String, b_variant_key: String,
/// cell_count: u64)` — one row per `(A-variant, B-variant)` combination that
/// has at least one shared context. Empty variant keys are filtered out via a
/// `HAVING`-equivalent `WHERE` guard so phantom/blank variants never leak into
/// the χ² grid.
///
/// `interaction_end` upper-bounds both sides' `assigned_at`. Callers pass the
/// end of the overlap window between the two experiments' active periods.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_a == exp_b` — an
/// experiment cannot interact with itself, and the self-join would otherwise
/// produce a meaningless variant-vs-itself diagonal.
pub fn build_interaction_cells_query(
    env_id: &str,
    exp_a: &str,
    exp_b: &str,
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    if exp_a == exp_b {
        return Err(QueryBuildError::InvalidConfig(
            "exp_a and exp_b must be distinct experiments".into(),
        ));
    }

    let mut binds = Vec::new();
    let (a_filter, b_filter) = emit_filters(&mut binds, env_id, exp_a, exp_b, context_type, interaction_end);

    let sql = format!(
        "SELECT\n    \
            a.variant_key AS a_variant_key,\n    \
            b.variant_key AS b_variant_key,\n    \
            count() AS cell_count\n\
        FROM experiment_assignments AS a FINAL\n\
        INNER JOIN experiment_assignments AS b FINAL\n    \
            ON a.env_id = b.env_id\n   \
            AND a.context_type = b.context_type\n   \
            AND a.context_key = b.context_key\n\
        WHERE {a_filter}\n  \
          AND {b_filter}\n  \
          AND a.variant_key != ''\n  \
          AND b.variant_key != ''\n\
        GROUP BY a.variant_key, b.variant_key"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build the scalar shared-context-count query for the interaction between
/// `exp_a` and `exp_b` within `env_id`, restricted to a single `context_type`.
///
/// Returns a single row with one column, `shared_count: u64` — the number of
/// contexts assigned to BOTH experiments (the grand total of the contingency
/// table produced by [`build_interaction_cells_query`]). Empty variant keys
/// are excluded so the count matches the cells grid's grand total exactly.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_a == exp_b`.
pub fn build_shared_count_query(
    env_id: &str,
    exp_a: &str,
    exp_b: &str,
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    if exp_a == exp_b {
        return Err(QueryBuildError::InvalidConfig(
            "exp_a and exp_b must be distinct experiments".into(),
        ));
    }

    let mut binds = Vec::new();
    let (a_filter, b_filter) = emit_filters(&mut binds, env_id, exp_a, exp_b, context_type, interaction_end);

    let sql = format!(
        "SELECT count() AS shared_count\n\
        FROM experiment_assignments AS a FINAL\n\
        INNER JOIN experiment_assignments AS b FINAL\n    \
            ON a.env_id = b.env_id\n   \
            AND a.context_type = b.context_type\n   \
            AND a.context_key = b.context_key\n\
        WHERE {a_filter}\n  \
          AND {b_filter}\n  \
          AND a.variant_key != ''\n  \
          AND b.variant_key != ''"
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Internal: shared WHERE-clause emission ───────────────────────────────────

/// Push the six interaction binds in SQL-appearance order and return the
/// rendered A-side and B-side filter fragments.
///
/// Both builders share the exact same predicate shape — keeping it in one
/// place means the cells query and the shared-count query can never drift out
/// of step on bind order or scoping semantics.
fn emit_filters(
    binds: &mut Vec<QueryBind>,
    env_id: &str,
    exp_a: &str,
    exp_b: &str,
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> (String, String) {
    let end_ms = interaction_end.timestamp_millis();

    // A side: env + experiment A + context_type + window upper bound.
    let env_ph = push_bind(binds, QueryBind::Str(env_id.to_owned()));
    let exp_a_ph = push_bind(binds, QueryBind::Str(exp_a.to_owned()));
    let ctx_ph = push_bind(binds, QueryBind::Str(context_type.to_owned()));
    let a_end_ph = push_bind(binds, QueryBind::I64(end_ms));
    let a_filter = format!(
        "a.env_id = toUUID({env_ph})\n  \
          AND a.experiment_id = toUUID({exp_a_ph})\n  \
          AND a.context_type = {ctx_ph}\n  \
          AND a.assigned_at < fromUnixTimestamp64Milli({a_end_ph})"
    );

    // B side: experiment B + window upper bound. env_id / context_type are
    // pinned to the A side by the JOIN ON, so they are not re-bound.
    let exp_b_ph = push_bind(binds, QueryBind::Str(exp_b.to_owned()));
    let b_end_ph = push_bind(binds, QueryBind::I64(end_ms));
    let b_filter = format!(
        "b.experiment_id = toUUID({exp_b_ph})\n  \
          AND b.assigned_at < fromUnixTimestamp64Milli({b_end_ph})"
    );

    (a_filter, b_filter)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_A: &str = "00000000-0000-0000-0000-0000000000aa";
    const EXP_B: &str = "00000000-0000-0000-0000-0000000000bb";

    fn end() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()
    }

    // ── cells query: SQL shape + bind order ──────────────────────────────────

    #[test]
    fn cells_query_reads_assignments_final_self_join() {
        let q = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        assert!(
            q.sql.contains("FROM experiment_assignments AS a FINAL"),
            "A side must read assignments FINAL, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("INNER JOIN experiment_assignments AS b FINAL"),
            "B side must self-join assignments FINAL, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn cells_query_joins_on_shared_context_identity() {
        let q = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        assert!(q.sql.contains("ON a.env_id = b.env_id"), "{}", q.sql);
        assert!(
            q.sql.contains("AND a.context_type = b.context_type"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("AND a.context_key = b.context_key"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn cells_query_selects_variant_combination_and_count() {
        let q = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        assert!(q.sql.contains("a.variant_key AS a_variant_key"), "{}", q.sql);
        assert!(q.sql.contains("b.variant_key AS b_variant_key"), "{}", q.sql);
        assert!(q.sql.contains("count() AS cell_count"), "{}", q.sql);
        assert!(
            q.sql.contains("GROUP BY a.variant_key, b.variant_key"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn cells_query_excludes_empty_variant_keys() {
        let q = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        assert!(q.sql.contains("a.variant_key != ''"), "{}", q.sql);
        assert!(q.sql.contains("b.variant_key != ''"), "{}", q.sql);
    }

    #[test]
    fn cells_query_bind_order() {
        let q = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        // env, exp_a, context_type, end_ms (A), exp_b, end_ms (B)
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_A.into()));
        assert_eq!(q.binds[2], QueryBind::Str("user".into()));
        assert_eq!(q.binds[3], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds[4], QueryBind::Str(EXP_B.into()));
        assert_eq!(q.binds[5], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds.len(), 6);
    }

    #[test]
    fn cells_query_placeholders_match_bind_positions() {
        let q = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        assert!(q.sql.contains("a.env_id = toUUID({p0})"), "{}", q.sql);
        assert!(
            q.sql.contains("a.experiment_id = toUUID({p1})"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("a.context_type = {p2}"), "{}", q.sql);
        assert!(
            q.sql
                .contains("a.assigned_at < fromUnixTimestamp64Milli({p3})"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("b.experiment_id = toUUID({p4})"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("b.assigned_at < fromUnixTimestamp64Milli({p5})"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn cells_query_rejects_self_interaction() {
        let err =
            build_interaction_cells_query(ENV_ID, EXP_A, EXP_A, "user", end()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    // ── shared-count query: SQL shape + bind order ───────────────────────────

    #[test]
    fn shared_count_query_selects_scalar_count() {
        let q = build_shared_count_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        assert!(q.sql.contains("SELECT count() AS shared_count"), "{}", q.sql);
        assert!(
            !q.sql.contains("GROUP BY"),
            "shared-count must be a scalar (no GROUP BY), got:\n{}",
            q.sql
        );
    }

    #[test]
    fn shared_count_query_same_join_and_binds_as_cells() {
        let cells = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "account", end()).unwrap();
        let count = build_shared_count_query(ENV_ID, EXP_A, EXP_B, "account", end()).unwrap();
        // Both must produce identical binds so the dispatcher can run them as a
        // pair against the same overlap scope.
        assert_eq!(cells.binds, count.binds);
        assert!(
            count
                .sql
                .contains("INNER JOIN experiment_assignments AS b FINAL"),
            "{}",
            count.sql
        );
    }

    #[test]
    fn shared_count_query_rejects_self_interaction() {
        let err = build_shared_count_query(ENV_ID, EXP_A, EXP_A, "user", end()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn context_type_is_bound_not_inlined() {
        // The context_type literal must be a bind, never inlined into SQL —
        // matches the parameterized-SQL invariant of the sibling builders.
        let q = build_interaction_cells_query(ENV_ID, EXP_A, EXP_B, "user", end()).unwrap();
        assert!(
            !q.sql.contains("'user'"),
            "context_type must be bound, not inlined, got:\n{}",
            q.sql
        );
        assert_eq!(q.binds[2], QueryBind::Str("user".into()));
    }
}
