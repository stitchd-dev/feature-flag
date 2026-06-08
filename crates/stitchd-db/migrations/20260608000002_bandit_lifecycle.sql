-- =============================================================================
-- Bandit autonomous lifecycle state (bandit_20260608, Phase 7 Task 7.2).
-- Purely additive.
--
-- Records the most recently DETECTED convergence on a bandit experiment so the
-- advisory "ready to commit" badge (LifecyclePolicy::Advisory) and the
-- Phase-11 surfacing layer can read it without re-running the Monte-Carlo
-- detector. Written every tick a bandit converges (advisory, auto_commit and
-- auto_rollout all stamp it); NULL until the experiment first converges.
--
--   * bandit_converged_variant — the winning variant key (NULL = not converged)
--   * bandit_converged_prob     — its posterior probability-to-be-best in [0,1]
-- =============================================================================

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS bandit_converged_variant TEXT;

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS bandit_converged_prob DOUBLE PRECISION;
