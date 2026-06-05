// Shared API response types matching the gateway's AdminFlagJson shape.

export interface PaginatedResponse<T> {
  items: T[]
  total: number
  page: number
  per_page: number
}

// ── Segment domain types ──────────────────────────────────────────────────────

export type SegmentType = 'rule' | 'list'

export interface Segment {
  id: string
  name: string
  description?: string
  tags: string[]
  segment_type?: SegmentType
  /** Context kind this list targets (e.g. "user", "org", "device"). Only set for list segments. */
  context_type?: string
  condition_expr?: unknown
  /** @deprecated Always empty — use include_count instead. */
  user_list: string[]
  /** @deprecated Always empty — use exclude_count instead. */
  excluded_keys?: string[]
  /** Count of include-list entries (list-based segments only). */
  include_count: number
  /** Count of exclude-list entries (list-based segments only). */
  exclude_count: number
  condition_count: number
  version: number
  created_at: string
  updated_at: string
}

export interface CreateSegmentRequest {
  name: string
  description?: string
  tags: string[]
  segment_type: SegmentType
  /** Context kind for list-based segments. Required when segment_type is 'list'. */
  context_type?: string
  condition_expr?: unknown
  user_list: string[]
  env_id: string
  org_id: string
  project_id: string
}

export interface UpdateSegmentRequest {
  name: string
  description?: string
  tags: string[]
  condition_expr?: unknown
  user_list: string[]
  excluded_keys?: string[]
  /** Context kind for list-based segments. */
  context_type?: string
}

// ── Experiment domain types ───────────────────────────────────────────────────

export interface ExperimentResponse {
  /** UUID returned as `id` by the gateway. */
  id: string
  /** Alias for `id` (gateway provides both for back-compat). */
  key: string
  environment_id: string
  name: string
  description: string
  flag_key: string
  /** Flag UUID (optional until gateway surfaces it everywhere). */
  flag_id?: string
  status: string // "draft" | "active" | "paused" | "concluded"
  model: string // "frequentist" | "bayesian"
  /**
   * UUIDs referencing `metric_definitions` rows attached to this experiment.
   * Empty until the experimentation-service proto carries metric_ids.
   */
  metric_ids: string[]
  /** Number of variants (derived from variant_keys.length on the backend). */
  variants: number
  variant_keys: string[]
  started_at: string | null
  ended_at: string | null
  /** Optional ISO-8601 scheduled end. UI uses this to compute "days remaining"
   *  on the experiments list. */
  scheduled_end_at?: string | null
  unit_context_types: string[]
  /** UUID of the mutual-exclusion group this experiment is assigned to, when
   *  any. Absent / null when the experiment is ungrouped. */
  exclusion_group_id?: string | null
  created_at: string
  updated_at: string
}

export interface VariantJson {
  key: string
  value: unknown
}

export interface RuleJson {
  /**
   * UUID of the underlying `feature_flag_rules.id` row.
   * Surfaced by the gateway so admin flows (notably experiment creation) can
   * bind to a real rule UUID instead of fabricating index-derived placeholders.
   */
  rule_id: string
  /** Optional human-readable label set by the user; ignored by the evaluator. */
  name?: string
  /** ConditionExpr serde JSON — see ruleTypes.ts for the full type */
  condition: unknown
  /** Gateway output JSON — `{variant_key: "..."}` or `{allocation: [...]}` */
  output: unknown
  /**
   * Segment IDs referenced in this rule's condition (populated by Phase 1 backend).
   * Used to resolve segment names for display without a separate fetch.
   */
  segment_ids?: string[]
}

export interface AdminFlagResponse {
  flag_id: string
  key: string
  name: string
  description: string
  flag_type: 'bool' | 'string' | 'int' | 'double' | 'json'
  enabled: boolean
  status: 'enabled' | 'disabled'
  version: number
  variants: VariantJson[]
  rules: RuleJson[]
  default_variant_key: string | null
  created_at: string | null
  updated_at: string | null
  /**
   * UUID of the experiment currently locking this flag (running or paused).
   * Omitted by the gateway when the flag is not locked. Lets the admin UI
   * render the lock badge proactively instead of after a failing save round-trip.
   */
  locked_by_experiment_id?: string
}

