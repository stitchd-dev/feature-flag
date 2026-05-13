export interface Segment {
  id: string
  name: string
  description?: string
  tags: string[]
  condition_expr?: unknown
  user_list: string[]
  condition_count: number
  created_at: string
  updated_at: string
}

export interface CreateSegmentRequest {
  name: string
  description?: string
  tags: string[]
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
}
