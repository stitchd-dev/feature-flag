//! Flag apply path (Phase 3 Task 3) — placeholder pending implementation.
//!
//! Real dispatch to flag-service `MutateFlag` (with experiment-lock skip
//! detection) lands in Task 3.3. The seam (the [`crate::apply::Applier`] trait)
//! is defined now so the scheduler loop (Task 3.2) can be wired and tested
//! against an injected applier.

use async_trait::async_trait;
use stitchd_db::ScheduledChangeRow;

use crate::apply::{Applier, ApplyOutcome};

/// Placeholder flag applier (real implementation in Task 3.3).
pub struct FlagApplier;

#[async_trait]
impl Applier for FlagApplier {
    async fn apply(&self, _change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
        Ok(ApplyOutcome::Failed(
            "flag apply not yet implemented".to_string(),
        ))
    }
}
