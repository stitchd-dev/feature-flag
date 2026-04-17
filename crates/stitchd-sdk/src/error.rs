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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_failed_displays_message() {
        let e = SdkError::InitFailed("bad key".to_string());
        assert_eq!(e.to_string(), "SDK initialization failed: bad key");
    }

    #[test]
    fn grpc_transport_displays_message() {
        let e = SdkError::GrpcTransport("connection refused".to_string());
        assert_eq!(e.to_string(), "gRPC transport error: connection refused");
    }

    #[test]
    fn grpc_status_from_tonic_status() {
        let status = tonic::Status::not_found("flag not found");
        let e: SdkError = status.into();
        let msg = e.to_string();
        assert!(msg.contains("gRPC status"));
        assert!(msg.contains("flag not found"));
    }

    #[test]
    fn deserialization_error_displays_message() {
        let e = SdkError::Deserialization("invalid JSON".to_string());
        assert_eq!(e.to_string(), "deserialization error: invalid JSON");
    }

    #[test]
    fn evaluation_error_displays_message() {
        let e = SdkError::Evaluation("divide by zero".to_string());
        assert_eq!(e.to_string(), "evaluation error: divide by zero");
    }

    #[test]
    fn segment_eval_from_segment_evaluator_error() {
        use stitchd_core::segment::SegmentEvaluatorError;
        let inner = SegmentEvaluatorError::InvalidSegmentRule;
        let e: SdkError = inner.into();
        let msg = e.to_string();
        assert!(msg.contains("segment evaluation error"));
    }

    #[test]
    fn rule_engine_from_rule_engine_error() {
        use stitchd_core::rule_engine::RuleEngineError;
        let inner = RuleEngineError::MissingParameter {
            param: "user_id".to_string(),
        };
        let e: SdkError = inner.into();
        let msg = e.to_string();
        assert!(msg.contains("rule engine error"));
    }

    #[test]
    fn grpc_status_variant_debug() {
        let status = tonic::Status::internal("server error");
        let e = SdkError::GrpcStatus(Box::new(status));
        let debug_str = format!("{e:?}");
        assert!(debug_str.contains("GrpcStatus"));
    }
}
