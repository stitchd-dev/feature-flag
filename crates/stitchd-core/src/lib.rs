//! Core domain types, rule engine, and shared models for Stitchd.
//!
//! This crate contains no infrastructure concerns — only pure domain logic.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

// TODO: domain types (context, parameter values, feature flags, segments, variants)
// TODO: rule engine (core rule evaluation, operators, NOT/AND combinators)
// TODO: percentage allocation (deterministic hashing)

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
