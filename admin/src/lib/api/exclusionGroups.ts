/**
 * Exclusion-group + experiment-interaction API client (Phase 8 — xexp).
 *
 * Wraps the env-scoped REST endpoints exposed by the gateway:
 *
 *   GET    /v1/environments/{env}/exclusion-groups
 *   POST   /v1/environments/{env}/exclusion-groups
 *   GET    /v1/environments/{env}/exclusion-groups/{group}
 *   PATCH  /v1/environments/{env}/exclusion-groups/{group}
 *   DELETE /v1/environments/{env}/exclusion-groups/{group}
 *   POST   /v1/environments/{env}/experiments/{exp}/exclusion-group
 *   DELETE /v1/environments/{env}/experiments/{exp}/exclusion-group
 *   GET    /v1/environments/{env}/experiments/{exp}/interactions
 *
 * `bp` ("basis points") are integers in [0, 10000]; `allocated_bp + free_bp`
 * always sums to 10000 for a healthy group.
 */
import { api } from '../api'

/** Total basis points in a group (100% capacity). */
export const TOTAL_BP = 10000

/**
 * A mutual-exclusion group. Experiments assigned to the same group never share
 * traffic — their allocations are carved out of the group's basis-point budget.
 */
export interface ExclusionGroup {
  id: string
  env_id: string
  name: string
  description: string
  /**
   * Randomization unit the group diverts on (e.g. "user"). Every member
   * experiment must randomize on this context type. Immutable after creation.
   */
  unit_context_type: string
  /** Basis points currently allocated to assigned experiments (0–10000). */
  allocated_bp: number
  /** Basis points still available to assign (0–10000). */
  free_bp: number
  /** Optimistic-concurrency version. */
  version: number
}

export interface ExclusionGroupListResponse {
  items: ExclusionGroup[]
  total: number
  page: number
  per_page: number
}

/**
 * One N-way interaction estimate among a tuple of experiments that share
 * assignment population, scoped to a context type + metric.
 * `interaction_order` is the tuple size (2 = pairwise, 3 = three-way).
 */
export interface ExperimentInteraction {
  /** Experiment ids in this interaction tuple (length == interaction_order). */
  experiment_ids: string[]
  /**
   * Human-readable names aligned 1:1 with experiment_ids (same order, same
   * length); ids without a resolvable experiment fall back to the id string.
   */
  experiment_names: string[]
  /** Number of experiments in the tuple: 2 for pairwise, 3 for three-way. */
  interaction_order: number
  /** Term key: `main:<uuid>` / `2way:<a>x<b>` / `3way:<a>x<b>x<c>`. */
  term: string
  context_type: string
  metric_key: string
  /** Number of contexts assigned in every experiment of the tuple. */
  shared_count: number
  /** Estimated interaction effect size. */
  interaction_estimate: number
  p_value: number
  /** Degrees of freedom of the interaction test. */
  df: number
  /** True when the interaction is statistically significant. */
  significant: boolean
  /**
   * True when there isn't enough shared exposure to estimate the interaction.
   * When set, `interaction_estimate` and `p_value` are 0.0 sentinels (not real
   * values) and the row must never be shown as significant.
   */
  insufficient_data: boolean
  /** Bayesian posterior probability that an interaction effect exists. */
  bayes_prob: number
  /** Bayesian posterior expected interaction effect size. */
  bayes_expected: number
  /** Lower bound of the Bayesian credible interval. */
  bayes_ci_low: number
  /** Upper bound of the Bayesian credible interval. */
  bayes_ci_high: number
}

export interface InteractionsResponse {
  interactions: ExperimentInteraction[]
}

// ─── Exclusion-group CRUD ───────────────────────────────────────────────────

export async function listExclusionGroups(
  envId: string,
  params?: { page?: number; per_page?: number },
  signal?: AbortSignal,
): Promise<ExclusionGroupListResponse> {
  const qs = new URLSearchParams()
  if (params?.page != null) qs.set('page', String(params.page))
  if (params?.per_page != null) qs.set('per_page', String(params.per_page))
  const suffix = qs.toString() ? `?${qs}` : ''
  const { data } = await api.get<ExclusionGroupListResponse>(
    `/v1/environments/${envId}/exclusion-groups${suffix}`,
    { signal },
  )
  return data
}

export async function getExclusionGroup(
  envId: string,
  groupId: string,
  signal?: AbortSignal,
): Promise<ExclusionGroup> {
  const { data } = await api.get<ExclusionGroup>(
    `/v1/environments/${envId}/exclusion-groups/${groupId}`,
    { signal },
  )
  return data
}

export async function createExclusionGroup(
  envId: string,
  body: { name?: string; description?: string; unit_context_type: string },
): Promise<ExclusionGroup> {
  const { data } = await api.post<ExclusionGroup>(
    `/v1/environments/${envId}/exclusion-groups`,
    body,
  )
  return data
}

export async function updateExclusionGroup(
  envId: string,
  groupId: string,
  body: { name?: string; description?: string; version?: number },
): Promise<ExclusionGroup> {
  const { data } = await api.patch<ExclusionGroup>(
    `/v1/environments/${envId}/exclusion-groups/${groupId}`,
    body,
  )
  return data
}

export async function deleteExclusionGroup(
  envId: string,
  groupId: string,
): Promise<void> {
  await api.delete(`/v1/environments/${envId}/exclusion-groups/${groupId}`)
}

// ─── Experiment ↔ group assignment ──────────────────────────────────────────

/** Envelope returned by assign / unassign — the updated experiment + group. */
export interface ExperimentGroupAssignment {
  experiment: Record<string, unknown>
  group: ExclusionGroup
}

export async function assignExperimentToGroup(
  envId: string,
  experimentId: string,
  body: { group_id: string; requested_bp: number },
): Promise<ExperimentGroupAssignment> {
  const { data } = await api.post<ExperimentGroupAssignment>(
    `/v1/environments/${envId}/experiments/${experimentId}/exclusion-group`,
    body,
  )
  return data
}

export async function unassignExperimentFromGroup(
  envId: string,
  experimentId: string,
): Promise<ExperimentGroupAssignment> {
  const { data } = await api.delete<ExperimentGroupAssignment>(
    `/v1/environments/${envId}/experiments/${experimentId}/exclusion-group`,
  )
  return data
}

// ─── Interactions ───────────────────────────────────────────────────────────

export async function listExperimentInteractions(
  envId: string,
  experimentId: string,
  signal?: AbortSignal,
): Promise<InteractionsResponse> {
  const { data } = await api.get<InteractionsResponse>(
    `/v1/environments/${envId}/experiments/${experimentId}/interactions`,
    { signal },
  )
  return data
}
