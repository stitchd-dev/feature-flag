//! Environment-based configuration for `stitchd-stats-service`.

use std::time::Duration;

/// Runtime configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct StatsConfig {
    /// PostgreSQL connection URL (`DATABASE_URL`).
    pub database_url: String,
    /// ClickHouse HTTP endpoint (`CLICKHOUSE_URL`, default: `http://localhost:8123`).
    pub clickhouse_url: String,
    /// ClickHouse database name (`CLICKHOUSE_DB`, default: `stitchd`).
    pub clickhouse_db: String,
    /// How often the scheduler runs (`STATS_SCHEDULER_INTERVAL_SECS`, default: 3600).
    pub scheduler_interval: Duration,
    /// Port for the Axum health/metrics HTTP server (`STATS_HTTP_PORT`, default: 9200).
    pub http_port: u16,
    /// Port for the gRPC server (`STATS_GRPC_PORT`, default: 50056).
    pub grpc_port: u16,
}

impl StatsConfig {
    /// Load configuration from environment variables.
    ///
    /// # Errors
    /// Returns an error if `DATABASE_URL` is not set.
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL environment variable is required"))?;

        let clickhouse_url = std::env::var("CLICKHOUSE_URL")
            .unwrap_or_else(|_| "http://localhost:8123".to_string());

        let clickhouse_db =
            std::env::var("CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());

        let scheduler_interval_secs: u64 = std::env::var("STATS_SCHEDULER_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let http_port: u16 = std::env::var("STATS_HTTP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9200);

        let grpc_port: u16 = std::env::var("STATS_GRPC_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50056);

        Ok(Self {
            database_url,
            clickhouse_url,
            clickhouse_db,
            scheduler_interval: Duration::from_secs(scheduler_interval_secs),
            http_port,
            grpc_port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Save and restore DATABASE_URL around a config test to avoid poisoning
    /// the env for subsequent `#[sqlx::test]` tests.
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn new(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: single-threaded test binary with --test-threads=1
            unsafe {
                match &self.original {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn test_config_requires_database_url() {
        let _g = EnvGuard::new("DATABASE_URL");
        // SAFETY: single-threaded test binary with --test-threads=1
        unsafe { std::env::remove_var("DATABASE_URL") };
        let result = StatsConfig::from_env();
        assert!(result.is_err(), "should fail without DATABASE_URL");
        assert!(
            result.unwrap_err().to_string().contains("DATABASE_URL"),
            "error message should mention DATABASE_URL"
        );
    }

    #[test]
    fn test_config_loads_database_url() {
        let _g = EnvGuard::new("DATABASE_URL");
        // SAFETY: single-threaded test binary with --test-threads=1
        unsafe {
            std::env::set_var("DATABASE_URL", "postgresql://test:test@localhost/test");
        }
        let config = StatsConfig::from_env().expect("should load with DATABASE_URL set");
        assert_eq!(config.database_url, "postgresql://test:test@localhost/test");
    }

    #[test]
    fn test_config_defaults_clickhouse_url() {
        let _g_db = EnvGuard::new("DATABASE_URL");
        let _g_ch = EnvGuard::new("CLICKHOUSE_URL");
        // SAFETY: single-threaded test binary with --test-threads=1
        unsafe {
            std::env::set_var("DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("CLICKHOUSE_URL");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.clickhouse_url, "http://localhost:8123");
    }

    #[test]
    fn test_config_loads_clickhouse_url() {
        let _g_db = EnvGuard::new("DATABASE_URL");
        let _g_ch = EnvGuard::new("CLICKHOUSE_URL");
        // SAFETY: single-threaded test binary with --test-threads=1
        unsafe {
            std::env::set_var("DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::set_var("CLICKHOUSE_URL", "http://clickhouse:8123");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.clickhouse_url, "http://clickhouse:8123");
    }

    #[test]
    fn test_config_defaults_scheduler_interval() {
        let _g_db = EnvGuard::new("DATABASE_URL");
        let _g_si = EnvGuard::new("STATS_SCHEDULER_INTERVAL_SECS");
        // SAFETY: single-threaded test binary with --test-threads=1
        unsafe {
            std::env::set_var("DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STATS_SCHEDULER_INTERVAL_SECS");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.scheduler_interval.as_secs(), 3600);
    }

    #[test]
    fn test_config_loads_scheduler_interval() {
        let _g_db = EnvGuard::new("DATABASE_URL");
        let _g_si = EnvGuard::new("STATS_SCHEDULER_INTERVAL_SECS");
        // SAFETY: single-threaded test binary with --test-threads=1
        unsafe {
            std::env::set_var("DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::set_var("STATS_SCHEDULER_INTERVAL_SECS", "120");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.scheduler_interval.as_secs(), 120);
    }

    #[test]
    fn test_config_defaults_http_port() {
        let _g_db = EnvGuard::new("DATABASE_URL");
        let _g_hp = EnvGuard::new("STATS_HTTP_PORT");
        // SAFETY: single-threaded test binary with --test-threads=1
        unsafe {
            std::env::set_var("DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STATS_HTTP_PORT");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.http_port, 9200);
    }
}
