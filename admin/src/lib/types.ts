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
  created_at: string
  updated_at: string
}

export interface VariantJson {
  key: string
  value: unknown
}

export interface RuleJson {
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
}
