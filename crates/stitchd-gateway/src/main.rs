//! Entry point for `stitchd-gateway`.
//!
//! Environment variables:
//! - `STITCHD_GATEWAY_HTTP_PORT` (default: `8080`) — REST listen port
//! - `STITCHD_GATEWAY_GRPC_PORT` (default: `50050`) — SDK gRPC listen port
//! - `STITCHD_AUTH_SERVICE_ADDR` (default: `http://localhost:50051`)
//! - `STITCHD_FLAG_SERVICE_ADDR` (default: `http://localhost:50052`)
//! - `STITCHD_SEGMENTATION_SERVICE_ADDR` (default: `http://localhost:50053`)
//! - `STITCHD_ANALYTICS_SERVICE_ADDR` (default: `http://localhost:50054`)
//! - `STITCHD_EXPERIMENTATION_SERVICE_ADDR` (default: `http://localhost:50055`)
//! - `STITCHD_STATS_SERVICE_ADDR` (default: `http://localhost:50056`)
//! - `STITCHD_EVENT_QUOTA_PER_SEC` (default: `1000`) — per-env quota for
//!   `POST /v1/events/track`; requests above the cap return 429.
//!
//! Prometheus metrics are served at `GET /metrics` on the same port as the
//! REST API (Prometheus text exposition format, `text/plain; version=0.0.4`).
//! `/v1/metrics` is reserved for the metric-definitions admin REST surface
//! (CRUD + preview) under JWT auth.

use std::{net::SocketAddr, sync::Arc};

use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tracing::info;

