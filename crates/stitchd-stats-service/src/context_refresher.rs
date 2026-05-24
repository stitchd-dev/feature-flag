//! Periodic refresh job for the context type / parameter registry.
//!
//! [`ContextRegistryRefresher::run`] is intended to execute on a 15-minute cadence:
//! 1. Fetch all distinct context observations from ClickHouse (last 90 days).
//! 2. Upsert every discovered context type and param into PostgreSQL.
//! 3. Purge registry entries not seen in the last 90 days.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use stitchd_core::context::InferredType;
use stitchd_core::id::EnvironmentId;
use stitchd_db::ContextRegistryRepository;

/// Aggregated view of one distinct (env_id, context_type) pair.
#[derive(Debug, Clone)]
pub struct ContextSummary {
    /// Environment the evaluation was recorded for.
    pub env_id: EnvironmentId,
    /// Context type (e.g. `"user"`, `"session"`).
    pub context_type: String,
    /// Param key → (inferred type, is_private flag).
    pub params: HashMap<String, (InferredType, bool)>,
}

/// Source of aggregated evaluation log data for the registry refresh.
#[async_trait]
pub trait EvalLogSource: Send + Sync {
    /// Return all distinct context observations since `since`.
    ///
    /// Returns one `ContextSummary` per (env_id, context_type) pair, with all
    /// param keys discovered across matching rows merged into `params`.
    async fn fetch_context_summaries(&self, since: DateTime<Utc>) -> Result<Vec<ContextSummary>>;
}

/// Refreshes the context type/param registry from ClickHouse evaluation log data.
pub struct ContextRegistryRefresher {
    source: Arc<dyn EvalLogSource>,
    registry: Arc<dyn ContextRegistryRepository>,
}

impl ContextRegistryRefresher {
    /// Construct a refresher backed by the given source and registry.
    pub fn new(
        source: Arc<dyn EvalLogSource>,
        registry: Arc<dyn ContextRegistryRepository>,
    ) -> Self {
        Self { source, registry }
    }

