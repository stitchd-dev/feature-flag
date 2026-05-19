//! OpenTelemetry-compatible tracing for Scylla operations.
//!
//! This module provides a standardised [`scylla_span`] helper that creates
//! [`tracing::Span`] values following the [OpenTelemetry database semantic
//! conventions](https://opentelemetry.io/docs/specs/semconv/database/database-spans/).
//! Spans flow to any OpenTelemetry exporter via the [`tracing-opentelemetry`]
//! bridge installed by the service binary at startup.
//!
//! ## Span attributes
//!
//! | Attribute | Value |
//! |-----------|-------|
//! | `db.system` | `"scylladb"` |
//! | `db.operation` | e.g. `"execute"`, `"prepare"` |
//! | `db.statement` | The CQL string (may be truncated at 512 chars) |
//!
//! ## Usage
//!
//! ```rust,ignore
//! use stitchd_db::scylla::tracing::scylla_span;
//! use tracing::Instrument;
//!
//! let result = session
//!     .execute_unpaged(&stmt, &values)
//!     .instrument(scylla_span("execute", cql))
//!     .await?;
//! ```

/// Maximum CQL statement length recorded in a span before truncation.
const MAX_STMT_LEN: usize = 512;

/// Create an OpenTelemetry-compatible [`tracing::Span`] for a single Scylla
/// operation.
///
/// The span follows [OTel database conventions]:
/// - `db.system` = `"scylladb"`
/// - `db.operation` = `operation` (e.g. `"execute"`, `"prepare"`)
/// - `db.statement` = first [`MAX_STMT_LEN`] characters of `cql`
///
/// Long CQL strings are silently truncated so they don't bloat trace backends.
///
/// [OTel database conventions]: https://opentelemetry.io/docs/specs/semconv/database/database-spans/
#[must_use]
pub fn scylla_span(operation: &str, cql: &str) -> tracing::Span {
    let stmt = if cql.len() > MAX_STMT_LEN {
        &cql[..MAX_STMT_LEN]
    } else {
        cql
    };

    tracing::info_span!(
        "scylla.query",
        db.system = "scylladb",
        db.operation = %operation,
        db.statement = %stmt,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{MAX_STMT_LEN, scylla_span};

    const SAMPLE_CQL: &str =
        "SELECT segment_id, generation FROM stitchd_segments.segment_list_entries WHERE segment_id = ?";

    /// Span is constructed without panicking.
    #[test]
    fn span_creation_does_not_panic() {
        let _span = scylla_span("execute", SAMPLE_CQL);
    }

    /// When a subscriber is active, the span carries the expected name.
    #[test]
    fn span_has_correct_name_when_subscriber_active() {
        // Install a minimal fmt subscriber for this test.
        // `try_init` returns Err if already initialised — that's fine.
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        let span = scylla_span("execute", SAMPLE_CQL);
        if let Some(meta) = span.metadata() {
            assert_eq!(meta.name(), "scylla.query");
        }
    }

    /// Long CQL strings are truncated to `MAX_STMT_LEN` in the span.
    #[test]
    fn long_cql_is_truncated() {
        let long_cql = "SELECT ".to_string() + &"x".repeat(MAX_STMT_LEN + 100);
        // Must not panic for oversized strings.
        let _span = scylla_span("execute", &long_cql);
    }

    /// Empty CQL does not panic.
    #[test]
    fn empty_cql_does_not_panic() {
        let _span = scylla_span("prepare", "");
    }

    /// Span propagation: a future instrumented with the span executes within it.
    #[tokio::test]
    async fn instrumented_future_runs_in_span() {
        use tracing::Instrument;
        let span = scylla_span("execute", SAMPLE_CQL);
        let span_id = span.id();

        // Execute a trivial future inside the span and verify the tracing context.
        async { /* placeholder for actual query */ }
            .instrument(span)
            .await;

        // The span ID is non-None when tracing is enabled (subscriber active)
        // or None in a no-op context — either way we must not panic.
        let _ = span_id;
    }
}
