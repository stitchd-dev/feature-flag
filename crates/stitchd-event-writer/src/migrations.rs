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
        "20260525000001_v1_baseline",
        include_str!("../migrations/20260525000001_v1_baseline.sql"),
    ),
    (
        "20260602000002_experiment_interactions",
        include_str!("../migrations/20260602000002_experiment_interactions.sql"),
    ),
    (
        "20260602000005_interaction_insufficient_data",
        include_str!("../migrations/20260602000005_interaction_insufficient_data.sql"),
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

        // Strip `--` line-comments FIRST, then split on `;`. The previous
        // split-then-strip order broke when a comment line contained a `;`
        // (e.g. `-- foo; bar`) — the split point fell mid-comment and the
        // post-`;` text leaked into the next "statement", confusing the CH
        // parser with stray words like "naming the" prepended to an ALTER.
        let stripped: String = sql
            .lines()
            .map(|l| {
                // Truncate each line at the start of a `--` comment (if any).
                // Keep the leading whitespace + pre-comment SQL.
                l.find("--").map_or(l, |idx| &l[..idx])
            })
            .collect::<Vec<_>>()
            .join("\n");

        for statement in stripped.split(';') {
            let stmt = statement.trim();
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