    /// Run one refresh cycle.
    pub async fn run(&self) -> Result<()> {
        let horizon = Duration::days(90);
        let since = Utc::now() - horizon;

        let summaries = self.source.fetch_context_summaries(since).await?;

        for summary in &summaries {
            self.registry
                .upsert_context_type(summary.env_id, &summary.context_type)
                .await?;

            for (param_key, (inferred_type, is_private)) in &summary.params {
                self.registry
                    .upsert_param(
                        summary.env_id,
                        &summary.context_type,
                        param_key,
                        *inferred_type,
                        *is_private,
                    )
                    .await?;
            }
        }

        let cutoff = Utc::now() - horizon;
        self.registry.purge_stale(cutoff).await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ClickHouse implementation
// ---------------------------------------------------------------------------

/// ClickHouse row for sampling evaluation log entries.
#[derive(clickhouse::Row, serde::Deserialize)]
pub struct EvalLogSampleRow {
    /// The environment UUID — deserialized as 16-byte binary.
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: Uuid,
    /// Context type string.
    pub context_type: String,
    /// JSON-encoded parameter map for this row.
    pub params_json: String,
}

/// ClickHouse-backed implementation of [`EvalLogSource`].
pub struct ClickHouseEvalLogSource {
    client: clickhouse::Client,
}

impl ClickHouseEvalLogSource {
    /// Construct a new source wrapping `client`.
    pub fn new(client: clickhouse::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EvalLogSource for ClickHouseEvalLogSource {
    async fn fetch_context_summaries(&self, since: DateTime<Utc>) -> Result<Vec<ContextSummary>> {
        let since_ms = since.timestamp_millis();

        let rows = self
            .client
            .query(&format!(
                r"
                SELECT DISTINCT env_id, context_type, params_json
                FROM flag_evaluation_log_v2
                WHERE evaluated_at >= fromUnixTimestamp64Milli({since_ms})
                "
            ))
            .fetch_all::<EvalLogSampleRow>()
            .await
            .map_err(|e| anyhow::anyhow!("ClickHouse query failed: {e}"))?;

        // Merge param keys across all rows for the same (env_id, context_type).
        let mut by_ctx: HashMap<(Uuid, String), HashMap<String, (InferredType, bool)>> =
            HashMap::new();

        for row in rows {
            let params = by_ctx
                .entry((row.env_id, row.context_type.clone()))
                .or_default();

            let json: serde_json::Value = serde_json::from_str(&row.params_json)
                .unwrap_or(serde_json::Value::Object(Default::default()));

            if let serde_json::Value::Object(map) = json {
                for (key, val) in map {
                    let val_str = match &val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let is_private = val_str == "********";
                    let inferred = InferredType::infer(&val_str);
                    // First observed value wins; don't downgrade a known type to Unknown.
                    params.entry(key).or_insert((inferred, is_private));
                }
            }
        }

        let summaries = by_ctx
            .into_iter()
            .map(|((env_id_uuid, context_type), params)| ContextSummary {
                env_id: EnvironmentId::from_uuid(env_id_uuid),
                context_type,
                params,
            })
            .collect();

        Ok(summaries)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeEvalLogSource {
        summaries: Vec<ContextSummary>,
    }

    impl FakeEvalLogSource {
        fn new(summaries: Vec<ContextSummary>) -> Self {
            Self { summaries }
        }
    }

    #[async_trait]
    impl EvalLogSource for FakeEvalLogSource {
        async fn fetch_context_summaries(
            &self,
            _since: DateTime<Utc>,
        ) -> Result<Vec<ContextSummary>> {
            Ok(self.summaries.clone())
        }
    }

    #[derive(Default)]
    #[allow(clippy::type_complexity)]
    struct FakeRegistry {
        upserted_types: Mutex<Vec<(EnvironmentId, String)>>,
        upserted_params: Mutex<Vec<(EnvironmentId, String, String, InferredType, bool)>>,
        purge_cutoffs: Mutex<Vec<DateTime<Utc>>>,
    }

    #[async_trait]
    impl ContextRegistryRepository for FakeRegistry {
        async fn upsert_context_type(
            &self,
            env_id: EnvironmentId,
            context_type: &str,
        ) -> std::result::Result<(), stitchd_db::RepositoryError> {
            self.upserted_types
                .lock()
                .unwrap()
                .push((env_id, context_type.to_string()));
            Ok(())
        }

        async fn upsert_param(
            &self,
            env_id: EnvironmentId,
            context_type: &str,
            param_key: &str,
            inferred_type: InferredType,
            is_private: bool,
        ) -> std::result::Result<(), stitchd_db::RepositoryError> {
            self.upserted_params.lock().unwrap().push((
                env_id,
                context_type.to_string(),
                param_key.to_string(),
                inferred_type,
                is_private,
            ));
            Ok(())
        }

        async fn list_types(
            &self,
            _env_id: EnvironmentId,
        ) -> std::result::Result<
            Vec<stitchd_core::context::ContextTypeRecord>,
            stitchd_db::RepositoryError,
        > {
            Ok(vec![])
        }

        async fn list_params(
            &self,
            _env_id: EnvironmentId,
            _context_type: &str,
        ) -> std::result::Result<
            Vec<stitchd_core::context::ContextParamRecord>,
            stitchd_db::RepositoryError,
        > {
            Ok(vec![])
        }

        async fn purge_stale(
            &self,
            older_than: DateTime<Utc>,
        ) -> std::result::Result<(), stitchd_db::RepositoryError> {
            self.purge_cutoffs.lock().unwrap().push(older_than);
            Ok(())
        }
    }

    #[tokio::test]
    async fn run_upserts_context_type_and_params() {
        let env_id = EnvironmentId::new();
        let mut params = HashMap::new();
        params.insert("email".to_string(), (InferredType::Str, true));
        params.insert("plan".to_string(), (InferredType::Str, false));

        let source = Arc::new(FakeEvalLogSource::new(vec![ContextSummary {
            env_id,
            context_type: "user".to_string(),
            params,
        }]));
        let registry = Arc::new(FakeRegistry::default());

        let refresher = ContextRegistryRefresher::new(source, registry.clone());
        refresher.run().await.unwrap();

        let types = registry.upserted_types.lock().unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].1, "user");
        assert_eq!(types[0].0, env_id);

        let params = registry.upserted_params.lock().unwrap();
        assert_eq!(params.len(), 2);

        let email = params.iter().find(|p| p.2 == "email").unwrap();
        assert_eq!(email.3, InferredType::Str);
        assert!(email.4, "email should be private");

        let plan = params.iter().find(|p| p.2 == "plan").unwrap();
        assert_eq!(plan.3, InferredType::Str);
        assert!(!plan.4, "plan should not be private");
    }

    #[tokio::test]
    async fn run_purges_stale_entries() {
        let source = Arc::new(FakeEvalLogSource::new(vec![]));
        let registry = Arc::new(FakeRegistry::default());

        let before = Utc::now();
        let refresher = ContextRegistryRefresher::new(source, registry.clone());
        refresher.run().await.unwrap();

        let cutoffs = registry.purge_cutoffs.lock().unwrap();
        assert_eq!(cutoffs.len(), 1);
        let cutoff = cutoffs[0];
        let expected_min = before - Duration::days(90) - Duration::seconds(5);
        let expected_max = Utc::now() - Duration::days(90) + Duration::seconds(5);
        assert!(
            cutoff >= expected_min && cutoff <= expected_max,
            "purge cutoff should be approximately 90 days ago"
        );
    }

    #[tokio::test]
    async fn run_with_multiple_context_types() {
        let env_id = EnvironmentId::new();

        let source = Arc::new(FakeEvalLogSource::new(vec![
            ContextSummary {
                env_id,
                context_type: "user".to_string(),
                params: HashMap::new(),
            },
            ContextSummary {
                env_id,
                context_type: "session".to_string(),
                params: HashMap::new(),
            },
        ]));
        let registry = Arc::new(FakeRegistry::default());

        let refresher = ContextRegistryRefresher::new(source, registry.clone());
        refresher.run().await.unwrap();

        let types = registry.upserted_types.lock().unwrap();
        assert_eq!(types.len(), 2);
        let type_names: Vec<&str> = types.iter().map(|t| t.1.as_str()).collect();
        assert!(type_names.contains(&"user"));
        assert!(type_names.contains(&"session"));
    }

    #[tokio::test]
    async fn run_with_empty_source_still_purges() {
        let source = Arc::new(FakeEvalLogSource::new(vec![]));
        let registry = Arc::new(FakeRegistry::default());

        let refresher = ContextRegistryRefresher::new(source, registry.clone());
        refresher.run().await.unwrap();

        let types = registry.upserted_types.lock().unwrap();
        assert!(types.is_empty());
        let cutoffs = registry.purge_cutoffs.lock().unwrap();
        assert_eq!(
            cutoffs.len(),
            1,
            "purge must run even when no new data arrives"
        );
    }
}
