//! ClickHouse query layer — experiment analysis and evaluation telemetry.

pub mod eval_log;
pub mod experiment_queries;

pub use eval_log::{EvalLogRow, insert_eval_log_rows};
pub use experiment_queries::{
    CountMetricRow, FunnelStepRow, NumericMetricRow, QueryError, query_count_metric, query_funnel,
    query_numeric_metric,
};
