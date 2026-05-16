//! ClickHouse schema migration runner.
//!
//! Migrations are embedded at compile time from `migrations/*.sql` and applied in filename order.
//! A `_schema_migrations` table in ClickHouse tracks which have already run.

use clickhouse::Client;

/// Errors that can occur while applying ClickHouse migrations.
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// Underlying ClickHouse client error.
    #[error("ClickHouse error: {0}")]
    Clickhouse(#[from] clickhouse::error::Error),
    /// A specific migration statement failed.
    #[error("migration {name} failed: {source}")]
    Apply {
        /// Name of the failed migration.
        name: String,
        /// Underlying error.
        #[source]
        source: clickhouse::error::Error,
    },
}

/// An embedded migration: (filename, SQL content).
static MIGRATIONS: &[(&str, &str)] = &[
    (
        "20260419000001_events",
        include_str!("../migrations/20260419000001_events.sql"),
    ),
    (
        "20260419000002_metric_definitions",
        include_str!("../migrations/20260419000002_metric_definitions.sql"),
    ),
    (
        "20260419000003_events_count_mv",
        include_str!("../migrations/20260419000003_events_count_mv.sql"),
    ),
    (
        "20260419000004_events_numeric_mv",
        include_str!("../migrations/20260419000004_events_numeric_mv.sql"),
    ),
    (
        "20260516000005_events_experiment_daily_mv",
        include_str!("../migrations/20260516000005_events_experiment_daily_mv.sql"),
    ),
    (
        "20260516000006_events_experiment_daily_backfill",
        include_str!("../migrations/20260516000006_events_experiment_daily_backfill.sql"),
    ),
];

/// Apply all pending ClickHouse migrations.
///
/// Creates `_schema_migrations` if it does not exist, then runs each migration whose name is
/// not already recorded. Migrations are applied in filename order.
///
/// # Errors
/// Returns [`MigrationError`] if the tracker table cannot be created or any migration fails.
pub async fn run(client: &Client) -> Result<(), MigrationError> {
    client
        .query(
            "CREATE TABLE IF NOT EXISTS _schema_migrations
             (name String, applied_at DateTime64(3, 'UTC') DEFAULT now64())
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{database}/{table}', '{replica}', applied_at)
             ORDER BY name",
        )
        .execute()
        .await?;

    for (name, sql) in MIGRATIONS {
        let applied: u64 = client
            .query("SELECT count() FROM _schema_migrations WHERE name = ?")
            .bind(name)
            .fetch_one()
            .await?;

        if applied > 0 {
            tracing::debug!(migration = name, "already applied, skipping");
            continue;
        }

        tracing::info!(migration = name, "applying ClickHouse migration");

        // Split on semicolons so multi-statement migration files work (e.g. table + MV).
        // Strip leading SQL line-comments before checking if a statement is empty,
        // since our migration files include comment blocks above each DDL statement.
        for statement in sql.split(';') {
            let stmt: String = statement
                .lines()
                .filter(|l| !l.trim().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            client
                .query(stmt)
                .execute()
                .await
                .map_err(|e| MigrationError::Apply {
                    name: name.to_string(),
                    source: e,
                })?;
        }

        client
            .query("INSERT INTO _schema_migrations (name) VALUES (?)")
            .bind(name)
            .execute()
            .await?;

        tracing::info!(migration = name, "migration applied");
    }

    Ok(())
}