// ── Experiment results / exposures / timeseries types (Phase 7) ──────────────
//
// Mirrors the gateway's per-context-type result shape introduced in Phase 5/7:
//
//   {
//     "results_by_context_type": {
//       "user":    { "variants": [...], "srm": {...}, "guardrails": [...] },
//       "account": { ... }
//     },
//     "bound_target": { "kind": "rule" | "default_rule", "rule_id": "<uuid|null>", "label": "..." },
//     "pre_period_days": 0
//   }
//
// All numeric fields are nullable when no data is available yet (e.g. an
// experiment with zero exposures still returns a `variants` array with
// per-variant rows, but most stats fields are null).

/** One row in the per-(context_type, variant_key) results table. */
export interface VariantResultJson {
  variant_key: string
  /** Number of unique contexts assigned to this variant. */
  participant_count: number
  /** Frequentist: two-sided p-value vs control (null / absent for the control row). */
  p_value: number | null
  /** Multiple-comparison-corrected p-value (Bonferroni); absent for single-metric runs. */
  p_value_corrected?: number | null
  /** Observed treatment − control lift. `0.0` for the control row. */
  lift: number
  /** True when this row violates the metric's goal_direction (guardrail rows only). */
  direction_violation: boolean
  /** Attribution unit context type ("user", "account", …). */
  context_type: string

  // ── Sequential (always-valid) inference (Phase 5.2) ────────────────────────
  //
  // Attached by the gateway only when sequential testing is enabled for the
  // experiment. All keys are OMITTED when disabled; the CI bounds are also
  // omitted when there is insufficient data (`sequential_insufficient_data`).
  //
  /** Always-valid (anytime-valid) p-value vs control. Control baseline = 1.0. */
  sequential_p_value?: number
  /** Lower bound of the anytime-valid confidence interval; absent when insufficient data. */
  sequential_ci_lower?: number
  /** Upper bound of the anytime-valid confidence interval; absent when insufficient data. */
  sequential_ci_upper?: number
  /** True when the always-valid p-value has crossed the significance boundary (p ≤ α). */
  sequential_crossed?: boolean
  /** True when there is not yet enough data for a valid sequential decision. */
  sequential_insufficient_data?: boolean
  /** Sequential testing method used (e.g. "msprt"). */
  sequential_method?: string
}

/** Per-variant SRM (Sample Ratio Mismatch) row. */
export interface SrmPerVariant {
  variant_key: string
  /** Count actually observed for this variant. */
  observed_count: number
  /** Count expected given the allocation. */
  expected_count: number
  /** Signed deviation as a fraction (e.g. -0.03 = 3% under-assigned). */
  deviation: number
}

/** Top-level SRM block per context type. */
export interface SrmResultJson {
  per_variant: SrmPerVariant[]
  /** Chi-square test p-value across all variants for this context type. */
  overall_chi_sq_p: number | null
  /** Health rollup: red when chi_sq_p < 0.001, yellow when < 0.01, else green. */
  health: 'green' | 'yellow' | 'red'
}

/** Per-context-type result block. */
export interface ContextTypeResultJson {
  variants: VariantResultJson[]
  srm: SrmResultJson
  /** Optional guardrail metric results — empty array when no guardrails attached. */
  guardrails: VariantResultJson[]
}

/** Describes whether the experiment is bound to a specific flag rule or the default-rule fallthrough. */
export interface BoundTarget {
  kind: 'rule' | 'default_rule'
  /** UUID of the bound flag rule, or `null` when `kind === 'default_rule'`. */
  rule_id: string | null
  /** Human-readable label for UI display ("checkout-percentage-rule" or "Default rule"). */
  label: string
}

/** Top-level experiment results envelope returned by `GET /experiments/{id}/results`. */
export interface ExperimentResults {
  results_by_context_type: Record<string, ContextTypeResultJson>
  bound_target: BoundTarget
  pre_period_days: number
}

