//! Environment-based configuration for `stitchd-schedule-service`.

use std::time::Duration;

/// Runtime configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct ScheduleConfig {
    /// PostgreSQL connection URL (`STITCHD_DATABASE_URL`).
    pub database_url: String,
    /// How often the scheduler tick fires
    /// (`STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS`, default: 60).
    pub scheduler_interval: Duration,
    /// Max due changes claimed per tick
    /// (`STITCHD_SCHEDULE_CLAIM_BATCH`, default: 100).
    pub claim_batch: i64,
    /// Port for the Axum health/metrics HTTP server
    /// (`STITCHD_SCHEDULE_SERVICE_HTTP_PORT`, default: 9201).
    pub http_port: u16,
    /// Port for the gRPC server
    /// (`STITCHD_SCHEDULE_SERVICE_GRPC_PORT`, default: 50057).
    pub grpc_port: u16,
    /// gRPC endpoint for flag-service
    /// (`STITCHD_FLAG_SERVICE_GRPC_URL`, default: `http://localhost:50051`).
    pub flag_service_grpc_url: String,
    /// gRPC endpoint for experimentation-service
    /// (`STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL`, default: `http://localhost:50055`).
    pub experimentation_service_grpc_url: String,
    /// gRPC endpoint for segmentation-service
    /// (`STITCHD_SEGMENTATION_SERVICE_GRPC_URL`, default: `http://localhost:50053`).
    pub segmentation_service_grpc_url: String,
}

impl ScheduleConfig {
    /// Load configuration from environment variables.
    ///
    /// # Errors
    /// Returns an error if `STITCHD_DATABASE_URL` is not set.
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("STITCHD_DATABASE_URL").map_err(|_| {
            anyhow::anyhow!("STITCHD_DATABASE_URL environment variable is required")
        })?;

        let scheduler_interval_secs: u64 =
            std::env::var("STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60);

        let claim_batch: i64 = std::env::var("STITCHD_SCHEDULE_CLAIM_BATCH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        let http_port: u16 = std::env::var("STITCHD_SCHEDULE_SERVICE_HTTP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9201);

        let grpc_port: u16 = std::env::var("STITCHD_SCHEDULE_SERVICE_GRPC_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50057);

        let flag_service_grpc_url = std::env::var("STITCHD_FLAG_SERVICE_GRPC_URL")
            .unwrap_or_else(|_| "http://localhost:50051".to_string());

        let experimentation_service_grpc_url =
            std::env::var("STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL")
                .unwrap_or_else(|_| "http://localhost:50055".to_string());

        let segmentation_service_grpc_url = std::env::var("STITCHD_SEGMENTATION_SERVICE_GRPC_URL")
            .unwrap_or_else(|_| "http://localhost:50053".to_string());

        Ok(Self {
            database_url,
            scheduler_interval: Duration::from_secs(scheduler_interval_secs),
            claim_batch,
            http_port,
            grpc_port,
            flag_service_grpc_url,
            experimentation_service_grpc_url,
            segmentation_service_grpc_url,
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
    fn requires_database_url() {
        let _g = EnvGuard::new("STITCHD_DATABASE_URL");
        unsafe { std::env::remove_var("STITCHD_DATABASE_URL") };
        let result = ScheduleConfig::from_env();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("STITCHD_DATABASE_URL")
        );
    }

    #[test]
    #[serial]
    fn defaults_are_applied() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g_si = EnvGuard::new("STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS");
        let _g_hp = EnvGuard::new("STITCHD_SCHEDULE_SERVICE_HTTP_PORT");
        let _g_gp = EnvGuard::new("STITCHD_SCHEDULE_SERVICE_GRPC_PORT");
        let _g_fs = EnvGuard::new("STITCHD_FLAG_SERVICE_GRPC_URL");
        let _g_es = EnvGuard::new("STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL");
        let _g_ss = EnvGuard::new("STITCHD_SEGMENTATION_SERVICE_GRPC_URL");
        let _g_cb = EnvGuard::new("STITCHD_SCHEDULE_CLAIM_BATCH");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::remove_var("STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS");
            std::env::remove_var("STITCHD_SCHEDULE_SERVICE_HTTP_PORT");
            std::env::remove_var("STITCHD_SCHEDULE_SERVICE_GRPC_PORT");
            std::env::remove_var("STITCHD_FLAG_SERVICE_GRPC_URL");
            std::env::remove_var("STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL");
            std::env::remove_var("STITCHD_SEGMENTATION_SERVICE_GRPC_URL");
            std::env::remove_var("STITCHD_SCHEDULE_CLAIM_BATCH");
        }
        let config = ScheduleConfig::from_env().unwrap();
        assert_eq!(config.scheduler_interval.as_secs(), 60);
        assert_eq!(config.http_port, 9201);
        assert_eq!(config.grpc_port, 50057);
        assert_eq!(config.claim_batch, 100);
        assert_eq!(config.flag_service_grpc_url, "http://localhost:50051");
        assert_eq!(
            config.experimentation_service_grpc_url,
            "http://localhost:50055"
        );
        assert_eq!(
            config.segmentation_service_grpc_url,
            "http://localhost:50053"
        );
    }

    #[test]
    #[serial]
    fn overrides_are_read() {
        let _g_db = EnvGuard::new("STITCHD_DATABASE_URL");
        let _g_gp = EnvGuard::new("STITCHD_SCHEDULE_SERVICE_GRPC_PORT");
        let _g_fs = EnvGuard::new("STITCHD_FLAG_SERVICE_GRPC_URL");
        unsafe {
            std::env::set_var("STITCHD_DATABASE_URL", "postgresql://x:x@localhost/x");
            std::env::set_var("STITCHD_SCHEDULE_SERVICE_GRPC_PORT", "60000");
            std::env::set_var("STITCHD_FLAG_SERVICE_GRPC_URL", "http://flag:50051");
        }
        let config = ScheduleConfig::from_env().unwrap();
        assert_eq!(config.grpc_port, 60000);
        assert_eq!(config.flag_service_grpc_url, "http://flag:50051");
    }
}
