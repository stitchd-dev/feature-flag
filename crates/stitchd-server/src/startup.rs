use std::time::Duration;
use sqlx::PgPool;
use tracing::{info, error};

/// Run pg_partman maintenance.
pub async fn run_partman_maintenance(pool: &PgPool) {
    info!("running pg_partman maintenance");
    // We use a plain query here because we don't want to depend on 
    // offline macros for an extension function that might not be present 
    // in all environments (e.g. local dev without partman).
    let result = sqlx::query("SELECT partman.run_maintenance(false)")
        .execute(pool)
        .await;

    if let Err(e) = result {
        // We log as warning because if partman is not installed, 
        // this will fail but the server should still run.
        tracing::warn!(error = %e, "pg_partman maintenance failed (is the extension installed?)");
    } else {
        info!("pg_partman maintenance complete");
    }
}

/// Start the background maintenance task.
pub fn spawn_maintenance_task(pool: PgPool) {
    tokio::spawn(async move {
        // Initial delay to avoid running immediately if we just ran it on startup
        tokio::time::sleep(Duration::from_secs(3600)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            run_partman_maintenance(&pool).await;
        }
    });
}
