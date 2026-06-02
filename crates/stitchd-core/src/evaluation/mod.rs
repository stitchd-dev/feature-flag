pub mod engine;
pub mod exclusion;
pub mod preview;
#[cfg(test)]
mod purity;
pub mod types;

pub use engine::{FlagEvaluator, evaluate_flag};
pub use types::{
    EvalOutcome, EvaluationTrace, FlagEvaluationResult, HashInputSpec, HashSelector,
    ListMembershipIndex, TraceLevel,
};
