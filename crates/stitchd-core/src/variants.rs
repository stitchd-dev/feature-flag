//! Type-safe variant values for feature flags.
//!
//! Every flag has a [`FlagValueType`] that defines what kind of data it returns.
//! Each variant of that flag must carry a [`VariantValue`] that matches that type.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::id::VariantId;

/// The type of value a feature flag produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagValueType {
    /// Boolean flag — `true` / `false`.
    Bool,
    /// 64-bit signed integer flag.
    Int,
    /// 64-bit IEEE 754 double flag.
    Double,
    /// UTF-8 string flag.
    Str,
    /// Arbitrary JSON flag (object, array, scalar).
    Json,
}

/// A concrete value carried by a variant.
///
/// The discriminant must match the parent flag's [`FlagValueType`].
/// Use [`VariantValue::matches_type`] to enforce this invariant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VariantValue {
    /// Boolean value.
    BoolValue(bool),
    /// Integer value.
    IntValue(i64),
    /// Double-precision floating-point value.
    DoubleValue(f64),
    /// String value.
    StrValue(String),
    /// Arbitrary JSON value.
    JsonValue(JsonValue),
}

impl VariantValue {
    /// Returns `true` if this value's type matches `flag_type`.
    #[must_use]
    pub fn matches_type(&self, flag_type: &FlagValueType) -> bool {
        matches!(
            (self, flag_type),
            (Self::BoolValue(_), FlagValueType::Bool)
                | (Self::IntValue(_), FlagValueType::Int)
                | (Self::DoubleValue(_), FlagValueType::Double)
                | (Self::StrValue(_), FlagValueType::Str)
                | (Self::JsonValue(_), FlagValueType::Json)
        )
    }

    /// Access the value as a boolean if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::BoolValue(v) => Some(*v),
            _ => None,
        }
    }

    /// Access the value as an i64 if it is an integer.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::IntValue(v) => Some(*v),
            _ => None,
        }
    }

    /// Access the value as an f64 if it is a double.
    pub fn as_double(&self) -> Option<f64> {
        match self {
            Self::DoubleValue(v) => Some(*v),
            _ => None,
        }
    }

    /// Access the value as a string slice if it is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::StrValue(v) => Some(v.as_str()),
            _ => None,
        }
    }

    /// Access the value as a JSON reference if it is JSON.
    pub fn as_json(&self) -> Option<&JsonValue> {
        match self {
            Self::JsonValue(v) => Some(v),
            _ => None,
        }
    }
}

/// A named variant of a feature flag.
///
/// Each variant belongs to exactly one flag and carries a value whose type
/// must match the parent flag's [`FlagValueType`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    /// Unique identifier for this variant.
    pub id: VariantId,
    /// Human-readable key (unique within the flag, e.g. `"control"`, `"treatment"`).
    pub key: String,
    /// The value returned when this variant is selected.
    pub value: VariantValue,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_matches_type() {
        assert!(VariantValue::BoolValue(true).matches_type(&FlagValueType::Bool));
        assert!(VariantValue::IntValue(42).matches_type(&FlagValueType::Int));
        assert!(VariantValue::DoubleValue(3.14).matches_type(&FlagValueType::Double));
        assert!(VariantValue::StrValue("hi".into()).matches_type(&FlagValueType::Str));
        assert!(VariantValue::JsonValue(json!({})).matches_type(&FlagValueType::Json));

        assert!(!VariantValue::BoolValue(true).matches_type(&FlagValueType::Int));
    }

    #[test]
    fn test_accessors() {
        let b = VariantValue::BoolValue(true);
        assert_eq!(b.as_bool(), Some(true));
        assert_eq!(b.as_int(), None);

        let i = VariantValue::IntValue(42);
        assert_eq!(i.as_int(), Some(42));
        assert_eq!(i.as_bool(), None);

        let d = VariantValue::DoubleValue(3.14);
        assert_eq!(d.as_double(), Some(3.14));
        assert_eq!(d.as_int(), None);

        let s = VariantValue::StrValue("hello".into());
        assert_eq!(s.as_str(), Some("hello"));
        assert_eq!(s.as_double(), None);

        let j = VariantValue::JsonValue(json!({"foo": "bar"}));
        assert_eq!(j.as_json(), Some(&json!({"foo": "bar"})));
        assert_eq!(j.as_str(), None);
    }
}
