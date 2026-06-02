# Track Learnings: xexp_interaction_20260602

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

See `conductor/patterns.md` for the full set. Most relevant to this track:
- **Evaluation is pure + in-memory.** `stitchd-core::evaluation::evaluate_flag` (engine.rs:70) is
  non-async, does zero I/O, and is experiment-unaware. The SDK holds definitions in a lock-free
  `ArcSwap<DefinitionSnapshot>` (snapshot.rs:259). Experiment→rule routing is post-evaluation only
  (`experiment_assignments_mv`, keyed on `(env_id, flag_id, matched_rule_id, context_type)`).
- **Exclusion gate must ride on the rule** as static snapshot data (like `hash_inputs`/`weights`),
  flowing through the existing PG → proto → server-streaming sync. The experimentation-service stamps
  it on assign and clears it on unassign/stop. The core never looks up an experiment.
- **ClickHouse first-exposure** assignments live in `experiment_assignments`
  (ReplacingMergeTree, inverted `_version`); readers use `FINAL`/`argMin`. Detection self-joins this
  table on `(env_id, context_type, context_key)`.
- **Stats query builders are pure** (`queries::{aggregation,ratio,funnel,preview}` → `BuiltQuery`),
  parameterized (no `format!()` SQL). Interaction follows the same shape.
- **Parallel waves:** isolated worktrees, file-ownership table per worker prompt, repo-side worker
  owns shared traits, `bd close <id>` (plain) per the workflow.md beads-close gotcha.

---

<!-- Learnings from implementation will be appended below -->