/** One row in the paginated exposures list. */
export interface ExposureRow {
  context_type: string
  context_key: string
  variant_key: string
  /** ISO-8601 timestamp of the first exposure (assignment) for this context. */
  assigned_at: string
  /** UUID of the matched rule when the experiment is rule-bound; `null` for default-rule experiments. */
  matched_rule_id: string | null
}

/** Paginated response envelope used by `GET /experiments/{id}/exposures`. */
export interface PaginatedExposures {
  items: ExposureRow[]
  total: number
  page: number
  per_page: number
}

/** One per-variant per-day bucket from the timeseries endpoint. */
export interface TimeseriesBucket {
  /** ISO-8601 date (YYYY-MM-DD) at UTC. */
  day: string
  variant_key: string
  /** Aggregated metric value for this (day, variant) — null when the variant had no data on this day. */
  value: number | null
  /** Sample size contributing to this bucket. */
  sample_size: number
}

/** Daily per-variant time-series returned by `GET /experiments/{id}/timeseries`. */
export interface Timeseries {
  metric_id: string
  context_type: string
  /** All buckets, ordered by (day asc, variant_key asc). */
  buckets: TimeseriesBucket[]
}

// ── Rollout distribution (Phase 1 domain type) ──────────────────────────────
//
// Mirrors `stitchd-core::rollout::RolloutDistribution` on the backend.
// Used as the request body for `POST /flags/{key}/default-rule-distribution`
// and as the (optional) value of `AdminFlagResponse.default_rule_distribution`
// once the flag-fetch endpoint surfaces it.
export interface RolloutAllocation {
  variant_key: string
  /** Percentage in the range [0, 100]; the sum across allocations must equal 100. */
  percentage: number
}

export interface RolloutDistribution {
  allocations: RolloutAllocation[]
  /**
   * Ordered selector list driving the default-rule percentage hash.
   * Mirrors the gateway's `DefaultRuleDistributionBody.hash_inputs` (Phase 4
   * of `flag_eval_unify_20260522`). When omitted, the server falls back to
   * the flag's `default_rule_hash_inputs` column (the legacy single-input
   * default). The wire shape mirrors `HashSelectorJson` 1:1 — see
   * `admin/src/lib/hashInputTypes.ts`.
   */
  hash_inputs?: import('./hashInputTypes').HashSelector[]
}

// ── Flag-lifecycle automation types (flag_lifecycle_20260604, Phase 8) ────────
//
// Mirror the gateway DTOs in `crates/stitchd-gateway/src/routes/{schedules,
// dependencies,flags}.rs`. Scheduled changes are ENVIRONMENT-scoped
// (`/v1/environments/{env_id}/...`); prerequisites + the dependency graph are
// PROJECT-scoped (`/v1/projects/{project_id}/...`).

/** Entity kinds a schedule / dependency endpoint accepts in its URL path. */
export const LIFECYCLE_ENTITY_KINDS = ['flags', 'segments', 'experiments'] as const
export type LifecycleEntityKind = (typeof LIFECYCLE_ENTITY_KINDS)[number]

/** Schedule recurrence kind. */
export type ScheduleKind = 'one_shot' | 'recurring'

/**
 * Schedule lifecycle status. One-shot: `pending → applied | failed | cancelled`.
 * Recurring: `active ⇄ paused`.
 */
export type ScheduleStatus =
  | 'pending'
  | 'active'
  | 'paused'
  | 'applied'
  | 'failed'
  | 'cancelled'
  | 'unspecified'

/** Outcome of a single scheduled firing. */
export type ScheduleRunOutcome = 'applied' | 'skipped' | 'failed' | 'unspecified'

/** One firing of a scheduled change (run history). */
export interface ScheduleRun {
  id: string
  /** Epoch-ms instant the run fired. */
  fired_at_ms: number
  outcome: ScheduleRunOutcome
  /** Free-text reason / error detail (e.g. "skipped: flag locked by experiment"). */
  detail: string
}

