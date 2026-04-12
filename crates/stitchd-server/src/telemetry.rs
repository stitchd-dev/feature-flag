//! Observability initialisation: tracing, OpenTelemetry, and Prometheus metrics.
//!
//! Call [`init_tracing`] and [`init_metrics`] once at startup before any other
//! instrumented code runs.  Call [`shutdown_tracing`] on graceful exit to flush
//! pending spans.

use anyhow::{Context, Result};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{RandomIdGenerator, Sampler, SdkTracerProvider};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

/// Initialise the Prometheus metrics recorder.
///
/// Must be called before any `metrics::*` macros are used.
/// Returns a handle whose [`PrometheusHandle::render`] method produces the
/// `/metrics` scrape payload.
///
/// # Errors
/// Returns an error if a global recorder has already been installed.
pub fn init_metrics() -> Result<PrometheusHandle> {
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install Prometheus metrics recorder")?;
    Ok(handle)
}

/// Initialise the tracing subscriber and OpenTelemetry tracer provider.
///
/// - Log format: JSON when `APP_ENV=production`, pretty otherwise.
/// - Log level:  driven by the `RUST_LOG` environment variable.
/// - OTLP export: disabled in this scaffold — wire up `opentelemetry-otlp`
///   once `OTEL_EXPORTER_OTLP_ENDPOINT` configuration is needed at runtime.
///
/// Returns the [`SdkTracerProvider`] so the caller can shut it down cleanly.
///
/// # Errors
/// Returns an error if subscriber initialisation fails.
pub fn init_tracing(service_name: &str, _service_version: &str) -> Result<SdkTracerProvider> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // TODO: attach Resource with service.name / service.version attributes once
    // opentelemetry_sdk 0.28 builder API is confirmed (Resource::new is private in 0.28).
    // TODO: wire OTLP span exporter when `OTEL_EXPORTER_OTLP_ENDPOINT` is set.
    // For now, spans are recorded in-process but not exported.
    let tracer_provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_id_generator(RandomIdGenerator::default())
        .build();

    let otel_layer =
        tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer(service_name.to_owned()));

    let is_production = std::env::var("APP_ENV")
        .map(|v| v.eq_ignore_ascii_case("production"))
        .unwrap_or(false);

    if is_production {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(otel_layer)
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()
            .context("failed to initialise JSON tracing subscriber")?;
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(otel_layer)
            .with(tracing_subscriber::fmt::layer().pretty())
            .try_init()
            .context("failed to initialise pretty tracing subscriber")?;
    }

    Ok(tracer_provider)
}

/// Flush and shut down the OpenTelemetry tracer provider.
///
/// Call this before the process exits to ensure all in-flight spans are flushed.
pub fn shutdown_tracing(provider: &SdkTracerProvider) {
    if let Err(e) = provider.shutdown() {
        // Shutdown errors are non-fatal — log and continue.
        eprintln!("OTEL tracer shutdown error: {e}");
    }
}
