//! Entry point for `stitchd-gateway`.
//!
//! Environment variables:
//! - `GATEWAY_PORT` (default: `8080`) — REST listen port
//! - `GATEWAY_GRPC_PORT` (default: `50050`) — SDK gRPC listen port
//! - `METRICS_PORT` (default: `9080`) — Prometheus metrics port
//! - `AUTH_SERVICE_ADDR` (default: `http://localhost:50051`)
//! - `FLAG_SERVICE_ADDR` (default: `http://localhost:50052`)
//! - `SEGMENTATION_SERVICE_ADDR` (default: `http://localhost:50053`)
//! - `ANALYTICS_SERVICE_ADDR` (default: `http://localhost:50054`)
//! - `EXPERIMENTATION_SERVICE_ADDR` (default: `http://localhost:50055`)
//! - `STATS_SERVICE_ADDR` (default: `http://localhost:50056`)

use std::{net::SocketAddr, sync::Arc};

use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tracing::info;

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

    let metrics_port: u16 = env_or("METRICS_PORT", "9080").parse().unwrap_or(9080);
    let metrics_addr: SocketAddr = format!("0.0.0.0:{metrics_port}").parse()?;
    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()?;
    info!(%metrics_addr, "prometheus metrics ready");

    let state = GatewayState::connect(
        env_or("AUTH_SERVICE_ADDR", "http://localhost:50051"),
        env_or("FLAG_SERVICE_ADDR", "http://localhost:50052"),
        env_or("SEGMENTATION_SERVICE_ADDR", "http://localhost:50053"),
        env_or("ANALYTICS_SERVICE_ADDR", "http://localhost:50054"),
        env_or("EXPERIMENTATION_SERVICE_ADDR", "http://localhost:50055"),
        env_or("STATS_SERVICE_ADDR", "http://localhost:50056"),
    )
    .await?;

    let gateway_port: u16 = env_or("GATEWAY_PORT", "8080").parse().unwrap_or(8080);
    let addr: SocketAddr = format!("0.0.0.0:{gateway_port}").parse()?;

    let grpc_port: u16 = env_or("GATEWAY_GRPC_PORT", "50050")
        .parse()
        .unwrap_or(50050);
    let grpc_addr: SocketAddr = format!("0.0.0.0:{grpc_port}").parse()?;

    // Capture the two SDK backend clients BEFORE we move `state` into the
    // Axum router — both servers need them.
    let state_arc = Arc::new(state);
    let auth_client_for_grpc = Arc::clone(&state_arc.auth_client);
    let flag_sdk_backend_for_grpc = Arc::clone(&state_arc.flag_sdk_backend_client);

    let app = build_router(Arc::clone(&state_arc));

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