/**
 * A scheduled change. Mirrors the gateway `ScheduleJson`. `mutation_payload` is
 * the entity-specific JSON the owning service's RPC consumes at fire time
 * (flag → MutateFlag, experiment → transition, segment → definition update).
 */
export interface ScheduledChange {
  id: string
  /** `flag` | `segment` | `experiment` (singular, per the gateway). */
  entity_type: 'flag' | 'segment' | 'experiment' | 'unspecified'
  entity_id: string
  env_id: string
  mutation_payload: unknown
  schedule_kind: ScheduleKind | 'unspecified'
  /** One-shot: epoch-ms instant the change fires (UTC). 0 for recurring. */
  scheduled_at_ms: number
  /** Recurring: RFC-5545 RRULE string. Empty for one-shot. */
  rrule: string
  /** Recurring: IANA timezone the RRULE is evaluated in. */
  tz: string
  status: ScheduleStatus
  /** Epoch-ms of the next scheduled firing (0 when none pending). */
  next_run_at_ms: number
  /** Epoch-ms of the most recent firing (0 when never fired). */
  last_run_at_ms: number
  created_at_ms: number
  updated_at_ms: number
  /** Optimistic-concurrency version (passed back on cancel/pause/resume). */
  version: number
  /** Run history (most-recent-first); populated on `get`, empty on `list`. */
  runs: ScheduleRun[]
}

/** Request body to create a scheduled change (gateway `CreateScheduleBody`). */
export interface CreateScheduleBody {
  mutation_payload: unknown
  schedule_kind: ScheduleKind
  /** Required for `one_shot` — epoch-ms instant (UTC). */
  scheduled_at_ms?: number
  /** Required for `recurring` — RFC-5545 RRULE string. */
  rrule?: string
  /** Required for `recurring` — IANA timezone. */
  tz?: string
}

/** One prerequisite-gate edge (gateway `PrerequisiteJson`). */
export interface Prerequisite {
  /** UUID of the prerequisite flag (optional on write; the key is sufficient). */
  prerequisite_flag_id?: string
  prerequisite_flag_key: string
  /** UUID of the required variant (optional on write). */
  required_variant_id?: string
  required_variant_key: string
}

/** A flag's prerequisite gate (gateway `PrerequisitesResponseJson`). */
export interface PrerequisiteGate {
  prerequisites: Prerequisite[]
  /** Key of the configured fallback variant; empty = the off/disabled variant. */
  fallback_variant_key: string
}

/** Request body to replace a flag's prerequisite gate (gateway `SetPrerequisitesBody`). */
export interface SetPrerequisitesBody {
  prerequisites: Prerequisite[]
  fallback_variant_key: string
  /** Optimistic-locking version matching the flag's current `version`. */
  version: number
}

/** One edge in the dependency graph (gateway `DependencyEdge`). */
export interface DependencyEdge {
  /** Entity kind of the referenced entity: `flag` | `segment` | `experiment`. */
  entity_kind: 'flag' | 'segment' | 'experiment'
  /** Stable id of the referenced entity (UUID or key, per kind). */
  id: string
  /** Human-readable key, when known (else empty). */
  key: string
  /** Relationship kind: `prerequisite_flag` | `segment_ref` | `dependent_flag`. */
  kind: 'prerequisite_flag' | 'segment_ref' | 'dependent_flag' | string
}

/** The dependency graph for one entity (gateway `DependencyGraphJson`). */
export interface DependencyGraph {
  entity_kind: 'flag' | 'segment' | 'experiment'
  entity_id: string
  /** Entities the subject depends on. */
  upstream: DependencyEdge[]
  /** Entities that depend on the subject. */
  downstream: DependencyEdge[]
  /** Note describing edges that could not be computed (empty when complete). */
  note?: string
}

/**
 * The structured `409 dependency_exists` body returned when deleting/archiving a
 * still-referenced entity. Mirrors the gateway error envelope.
 */
export interface DependencyExistsError {
  error: 'dependency_exists'
  /** Blocking dependent entity ids — references must be removed first. */
  dependents: string[]
  message: string
}
