use std::net::SocketAddr;

pub struct Config {
    pub grpc_addr: SocketAddr,
    pub metrics_addr: SocketAddr,
    pub database_url: String,
    pub clickhouse_url: String,
    pub clickhouse_db: String,
    pub clickhouse_user: Option<String>,
    pub clickhouse_password: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let grpc_port: u16 = std::env::var("STITCHD_ANALYTICS_SERVICE_GRPC_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50054);

        let metrics_port: u16 = std::env::var("STITCHD_ANALYTICS_SERVICE_METRICS_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9104);

        let database_url = std::env::var("STITCHD_DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("STITCHD_DATABASE_URL environment variable is required"))?;

        let clickhouse_url = std::env::var("STITCHD_CLICKHOUSE_URL")
            .unwrap_or_else(|_| "http://localhost:8123".to_string());

        let clickhouse_db = std::env::var("STITCHD_CLICKHOUSE_DB")
            .unwrap_or_else(|_| "stitchd".to_string());

        let clickhouse_user = std::env::var("STITCHD_CLICKHOUSE_USER").ok();
        let clickhouse_password = std::env::var("STITCHD_CLICKHOUSE_PASSWORD").ok();

        Ok(Self {
            grpc_addr: SocketAddr::from(([0, 0, 0, 0], grpc_port)),
            metrics_addr: SocketAddr::from(([0, 0, 0, 0], metrics_port)),
            database_url,
            clickhouse_url,
            clickhouse_db,
            clickhouse_user,
            clickhouse_password,
        })
    }
}
