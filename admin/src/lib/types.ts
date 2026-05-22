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
  experiment_id: string
  environment_id: string
  key: string
  name: string
  description: string
  flag_key: string
  /** Flag UUID (Phase 10 — surfaced for the list filter; optional until
   *  gateway surfaces it everywhere). */
  flag_id?: string
  status: string // "draft" | "running" | "stopped" | "completed"
  model: string // "frequentist" | "bayesian"
  /**
   * UUIDs referencing `metric_definitions` rows attached to this experiment.
   * Phase 7 cutover replaced the legacy `primary_metric: string` (free-form
   * event key). The first entry is conventionally treated as primary; the
   * remainder as secondaries.
   */
  metric_ids: string[]
  variants: number
  started_at: string | null
  ended_at: string | null
  /** Optional ISO-8601 scheduled end. UI uses this to compute "days remaining"
   *  on the experiments list. */
  scheduled_end_at?: string | null
  /** Optional list of context types declared on the experiment. */
  unit_context_types?: string[]
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
  assigned_count: number
  /** Mean of the primary metric across all assigned contexts (or null when no data). */
  mean: number | null
  /** Sample size (events used in the computation). */
  sample_size: number
  /** Frequentist: two-sided p-value vs control (null for the control row itself). */
  p_value: number | null
  /** Frequentist: 95% confidence interval bounds for the mean estimate. */
  ci_lower: number | null
  ci_upper: number | null
  /** Bayesian: posterior probability that this variant beats control. */
  prob_to_beat_control: number | null
  /** Relative lift vs control (null for the control row, expressed as a fraction). */
  expected_lift: number | null
  /** True when this variant is the recommended winner per `goal_direction`. */
  is_winner: boolean
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
