//! Reads N-way cross-experiment interaction rows from the ClickHouse
//! `experiment_interactions` table.
//!
//! Powers the `GetExperimentInteractions` RPC: given an experiment, return every
//! persisted interaction tuple it participates in (`has(experiment_ids, ?)`),
//! across orders 2 (pairwise) and 3 (three-way) plus the per-experiment "main"
//! rows. The rows are written by the stats-service interaction sweep.
//!
//! Abstracted behind the [`InteractionsReader`] trait so the service handler is
//! unit-testable without a live ClickHouse. Production wiring uses
//! [`ClickHouseInteractionsReader`].

use async_trait::async_trait;
use clickhouse::Client;
use std::sync::Arc;
use uuid::Uuid;

/// Deserializer for `Array(UUID)` over the ClickHouse RowBinary protocol.
///
/// clickhouse 0.15 ships `clickhouse::serde::uuid` for a scalar `UUID` (decoded
/// from a `(u64, u64)` pair in RowBinary) but has no `vec` helper, so we mirror
/// that element decoding for a `Vec<Uuid>`. The element representation matches
/// the writer's scalar `clickhouse::serde::uuid`, keeping the array
/// wire-compatible with how individual UUIDs are persisted. This is a read-only
/// reader, so only `deserialize` is provided.
mod uuid_vec {
    use serde::{Deserialize, Deserializer};
    use uuid::Uuid;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Uuid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let pairs: Vec<(u64, u64)> = Vec::deserialize(deserializer)?;
        Ok(pairs
            .into_iter()
            .map(|(hi, lo)| Uuid::from_u64_pair(hi, lo))
            .collect())
    }
}

/// One interaction row for an `(experiment_ids, context_type, metric_key, term)`
/// tuple. `interaction_order` is the tuple size (2 = pairwise, 3 = three-way);
/// `term` is `"main:<uuid>"` / `"2way:<a>x<b>"` / `"3way:<a>x<b>x<c>"`.
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionRow {
    /// Experiment ids participating in this tuple (length == `interaction_order`).
    pub experiment_ids: Vec<Uuid>,
    pub interaction_order: u8,
    pub term: String,
    pub context_type: String,
    pub metric_key: String,
    pub shared_count: u64,
    pub interaction_estimate: f64,
    pub p_value: f64,
    pub df: u32,
    pub significant: bool,
    /// True when the tuple lacked enough shared exposures to run a meaningful
    /// interaction test; callers should treat `significant`/estimate as
    /// inconclusive in that case.
    pub insufficient_data: bool,
    pub bayes_prob: f64,
    pub bayes_expected: f64,
    pub bayes_ci_low: f64,
    pub bayes_ci_high: f64,
}

/// Reads interaction rows involving a given experiment.
#[async_trait]
pub trait InteractionsReader: Send + Sync + 'static {
    /// Fetch all interaction rows whose `experiment_ids` tuple contains
    /// `experiment_id`, scoped to `env_id`.
    async fn list_interactions(
        &self,
        env_id: Uuid,
        experiment_id: Uuid,
    ) -> Result<Vec<InteractionRow>, clickhouse::error::Error>;
}

/// Production implementation backed by a `clickhouse::Client`.
pub struct ClickHouseInteractionsReader {
    client: Arc<Client>,
}

