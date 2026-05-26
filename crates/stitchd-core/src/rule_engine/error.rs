use crate::id::FlagId;
use thiserror::Error;

/// Errors that can occur during rule engine evaluation.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum RuleEngineError {
    /// A condition was applied to a parameter value of the wrong type.
    #[error("type mismatch for parameter '{param}': expected {expected}, got {actual}")]
    TypeMismatch {
        param: String,
        expected: &'static str,
        actual: &'static str,
    },

    /// A required parameter was not present in the context.
    #[error("missing parameter '{param}'")]
    MissingParameter { param: String },

    /// No context with the given context_type was provided.
    #[error("missing context '{context_type}'")]
    MissingContext { context_type: String },

    /// A cyclic dependency was detected among flags.
    #[error("cyclic flag dependency detected, involved flags: {involved:?}")]
    CyclicFlagDependency { involved: Vec<FlagId> },

    /// Percentage weights do not sum to 10000 (basis points).
    #[error("percentage weights must sum to 10000 (basis points)")]
    InvalidWeights,

    /// A percentage rule has no targets to hash on.
    #[error("percentage allocation requires at least one target")]
    EmptyPercentageTargets,

    /// An internal or unexpected error occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn type_mismatch_formats_correctly() {
        let err = RuleEngineError::TypeMismatch {
            param: "age".to_string(),
            expected: "Int",
            actual: "Str",
        };
        assert_eq!(
            err.to_string(),
            "type mismatch for parameter 'age': expected Int, got Str"
        );
    }

    #[test]
    fn missing_parameter_formats_correctly() {
        let err = RuleEngineError::MissingParameter {
            param: "plan".to_string(),
        };
        assert_eq!(err.to_string(), "missing parameter 'plan'");
    }

    #[test]
    fn missing_context_formats_correctly() {
        let err = RuleEngineError::MissingContext {
            context_type: "device".to_string(),
        };
        assert_eq!(err.to_string(), "missing context 'device'");
    }

    #[test]
    fn cyclic_flag_dependency_formats_correctly() {
        let id = FlagId::from_uuid(Uuid::nil());
        let err = RuleEngineError::CyclicFlagDependency { involved: vec![id] };
        let s = err.to_string();
        assert!(s.contains("cyclic flag dependency"));
    }

    #[test]
    fn invalid_weights_formats_correctly() {
        let err = RuleEngineError::InvalidWeights;
        assert_eq!(
            err.to_string(),
            "percentage weights must sum to 10000 (basis points)"
        );
    }

    #[test]
    fn empty_percentage_targets_formats_correctly() {
        let err = RuleEngineError::EmptyPercentageTargets;
        assert_eq!(
            err.to_string(),
            "percentage allocation requires at least one target"
        );
    }
}
