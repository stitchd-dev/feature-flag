//! Environment-based configuration for `stitchd-stats-service`.

use std::time::Duration;

/// Runtime configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct StatsConfig {
    /// PostgreSQL connection URL (`STITCHD_DATABASE_URL`).
    pub database_url: String,
    /// ClickHouse HTTP endpoint (`STITCHD_CLICKHOUSE_URL`, default: `http://localhost:8123`).
    pub clickhouse_url: String,
    /// ClickHouse database name (`STITCHD_CLICKHOUSE_DB`, default: `stitchd`).
    pub clickhouse_db: String,
    /// How often the scheduler runs (`STITCHD_STATS_SCHEDULER_INTERVAL_SECS`, default: 3600).
    pub scheduler_interval: Duration,
    /// Port for the Axum health/metrics HTTP server (`STITCHD_STATS_SERVICE_HTTP_PORT`, default: 9200).
    pub http_port: u16,
    /// Port for the gRPC server (`STITCHD_STATS_SERVICE_GRPC_PORT`, default: 50056).
    pub grpc_port: u16,
    /// gRPC endpoint for experimentation-service
    /// (`STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL`, default: `http://localhost:50055`).
    pub experimentation_service_grpc_url: String,
    /// gRPC endpoint for analytics-service
    /// (`STITCHD_ANALYTICS_SERVICE_GRPC_URL`, default: `http://localhost:50054`).
    pub analytics_service_grpc_url: String,
}

impl StatsConfig {
    /// Load configuration from environment variables.
    ///
    /// # Errors
    /// Returns an error if `STITCHD_DATABASE_URL` is not set.
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("STITCHD_DATABASE_URL").map_err(|_| {
            anyhow::anyhow!("STITCHD_DATABASE_URL environment variable is required")
        })?;

        let clickhouse_url = std::env::var("STITCHD_CLICKHOUSE_URL")
            .unwrap_or_else(|_| "http://localhost:8123".to_string());

        let clickhouse_db =
            std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());

        let scheduler_interval_secs: u64 = std::env::var("STITCHD_STATS_SCHEDULER_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let http_port: u16 = std::env::var("STITCHD_STATS_SERVICE_HTTP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9200);

        let grpc_port: u16 = std::env::var("STITCHD_STATS_SERVICE_GRPC_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50056);

        let experimentation_service_grpc_url =
            std::env::var("STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL")
                .unwrap_or_else(|_| "http://localhost:50055".to_string());

        let analytics_service_grpc_url = std::env::var("STITCHD_ANALYTICS_SERVICE_GRPC_URL")
            .unwrap_or_else(|_| "http://localhost:50054".to_string());

        Ok(Self {
            database_url,
            clickhouse_url,
            clickhouse_db,
            scheduler_interval: Duration::from_secs(scheduler_interval_secs),
            http_port,
            grpc_port,
            experimentation_service_grpc_url,
            analytics_service_grpc_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
            // SAFETY: serial ensures no other thread reads these vars concurrently.
            unsafe {
                match &self.original {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    #[serial]
    fn test_config_requires_database_url() {
        let _g = EnvGuard::new("STITCHD_DATABASE_URL");
        unsafe { std::env::remove_var("STITCHD_DATABASE_URL") };
        let result = StatsConfig::from_env();
        assert!(result.is_err(), "should fail without STITCHD_DATABASE_URL");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("STITCHD_DATABASE_URL"),
            "error message should mention STITCHD_DATABASE_URL"
        );
    }

    #[test]
    #[serial]
    fn test_config_loads_database_url() {
        let _g = EnvGuard::new("STITCHD_DATABASE_URL");
        unsafe {
            std::env::set_var(
                "STITCHD_DATABASE_URL",
                "postgresql://test:test@localhost/test",
            )
        };
        let config = StatsConfig::from_env().expect("should load with STITCHD_DATABASE_URL set");
        assert_eq!(config.database_url, "postgresql://test:test@localhost/test");
    }

    #[test]
    #[serial]
    fn test_config_defaults_clickhouse_url() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g_ch = EnvGuard::new("STITCHD_CLICKHOUSE_URL");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STITCHD_CLICKHOUSE_URL");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.clickhouse_url, "http://localhost:8123");
    }

    #[test]
    #[serial]
    fn test_config_loads_clickhouse_url() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g_ch = EnvGuard::new("STITCHD_CLICKHOUSE_URL");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::set_var("STITCHD_CLICKHOUSE_URL", "http://clickhouse:8123");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.clickhouse_url, "http://clickhouse:8123");
    }

    #[test]
    #[serial]
    fn test_config_defaults_scheduler_interval() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g_si = EnvGuard::new("STITCHD_STATS_SCHEDULER_INTERVAL_SECS");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STITCHD_STATS_SCHEDULER_INTERVAL_SECS");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.scheduler_interval.as_secs(), 3600);
    }

    #[test]
    #[serial]
    fn test_config_loads_scheduler_interval() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g_si = EnvGuard::new("STITCHD_STATS_SCHEDULER_INTERVAL_SECS");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::set_var("STITCHD_STATS_SCHEDULER_INTERVAL_SECS", "120");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.scheduler_interval.as_secs(), 120);
    }

    #[test]
    #[serial]
    fn test_config_defaults_http_port() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g_hp = EnvGuard::new("STITCHD_STATS_SERVICE_HTTP_PORT");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STITCHD_STATS_SERVICE_HTTP_PORT");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.http_port, 9200);
    }

    #[test]
    #[serial]
    fn test_config_defaults_experimentation_service_grpc_url() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g = EnvGuard::new("STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(
            config.experimentation_service_grpc_url,
            "http://localhost:50055"
        );
    }

    #[test]
    #[serial]
    fn test_config_loads_experimentation_service_grpc_url() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g = EnvGuard::new("STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::set_var(
                "STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL",
                "http://exp-svc:50054",
            );
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(
            config.experimentation_service_grpc_url,
            "http://exp-svc:50054"
        );
    }

    #[test]
    #[serial]
    fn test_config_defaults_analytics_service_grpc_url() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g = EnvGuard::new("STITCHD_ANALYTICS_SERVICE_GRPC_URL");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STITCHD_ANALYTICS_SERVICE_GRPC_URL");
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.analytics_service_grpc_url, "http://localhost:50054");
    }

    #[test]
    #[serial]
    fn test_config_loads_analytics_service_grpc_url() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g = EnvGuard::new("STITCHD_ANALYTICS_SERVICE_GRPC_URL");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::set_var(
                "STITCHD_ANALYTICS_SERVICE_GRPC_URL",
                "http://analytics:50055",
            );
        }
        let config = StatsConfig::from_env().unwrap();
        assert_eq!(config.analytics_service_grpc_url, "http://analytics:50055");
    }
}
