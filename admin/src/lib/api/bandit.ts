/**
 * Bandit state / allocation-history / campaign API client (FR7, Phase 11→12).
 *
 * Wraps the env-scoped REST endpoints the gateway surfaces:
 *
 *   GET /v1/environments/{env}/experiments/{exp}/bandit
 *   GET /v1/environments/{env}/experiments/{exp}/bandit/history?limit=N
 *   GET /v1/environments/{env}/bandit-campaigns
 *   GET /v1/environments/{env}/bandit-campaigns/{campaign}
 *
 * Shapes mirror the Phase 11 DTOs (`learnings.md` Phase 11). Optional fields are
 * omitted (serde skip_serializing_if) when empty/absent, so most are `?`.
 */
import { api } from '../api'

/** One arm's current weight, in basis points (sum across arms = 10000). */
export interface BanditAllocationBucket {
  variant_key: string
  weight_bp: number
}

/** Per-variant reward posterior for a single objective. */
export interface BanditObjectiveVariant {
  variant_key: string
  mean: number
  ci_lower: number
  ci_upper: number
  n: number
  guardrail_violated: boolean
}

/** One objective's posterior block (scalar / weighted / primary / guardrail). */
export interface BanditObjective {
  metric_id: string
  /** Scalar | Weighted | Primary | Guardrail. */
  role: string
  /** Only present for role=weighted. */
  weight?: number
  /** Goal direction the bandit optimises this metric toward. */
  goal: string
  variants: BanditObjectiveVariant[]
}

/** Nested `objectives` block surfaced under bandit state. */
export interface BanditObjectives {
  objectives: BanditObjective[]
}

/** Bandit config echo (nested JSON re-hydrated by the gateway). */
export interface BanditConfigEcho {
  algorithm?: string
  propagation_mode?: string
  lifecycle_policy?: string
  min_exploration_bp?: number
  objective?: unknown
  [k: string]: unknown
}

/**
 * `GET …/bandit` — current bandit state for an experiment.
 *
 * Optional fields are omitted when absent; `is_bandit=false` means the
 * experiment is a classic fixed test (the Bandit view must hide itself).
 */
export interface BanditState {
  experiment_id: string
  is_bandit: boolean
  current_allocation: BanditAllocationBucket[]
  objectives?: BanditObjectives
  bandit_config?: BanditConfigEcho
  converged_variant?: string
  converged_prob?: number
  has_converged: boolean
  committed: boolean
  campaign_id?: string
  campaign_status?: string
}

/**
 * One row in the allocation-history timeline. `new_allocation` carries per-arm
 * `{variant: bp}` plus a reserved `bandit_objectives` key.
 */
export interface BanditAllocationRun {
  /** RFC3339 timestamp. */
  fired_at: string
  /** reallocate | commit | rollout | skip | spawn_iteration | … */
  action: string
  /** applied | skipped | failed. */
  outcome: string
  old_allocation?: Record<string, unknown> | null
  new_allocation?: Record<string, unknown> | null
  detail?: string
}

export interface BanditAllocationHistory {
  /** Newest-first. */
  runs: BanditAllocationRun[]
}

/** A standing autonomous-optimization campaign. */
export interface BanditCampaign {
  id: string
  environment_id: string
  flag_id: string
  name: string
  config?: unknown
  status: string
  iterations_spawned: number
  version: number
}

export interface BanditCampaignList {
  campaigns: BanditCampaign[]
}

// ─── Client functions ───────────────────────────────────────────────────────

export async function getBanditState(
  environmentId: string,
  experimentId: string,
  signal?: AbortSignal,
): Promise<BanditState> {
  const { data } = await api.get<BanditState>(
    `/v1/environments/${encodeURIComponent(environmentId)}/experiments/${encodeURIComponent(experimentId)}/bandit`,
    { signal },
  )
  return data
}

export async function getBanditAllocationHistory(
  environmentId: string,
  experimentId: string,
  params?: { limit?: number },
  signal?: AbortSignal,
): Promise<BanditAllocationHistory> {
  const qs = new URLSearchParams()
  if (params?.limit != null) qs.set('limit', String(params.limit))
  const suffix = qs.toString() ? `?${qs}` : ''
  const { data } = await api.get<BanditAllocationHistory>(
    `/v1/environments/${encodeURIComponent(environmentId)}/experiments/${encodeURIComponent(experimentId)}/bandit/history${suffix}`,
    { signal },
  )
  return data
}

export async function listBanditCampaigns(
  environmentId: string,
  signal?: AbortSignal,
): Promise<BanditCampaignList> {
  const { data } = await api.get<BanditCampaignList>(
    `/v1/environments/${encodeURIComponent(environmentId)}/bandit-campaigns`,
    { signal },
  )
  return data
}

export async function getBanditCampaign(
  environmentId: string,
  campaignId: string,
  signal?: AbortSignal,
): Promise<BanditCampaign> {
  const { data } = await api.get<BanditCampaign>(
    `/v1/environments/${encodeURIComponent(environmentId)}/bandit-campaigns/${encodeURIComponent(campaignId)}`,
    { signal },
  )
  return data
}
