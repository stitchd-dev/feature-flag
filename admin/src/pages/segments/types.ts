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