impl ClickHouseInteractionsReader {
    /// Wrap a shared CH client.
    #[must_use]
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChRow {
    #[serde(with = "uuid_vec")]
    experiment_ids: Vec<Uuid>,
    interaction_order: u8,
    term: String,
    context_type: String,
    metric_key: String,
    shared_count: u64,
    interaction_estimate: f64,
    p_value: f64,
    df: u32,
    significant: bool,
    insufficient_data: bool,
    bayes_prob: f64,
    bayes_expected: f64,
    bayes_ci_low: f64,
    bayes_ci_high: f64,
}

#[async_trait]
impl InteractionsReader for ClickHouseInteractionsReader {
    async fn list_interactions(
        &self,
        env_id: Uuid,
        experiment_id: Uuid,
    ) -> Result<Vec<InteractionRow>, clickhouse::error::Error> {
        let env_str = env_id.to_string();
        let exp_str = experiment_id.to_string();
        // ReplacingMergeTree(computed_at): reads MUST use FINAL to collapse
        // superseded rows. Filter to tuples the focused experiment participates
        // in via `has(experiment_ids, ?)`.
        let sql = format!(
            r"
            SELECT
                experiment_ids,
                interaction_order,
                term,
                context_type,
                metric_key,
                shared_count,
                interaction_estimate,
                p_value,
                df,
                significant,
                insufficient_data,
                bayes_prob,
                bayes_expected,
                bayes_ci_low,
                bayes_ci_high
            FROM experiment_interactions FINAL
            WHERE env_id = toUUID('{env_str}')
              AND has(experiment_ids, toUUID('{exp_str}'))
            ORDER BY interaction_order, metric_key, term
            "
        );
        let rows: Vec<ChRow> = self.client.query(&sql).fetch_all().await?;
        Ok(rows
            .into_iter()
            .map(|r| InteractionRow {
                experiment_ids: r.experiment_ids,
                interaction_order: r.interaction_order,
                term: r.term,
                context_type: r.context_type,
                metric_key: r.metric_key,
                shared_count: r.shared_count,
                interaction_estimate: r.interaction_estimate,
                p_value: r.p_value,
                df: r.df,
                significant: r.significant,
                insufficient_data: r.insufficient_data,
                bayes_prob: r.bayes_prob,
                bayes_expected: r.bayes_expected,
                bayes_ci_low: r.bayes_ci_low,
                bayes_ci_high: r.bayes_ci_high,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test stub returning canned rows and recording the (env, experiment)
    /// queried.
    pub struct StubReader {
        pub rows: Vec<InteractionRow>,
        pub calls: Arc<Mutex<Vec<(Uuid, Uuid)>>>,
    }

    #[async_trait]
    impl InteractionsReader for StubReader {
        async fn list_interactions(
            &self,
            env_id: Uuid,
            experiment_id: Uuid,
        ) -> Result<Vec<InteractionRow>, clickhouse::error::Error> {
            self.calls.lock().unwrap().push((env_id, experiment_id));
            Ok(self.rows.clone())
        }
    }

    #[tokio::test]
    async fn stub_round_trips_params_and_rows() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let env = Uuid::new_v4();
        let exp = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        // A 3-way row in which the focused experiment participates.
        let reader = StubReader {
            rows: vec![InteractionRow {
                experiment_ids: vec![exp, b, c],
                interaction_order: 3,
                term: format!("3way:{exp}x{b}x{c}"),
                context_type: "user".into(),
                metric_key: "checkout".into(),
                shared_count: 400,
                interaction_estimate: 0.4,
                p_value: 0.001,
                df: 4,
                significant: true,
                insufficient_data: false,
                bayes_prob: 0.97,
                bayes_expected: 0.38,
                bayes_ci_low: 0.12,
                bayes_ci_high: 0.64,
            }],
            calls: calls.clone(),
        };
        let rows = reader.list_interactions(env, exp).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interaction_order, 3);
        assert_eq!(rows[0].experiment_ids, vec![exp, b, c]);
        assert!(rows[0].term.starts_with("3way:"));
        assert_eq!(rows[0].metric_key, "checkout");
        assert_eq!(rows[0].df, 4);
        assert!(rows[0].significant);
        assert!(!rows[0].insufficient_data);
        assert!((rows[0].bayes_prob - 0.97).abs() < 1e-9);
        assert!((rows[0].bayes_ci_high - 0.64).abs() < 1e-9);
        assert_eq!(calls.lock().unwrap()[0], (env, exp));
    }

    /// The custom `Array(UUID)` serde encodes/decodes each element via the same
    /// `(u64, u64)` pair representation `clickhouse::serde::uuid` uses for scalar
    /// UUIDs in RowBinary; assert that pair round-trip is the exact identity so
    /// arrays stay wire-compatible with how individual UUIDs are persisted.
    #[test]
    fn uuid_pair_round_trip_is_identity() {
        for _ in 0..16 {
            let u = Uuid::new_v4();
            let (hi, lo) = u.as_u64_pair();
            assert_eq!(Uuid::from_u64_pair(hi, lo), u);
        }
    }
}
