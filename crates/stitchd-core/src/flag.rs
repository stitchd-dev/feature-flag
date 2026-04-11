//! Flag types: `FlagValueType`, `VariantValue`, and `Variant`.
//!
//! These types model the typed nature of feature flags and their variants.
//! A flag declares a `FlagValueType`; every `Variant` belonging to that flag
//! must carry a `VariantValue` whose discriminant matches the flag's type.

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
    ///
    /// # Examples
    ///
    /// ```
    /// use stitchd_core::flag::{FlagValueType, VariantValue};
    ///
    /// assert!(VariantValue::BoolValue(true).matches_type(&FlagValueType::Bool));
    /// assert!(!VariantValue::IntValue(42).matches_type(&FlagValueType::Bool));
    /// ```
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

    // --- matches_type: correct matches ---

    #[test]
    fn bool_value_matches_bool_type() {
        assert!(VariantValue::BoolValue(true).matches_type(&FlagValueType::Bool));
        assert!(VariantValue::BoolValue(false).matches_type(&FlagValueType::Bool));
    }

    #[test]
    fn int_value_matches_int_type() {
        assert!(VariantValue::IntValue(0).matches_type(&FlagValueType::Int));
        assert!(VariantValue::IntValue(-999).matches_type(&FlagValueType::Int));
    }

    #[test]
    fn double_value_matches_double_type() {
        assert!(VariantValue::DoubleValue(3.14).matches_type(&FlagValueType::Double));
    }

    #[test]
    fn str_value_matches_str_type() {
        assert!(VariantValue::StrValue("hello".into()).matches_type(&FlagValueType::Str));
    }

    #[test]
    fn json_value_matches_json_type() {
        assert!(VariantValue::JsonValue(json!({"k": "v"})).matches_type(&FlagValueType::Json));
    }

    // --- matches_type: cross-type mismatches ---

    #[test]
    fn bool_value_does_not_match_other_types() {
        let v = VariantValue::BoolValue(true);
        assert!(!v.matches_type(&FlagValueType::Int));
        assert!(!v.matches_type(&FlagValueType::Double));
        assert!(!v.matches_type(&FlagValueType::Str));
        assert!(!v.matches_type(&FlagValueType::Json));
    }

    #[test]
    fn int_value_does_not_match_other_types() {
        let v = VariantValue::IntValue(1);
        assert!(!v.matches_type(&FlagValueType::Bool));
        assert!(!v.matches_type(&FlagValueType::Double));
        assert!(!v.matches_type(&FlagValueType::Str));
        assert!(!v.matches_type(&FlagValueType::Json));
    }

    #[test]
    fn double_value_does_not_match_other_types() {
        let v = VariantValue::DoubleValue(1.0);
        assert!(!v.matches_type(&FlagValueType::Bool));
        assert!(!v.matches_type(&FlagValueType::Int));
        assert!(!v.matches_type(&FlagValueType::Str));
        assert!(!v.matches_type(&FlagValueType::Json));
    }

    #[test]
    fn str_value_does_not_match_other_types() {
        let v = VariantValue::StrValue("x".into());
        assert!(!v.matches_type(&FlagValueType::Bool));
        assert!(!v.matches_type(&FlagValueType::Int));
        assert!(!v.matches_type(&FlagValueType::Double));
        assert!(!v.matches_type(&FlagValueType::Json));
    }

    #[test]
    fn json_value_does_not_match_other_types() {
        let v = VariantValue::JsonValue(json!(42));
        assert!(!v.matches_type(&FlagValueType::Bool));
        assert!(!v.matches_type(&FlagValueType::Int));
        assert!(!v.matches_type(&FlagValueType::Double));
        assert!(!v.matches_type(&FlagValueType::Str));
    }

    // --- Variant construction ---

    #[test]
    fn variant_roundtrips_through_serde() {
        let id = VariantId::new();
        let variant = Variant {
            id,
            key: "control".to_string(),
            value: VariantValue::BoolValue(false),
        };
        let json = serde_json::to_string(&variant).unwrap();
        let back: Variant = serde_json::from_str(&json).unwrap();
        assert_eq!(back.key, "control");
        assert_eq!(back.value, VariantValue::BoolValue(false));
    }
}
