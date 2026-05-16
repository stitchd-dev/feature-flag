/// ScyllaDB migration applier.
///
/// Reads versioned `.cql` files from a directory (sorted lexicographically, so
/// files named `0001_*.cql`, `0002_*.cql`, … apply in the correct order).
/// Applied versions are tracked in the `scylla_migrations` table within the
/// configured keyspace so re-runs are idempotent.
use std::path::Path;

use crate::scylla::{ScyllaClient, ScyllaError};

/// Apply all pending `.cql` migrations from `migrations_dir`.
///
/// # Order
/// Files are sorted lexicographically. Use zero-padded numeric prefixes
/// (`0001_`, `0002_`, …) to guarantee ordering.
///
/// # Idempotency
/// Versions that already appear in `scylla_migrations` are skipped.
///
/// # Errors
/// Returns [`ScyllaError`] if any CQL statement fails or the schema cannot be
/// queried.
pub async fn run(client: &ScyllaClient, migrations_dir: &str) -> Result<(), ScyllaError> {
    let keyspace = client.keyspace().to_string();

    // Bootstrap keyspace + migrations tracking table on first run.
    bootstrap(client, &keyspace).await?;

    // Collect applied versions.
    let applied = applied_versions(client, &keyspace).await?;

    // Collect and sort .cql files.
    let dir = Path::new(migrations_dir);
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| ScyllaError::Schema(format!("cannot read migrations dir: {e}")))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "cql"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        let version = version_from_filename(&filename);
        if applied.contains(&version) {
            tracing::debug!(%filename, "skipping already-applied migration");
            continue;
        }

        let cql = std::fs::read_to_string(&path)
            .map_err(|e| ScyllaError::Schema(format!("cannot read {filename}: {e}")))?;

        apply_file(client, &keyspace, &filename, &version, &cql).await?;
    }

    Ok(())
}

/// Ensure the keyspace and `scylla_migrations` table exist.
async fn bootstrap(client: &ScyllaClient, keyspace: &str) -> Result<(), ScyllaError> {
    // Create keyspace with SimpleStrategy for dev (RF=1 for local/single-node).
    client
        .session()
        .query_unpaged(
            format!(
                "CREATE KEYSPACE IF NOT EXISTS {keyspace} \
                 WITH replication = {{'class': 'SimpleStrategy', 'replication_factor': 1}}"
            ),
            &[],
        )
        .await
        .map_err(|e| ScyllaError::Schema(format!("create keyspace failed: {e}")))?;

    // Create migrations tracking table.
    client
        .session()
        .query_unpaged(
            format!(
                "CREATE TABLE IF NOT EXISTS {keyspace}.scylla_migrations ( \
                     version TEXT PRIMARY KEY, \
                     filename TEXT, \
                     applied_at TIMESTAMP \
                 )"
            ),
            &[],
        )
        .await
        .map_err(|e| ScyllaError::Schema(format!("create scylla_migrations failed: {e}")))?;

    Ok(())
}

/// Return the set of version strings that have already been applied.
async fn applied_versions(
    client: &ScyllaClient,
    keyspace: &str,
) -> Result<std::collections::HashSet<String>, ScyllaError> {
    let rows = client
        .session()
        .query_unpaged(
            format!("SELECT version FROM {keyspace}.scylla_migrations"),
            &[],
        )
        .await
        .map_err(|e| ScyllaError::Query(format!("select versions failed: {e}")))?;

    let mut versions = std::collections::HashSet::new();
    if let Ok(result) = rows.into_rows_result() {
        for row in result.rows::<(String,)>().map_err(|e| {
            ScyllaError::Query(format!("deserialize versions failed: {e}"))
        })? {
            let (v,) = row.map_err(|e| ScyllaError::Query(format!("row error: {e}")))?;
            versions.insert(v);
        }
    }
    Ok(versions)
}

/// Execute each non-empty CQL statement in `cql` and record the migration.
async fn apply_file(
    client: &ScyllaClient,
    keyspace: &str,
    filename: &str,
    version: &str,
    cql: &str,
) -> Result<(), ScyllaError> {
    tracing::info!(%filename, "applying migration");

    // Replace keyspace placeholder with the actual keyspace name.
    let cql = cql.replace("__KEYSPACE__", keyspace);

    // Strip line comments (-- to end of line) before splitting so that
    // semicolons inside comment text don't produce spurious statement fragments.
    let stripped: String = cql
        .lines()
        .map(|line| {
            if let Some(pos) = line.find("--") {
                &line[..pos]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Split on ';' and execute each non-blank statement.
    for stmt in stripped.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        client
            .session()
            .query_unpaged(stmt, &[])
            .await
            .map_err(|e| {
                ScyllaError::Query(format!("migration {filename} failed at statement: {e}\n  CQL: {stmt}"))
            })?;
    }

    // Record this version.
    let now = chrono::Utc::now().timestamp_millis();
    client
        .session()
        .query_unpaged(
            format!(
                "INSERT INTO {keyspace}.scylla_migrations (version, filename, applied_at) \
                 VALUES ('{version}', '{filename}', {now})"
            ),
            &[],
        )
        .await
        .map_err(|e| ScyllaError::Query(format!("record migration version failed: {e}")))?;

    tracing::info!(%filename, "migration applied successfully");
    Ok(())
}

/// Extract a version string from a filename, e.g. `"0001_keyspace.cql"` → `"0001"`.
fn version_from_filename(filename: &str) -> String {
    filename
        .split('_')
        .next()
        .unwrap_or(filename)
        .trim_end_matches(".cql")
        .to_string()
}
