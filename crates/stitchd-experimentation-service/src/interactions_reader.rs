//! Reads pairwise cross-experiment interaction rows from the ClickHouse
//! `experiment_interactions` table.
//!
//! Powers the `GetExperimentInteractions` RPC: given an experiment, return every
//! persisted interaction where it is either side of the pair. The rows are
//! written by the stats-service interaction sweep (Phase 6 Task 1).
//!
//! Abstracted behind the [`InteractionsReader`] trait so the service handler is
//! unit-testable without a live ClickHouse. Production wiring uses
//! [`ClickHouseInteractionsReader`].

use async_trait::async_trait;
use clickhouse::Client;
use std::sync::Arc;
use uuid::Uuid;

/// One interaction row for `(experiment_id_a, experiment_id_b, context_type,
/// metric_key)`.
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionRow {
    pub experiment_id_a: Uuid,
    pub experiment_id_b: Uuid,
    pub context_type: String,
    pub metric_key: String,
    pub shared_count: u64,
    pub interaction_estimate: f64,
    pub p_value: f64,
    pub significant: bool,
    /// True when the pair lacked enough shared exposures to run a meaningful
    /// interaction test; callers should treat `significant`/estimate as
    /// inconclusive in that case.
    pub insufficient_data: bool,
}

/// Reads interaction rows involving a given experiment.
#[async_trait]
pub trait InteractionsReader: Send + Sync + 'static {
    /// Fetch all interaction rows where `experiment_id` is `experiment_id_a` OR
    /// `experiment_id_b`, scoped to `env_id`.
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
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id_a: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id_b: Uuid,
    context_type: String,
    metric_key: String,
    shared_count: u64,
    interaction_estimate: f64,
    p_value: f64,
    significant: bool,
    insufficient_data: bool,
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
        let sql = format!(
            r"
            SELECT
                experiment_id_a,
                experiment_id_b,
                context_type,
                metric_key,
                shared_count,
                interaction_estimate,
                p_value,
                significant,
                insufficient_data
            FROM experiment_interactions
            WHERE env_id = toUUID('{env_str}')
              AND (experiment_id_a = toUUID('{exp_str}') OR experiment_id_b = toUUID('{exp_str}'))
            ORDER BY context_type, metric_key
            "
        );
        let rows: Vec<ChRow> = self.client.query(&sql).fetch_all().await?;
        Ok(rows
            .into_iter()
            .map(|r| InteractionRow {
                experiment_id_a: r.experiment_id_a,
                experiment_id_b: r.experiment_id_b,
                context_type: r.context_type,
                metric_key: r.metric_key,
                shared_count: r.shared_count,
                interaction_estimate: r.interaction_estimate,
                p_value: r.p_value,
                significant: r.significant,
                insufficient_data: r.insufficient_data,
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
        let other = Uuid::new_v4();
        let reader = StubReader {
            rows: vec![InteractionRow {
                experiment_id_a: exp,
                experiment_id_b: other,
                context_type: "user".into(),
                metric_key: "checkout".into(),
                shared_count: 400,
                interaction_estimate: 0.4,
                p_value: 0.001,
                significant: true,
                insufficient_data: false,
            }],
            calls: calls.clone(),
        };
        let rows = reader.list_interactions(env, exp).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].metric_key, "checkout");
        assert!(rows[0].significant);
        assert!(!rows[0].insufficient_data);
        assert_eq!(calls.lock().unwrap()[0], (env, exp));
    }
}
