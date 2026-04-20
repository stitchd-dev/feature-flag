//! ClickHouse query layer for experiment statistical analysis.
//!
//! This module provides aggregation query functions that read from the
//! ClickHouse event tables populated by the `stitchd-events` crate.

pub mod experiment_queries;

pub use experiment_queries::{
    CountMetricRow, FunnelStepRow, NumericMetricRow, QueryError, query_count_metric,
    query_funnel, query_numeric_metric,
};
