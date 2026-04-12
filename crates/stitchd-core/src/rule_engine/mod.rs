pub mod condition;
pub mod dependency;
pub mod error;
pub mod eval_expr;
pub mod eval_leaf;
pub mod eval_rules;
pub mod orchestrator;
pub mod percentage;
pub mod types;

// ── Public re-exports ─────────────────────────────────────────────────────────

pub use condition::Condition;
pub use error::RuleEngineError;
pub use eval_rules::evaluate_rules;
pub use orchestrator::evaluate_flags;
pub use types::{ConditionExpr, EvaluationInput, PercentageTarget, Rule, RuleOutput, TargetField};
