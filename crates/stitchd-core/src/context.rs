use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParameterValue {
    Bool(bool),
    Int(i64),
    Double(f64),
    SemVer(Version),
    Str(String),
}

impl fmt::Display for ParameterValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{}", b),
            Self::Int(i) => write!(f, "{}", i),
            Self::Double(d) => write!(f, "{}", d),
            Self::SemVer(v) => write!(f, "{}", v),
            Self::Str(s) => write!(f, "{}", s),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Context {
    pub context_type: String,
    pub key: String,
    pub parameters: HashMap<String, ParameterValue>,
    pub private_parameters: HashSet<String>,
}

impl Context {
    pub fn new(context_type: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            context_type: context_type.into(),
            key: key.into(),
            parameters: HashMap::new(),
            private_parameters: HashSet::new(),
        }
    }

    pub fn with_parameter(mut self, name: impl Into<String>, value: ParameterValue) -> Self {
        self.parameters.insert(name.into(), value);
        self
    }

    pub fn with_private_parameter(mut self, name: impl Into<String>) -> Self {
        self.private_parameters.insert(name.into());
        self
    }

    pub fn is_private(&self, param_name: &str) -> bool {
        self.private_parameters.contains(param_name)
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("Context");
        debug_struct.field("context_type", &self.context_type);
        debug_struct.field("key", &self.key);

        let mut masked_params = HashMap::new();
        for (k, v) in &self.parameters {
            if self.is_private(k) {
                masked_params.insert(k, "[REDACTED]".to_string());
            } else {
                masked_params.insert(k, format!("{:?}", v));
            }
        }
        debug_struct.field("parameters", &masked_params);
        debug_struct.field("private_parameters", &self.private_parameters);
        debug_struct.finish()
    }
}

/// A collection of contexts used for a single evaluation.
///
/// Typical contexts include 'user', 'session', 'application', etc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvaluationContext {
    pub contexts: Vec<Context>,
}

impl EvaluationContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_context(mut self, context: Context) -> Self {
        self.contexts.push(context);
        self
    }

    pub fn get_context(&self, context_type: &str) -> Option<&Context> {
        self.contexts
            .iter()
            .find(|c| c.context_type == context_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_privacy() {
        let context = Context::new("user", "user-1")
            .with_parameter("email", ParameterValue::Str("user@example.com".to_string()))
            .with_parameter("age", ParameterValue::Int(30))
            .with_private_parameter("email");

        assert!(context.is_private("email"));
        assert!(!context.is_private("age"));

        let debug_output = format!("{:?}", context);
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("user@example.com"));
        assert!(debug_output.contains("30"));
    }

    #[test]
    fn test_parameter_value_serialization() {
        let val = ParameterValue::SemVer(Version::parse("1.2.3").unwrap());
        let json = serde_json::to_string(&val).unwrap();
        assert_eq!(json, "\"1.2.3\"");

        let deserialized: ParameterValue = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, val);
    }

    #[test]
    fn test_evaluation_context() {
        let ctx1 = Context::new("user", "u1");
        let ctx2 = Context::new("session", "s1");
        let eval_ctx = EvaluationContext::new()
            .with_context(ctx1)
            .with_context(ctx2);

        assert!(eval_ctx.get_context("user").is_some());
        assert!(eval_ctx.get_context("session").is_some());
        assert!(eval_ctx.get_context("other").is_none());
    }
}
