use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum ParameterValue {
    Bool(bool),
    Int(i64),
    Double(f64),
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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

// ── Context registry domain types ────────────────────────────────────────────

/// Inferred value type for a context parameter key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InferredType {
    Str,
    Int,
    Double,
    Bool,
    SemVer,
    Unknown,
}

impl InferredType {
    /// Parse a string to the most specific type, in priority order:
    /// bool → int → double → semver → str.
    /// Returns `Unknown` when `value == "********"` (masked private param).
    #[must_use]
    pub fn infer(value: &str) -> Self {
        if value == "********" {
            return Self::Unknown;
        }
        if value == "true" || value == "false" {
            return Self::Bool;
        }
        if value.parse::<i64>().is_ok() {
            return Self::Int;
        }
        if value.parse::<f64>().is_ok() {
            return Self::Double;
        }
        if semver::Version::parse(value).is_ok() {
            return Self::SemVer;
        }
        Self::Str
    }

    /// Return the database string representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Str => "str",
            Self::Int => "int",
            Self::Double => "double",
            Self::Bool => "bool",
            Self::SemVer => "semver",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for InferredType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for InferredType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "str" => Ok(Self::Str),
            "int" => Ok(Self::Int),
            "double" => Ok(Self::Double),
            "bool" => Ok(Self::Bool),
            "semver" => Ok(Self::SemVer),
            "unknown" => Ok(Self::Unknown),
            other => Err(format!("unknown InferredType: {other}")),
        }
    }
}

/// A row in `context_type_registry`.
#[derive(Debug, Clone)]
pub struct ContextTypeRecord {
    pub env_id: crate::id::EnvironmentId,
    pub context_type: String,
    pub first_seen_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

/// A row in `context_param_registry`.
#[derive(Debug, Clone)]
pub struct ContextParamRecord {
    pub env_id: crate::id::EnvironmentId,
    pub context_type: String,
    pub param_key: String,
    pub inferred_type: InferredType,
    pub is_private: bool,
    pub first_seen_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_type_bool() {
        assert_eq!(InferredType::infer("true"), InferredType::Bool);
        assert_eq!(InferredType::infer("false"), InferredType::Bool);
    }

    #[test]
    fn test_infer_type_int() {
        assert_eq!(InferredType::infer("42"), InferredType::Int);
        assert_eq!(InferredType::infer("-7"), InferredType::Int);
        assert_eq!(InferredType::infer("0"), InferredType::Int);
    }

    #[test]
    fn test_infer_type_double() {
        assert_eq!(InferredType::infer("3.14"), InferredType::Double);
        assert_eq!(InferredType::infer("-0.5"), InferredType::Double);
    }

    #[test]
    fn test_infer_type_semver() {
        assert_eq!(InferredType::infer("1.2.3"), InferredType::SemVer);
        assert_eq!(InferredType::infer("0.0.1"), InferredType::SemVer);
    }

    #[test]
    fn test_infer_type_str() {
        assert_eq!(InferredType::infer("hello"), InferredType::Str);
        assert_eq!(InferredType::infer("user@example.com"), InferredType::Str);
        assert_eq!(InferredType::infer(""), InferredType::Str);
    }

    #[test]
    fn test_infer_type_private_param_is_unknown() {
        assert_eq!(InferredType::infer("********"), InferredType::Unknown);
    }

    #[test]
    fn test_infer_type_priority_bool_over_int() {
        // "true"/"false" must match Bool before the int parser sees them
        assert_eq!(InferredType::infer("true"), InferredType::Bool);
    }

    #[test]
    fn test_inferred_type_round_trip_from_str() {
        for ty in [
            InferredType::Str,
            InferredType::Int,
            InferredType::Double,
            InferredType::Bool,
            InferredType::SemVer,
            InferredType::Unknown,
        ] {
            let s = ty.as_str();
            let parsed: InferredType = s.parse().unwrap();
            assert_eq!(parsed, ty);
        }
    }

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
    fn test_parameter_value_display_all_variants() {
        assert_eq!(ParameterValue::Bool(true).to_string(), "true");
        assert_eq!(ParameterValue::Bool(false).to_string(), "false");
        assert_eq!(ParameterValue::Int(42).to_string(), "42");
        assert_eq!(ParameterValue::Double(2.5).to_string(), "2.5");
        assert_eq!(
            ParameterValue::SemVer(Version::parse("2.0.0").unwrap()).to_string(),
            "2.0.0"
        );
        assert_eq!(ParameterValue::Str("hello".into()).to_string(), "hello");
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
