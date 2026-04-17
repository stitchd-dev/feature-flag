use stitchd_core::segment::SegmentEvaluatorError;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("SDK initialization failed: {0}")]
    InitFailed(String),

    #[error("gRPC transport error: {0}")]
    GrpcTransport(String),

    #[error("gRPC status: {0}")]
    GrpcStatus(Box<tonic::Status>),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("deserialization error: {0}")]
    Deserialization(String),

    #[error("evaluation error: {0}")]
    Evaluation(String),

    #[error("segment evaluation error: {0}")]
    SegmentEval(#[from] SegmentEvaluatorError),

    #[error("rule engine error: {0}")]
    RuleEngine(#[from] stitchd_core::rule_engine::RuleEngineError),
}

impl From<tonic::Status> for SdkError {
    fn from(s: tonic::Status) -> Self {
        SdkError::GrpcStatus(Box::new(s))
    }
}