use stitchd_core::util::grpc_connect::connect_with_retry_default;
use stitchd_gateway::{
    grpc_server::build_grpc_server, openapi::export_to_file, router::build_router,
    state::GatewayState,
};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Handle `--export-openapi <path>` before any server setup.
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--export-openapi") {
        let path = args.get(pos + 1).map(String::as_str).unwrap_or_else(|| {
            eprintln!("error: --export-openapi requires a path argument");
            std::process::exit(1);
        });
        export_to_file(path);
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Install Prometheus recorder and obtain a handle for the /metrics handler.
    // The handle is passed into the router so the GET /metrics route can call
    // handle.render() to return real Prometheus exposition text on the main port.
    let metrics_handle = PrometheusBuilder::new().install_recorder()?;
    // Emit a startup counter so the exposition is never completely empty even
    // before request-level metrics are recorded.
    metrics::counter!("gateway_up_total").increment(1);
    info!("prometheus metrics recorder installed (served at GET /metrics)");

    let auth_addr = env_or("STITCHD_AUTH_SERVICE_ADDR", "http://localhost:50051");
    let flag_addr = env_or("STITCHD_FLAG_SERVICE_ADDR", "http://localhost:50052");
    let segmentation_addr = env_or(
        "STITCHD_SEGMENTATION_SERVICE_ADDR",
        "http://localhost:50053",
    );
    let analytics_addr = env_or("STITCHD_ANALYTICS_SERVICE_ADDR", "http://localhost:50054");
    let experimentation_addr = env_or(
        "STITCHD_EXPERIMENTATION_SERVICE_ADDR",
        "http://localhost:50055",
    );
    let stats_addr = env_or("STITCHD_STATS_SERVICE_ADDR", "http://localhost:50056");
    let schedule_addr = env_or("STITCHD_SCHEDULE_SERVICE_ADDR", "http://localhost:50057");

    // `GatewayState::connect` dials all six downstream services in sequence
    // and fails fast on the first transport error. During a parallel cold
    // boot any one of those peers may not yet be listening, so we wrap the
    // whole sequence in `connect_with_retry_default` — when any single
    // dependency is slow to bind, the helper retries the full sequence
    // until every peer is reachable (or the ~60s budget is exhausted).
    let downstream_summary = format!(
        "auth={auth_addr} flag={flag_addr} seg={segmentation_addr} analytics={analytics_addr} experimentation={experimentation_addr} stats={stats_addr} schedule={schedule_addr}"
    );
    let state =
        connect_with_retry_default("gateway downstream services", &downstream_summary, || {
            let auth_addr = auth_addr.clone();
            let flag_addr = flag_addr.clone();
            let segmentation_addr = segmentation_addr.clone();
            let analytics_addr = analytics_addr.clone();
            let experimentation_addr = experimentation_addr.clone();
            let stats_addr = stats_addr.clone();
            let schedule_addr = schedule_addr.clone();
            async move {
                GatewayState::connect(
                    auth_addr,
                    flag_addr,
                    segmentation_addr,
                    analytics_addr,
                    experimentation_addr,
                    stats_addr,
                    schedule_addr,
                )
                .await
            }
        })
        .await?;

    let gateway_port: u16 = env_or("STITCHD_GATEWAY_HTTP_PORT", "8080")
        .parse()
        .unwrap_or(8080);
    let addr: SocketAddr = format!("0.0.0.0:{gateway_port}").parse()?;

    let grpc_port: u16 = env_or("STITCHD_GATEWAY_GRPC_PORT", "50050")
        .parse()
        .unwrap_or(50050);
    let grpc_addr: SocketAddr = format!("0.0.0.0:{grpc_port}").parse()?;

    // Capture the two SDK backend clients BEFORE we move `state` into the
    // Axum router — both servers need them.
    let state_arc = Arc::new(state);
    let auth_client_for_grpc = Arc::clone(&state_arc.auth_client);
    let flag_sdk_backend_for_grpc = Arc::clone(&state_arc.flag_sdk_backend_client);

    let mut app = build_router(Arc::clone(&state_arc), metrics_handle);

    // ── Idempotency-Key middleware (platform_hardening_20260608) ────────────
    // HTTP-edge request dedup backed by a narrowly-scoped PgPool. Opt-in by
    // deployment: only enabled when STITCHD_DATABASE_URL is set (the gateway is
    // otherwise DB-free). Layered OUTSIDE the router so it applies to every
    // mutating route; it self-filters to POST/PUT/PATCH/DELETE carrying an
    // `Idempotency-Key` header and fails open on any store error.
    if let Ok(database_url) = std::env::var("STITCHD_DATABASE_URL") {
        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
        {
            Ok(pool) => {
                let ttl = stitchd_gateway::idempotency::ttl_from_env();
                stitchd_gateway::idempotency::spawn_sweeper(pool.clone(), ttl);
                let store: Arc<dyn stitchd_gateway::idempotency::IdempotencyStore> =
                    Arc::new(stitchd_gateway::idempotency::PgIdempotencyStore::new(pool));
                app = app.layer(axum::middleware::from_fn_with_state(
                    store,
                    stitchd_gateway::idempotency::idempotency_middleware,
                ));
                info!(ttl_secs = ttl.as_secs(), "idempotency middleware enabled");
            }
            Err(e) => {
                tracing::warn!(
                    "STITCHD_DATABASE_URL set but Postgres connect failed; \
                     idempotency middleware DISABLED: {e}"
                );
            }
        }
    } else {
        info!("STITCHD_DATABASE_URL unset — idempotency middleware disabled");
    }

    info!(%addr, %grpc_addr, "stitchd-gateway starting (REST + SDK gRPC)");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let rest_server = axum::serve(listener, app).with_graceful_shutdown(async {
        signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
        info!("REST shutdown signal received");
    });

    let grpc_router = build_grpc_server(auth_client_for_grpc, flag_sdk_backend_for_grpc);
    let grpc_server = grpc_router.serve_with_shutdown(grpc_addr, async {
        signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
        info!("gRPC shutdown signal received");
    });

    // Run both servers concurrently; either one's error or shutdown ends the process.
    let (rest_result, grpc_result) = tokio::join!(rest_server, grpc_server);
    rest_result?;
    grpc_result?;

    Ok(())
}
