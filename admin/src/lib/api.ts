import axios from 'axios'
import { auth } from './auth'
import type {
  AdminFlagResponse,
  CursorPage,
  Segment,
  CreateSegmentRequest,
  UpdateSegmentRequest,
  ExperimentResults,
  PaginatedExposures,
  Timeseries,
  RolloutDistribution,
  LifecycleEntityKind,
  ScheduledChange,
  CreateScheduleBody,
  PrerequisiteGate,
  SetPrerequisitesBody,
  DependencyGraph,
} from './types'

export const api = axios.create({
  baseURL: import.meta.env.VITE_API_BASE_URL ?? '/api',
})

// Inject JWT on every request
api.interceptors.request.use((config) => {
  const token = auth.getToken()
  if (token) config.headers.Authorization = `Bearer ${token}`
  return config
})

// On 401, attempt refresh once before redirecting to login.
// Concurrent 401s are queued and replayed after a single refresh call.
let refreshing: Promise<string> | null = null

api.interceptors.response.use(
  (r) => r,
  async (err) => {
    const original = err.config
    if (err.response?.status !== 401 || original._retried) {
      return Promise.reject(err)
    }
    const refreshToken = auth.getSession()?.refreshToken
    if (!refreshToken) {
      auth.clearSession()
      window.location.href = '/login'
      return Promise.reject(err)
    }
    original._retried = true
    if (!refreshing) {
      refreshing = axios
        .post<{ access_token: string; refresh_token: string; expires_in: number }>(
          `${api.defaults.baseURL ?? '/api'}/v1/auth/refresh`,
          { refresh_token: refreshToken },
        )
        .then(({ data }) => {
          const session = auth.getSession()!
          auth.setSession({ ...session, token: data.access_token, refreshToken: data.refresh_token })
          return data.access_token
        })
        .catch((refreshErr) => {
          auth.clearSession()
          window.location.href = '/login'
          return Promise.reject(refreshErr)
        })
        .finally(() => {
          refreshing = null
        })
    }
    const newToken = await refreshing
    original.headers = { ...original.headers, Authorization: `Bearer ${newToken}` }
    return api(original)
  },
)

// ─── Auth ────────────────────────────────────────────────────────────────────

export interface LoginResponse {
  access_token: string
  refresh_token: string
  expires_in: number
  user_id: string
  org_id: string
}

export async function loginWithPassword(
  email: string,
  password: string,
  orgId?: string,
): Promise<LoginResponse> {
  const { data } = await api.post<LoginResponse>('/v1/auth/login', {
    email,
    password,
    org_id: orgId,
  })
  return data
}

export interface OidcAuthorizeResponse {
  redirect_url: string
}

export async function initiateOidc(
  orgId: string,
): Promise<OidcAuthorizeResponse> {
  const { data } = await api.post<OidcAuthorizeResponse>(
    `/v1/orgs/${orgId}/auth/oidc/authorize`,
  )
  return data
}

export async function initiateSaml(orgId: string): Promise<{ redirect_url: string }> {
  const { data } = await api.post<{ redirect_url: string }>(
    `/v1/orgs/${orgId}/auth/saml/sso`,
  )
  return data
}

// ─── Org switching ────────────────────────────────────────────────────────────

export interface OrgEntry {
  org_id: string
  org_name: string
  role: string
}

export async function listUserOrgs(): Promise<OrgEntry[]> {
  const { data } = await api.get<OrgEntry[]>('/v1/auth/me/orgs')
  return data
}

export interface SwitchOrgResponse {
  access_token: string
  refresh_token: string
  expires_in: number
  org_id: string
  // user_id is intentionally absent — the caller preserves it from the current session
}

export async function switchOrg(orgId: string): Promise<SwitchOrgResponse> {
  const { data } = await api.post<SwitchOrgResponse>('/v1/auth/switch-org', { org_id: orgId })
  return data
}

// ─── Permissions ─────────────────────────────────────────────────────────────

export interface PermissionsResponse {
  roles: string[]
  permissions: string[]
}

export async function getMyPermissions(): Promise<PermissionsResponse> {
  const { data } = await api.get<PermissionsResponse>('/v1/auth/me/permissions')
  return data
}

// ─── Projects ────────────────────────────────────────────────────────────────

export interface ProjectSummary {
  project_id: string
  project_name: string
  created_at: string
}

export async function listProjects(orgId: string): Promise<ProjectSummary[]> {
  const { data } = await api.get<{ projects: ProjectSummary[] }>(
    `/v1/management/orgs/${orgId}/projects`,
  )
  return data.projects
}

export interface CreateProjectResponse {
  project_id: string
  project_name: string
}

export async function createProject(
  orgId: string,
  name: string,
): Promise<CreateProjectResponse> {
  const { data } = await api.post<CreateProjectResponse>(
    `/v1/management/orgs/${orgId}/projects`,
    { name },
  )
  return data
}

export async function renameProject(projectId: string, name: string): Promise<void> {
  await api.patch(`/v1/management/projects/${projectId}`, { name })
}

export async function deleteProject(projectId: string): Promise<void> {
  await api.delete(`/v1/management/projects/${projectId}`)
}

// ─── Environments ─────────────────────────────────────────────────────────────

export interface EnvironmentSummary {
  environment_id: string
  environment_name: string
  created_at: string
}

export async function listEnvironments(projectId: string): Promise<EnvironmentSummary[]> {
  const { data } = await api.get<{ environments: EnvironmentSummary[] }>(
    `/v1/management/projects/${projectId}/environments`,
  )
  return data.environments
}

export interface CreateEnvironmentResponse {
  environment_id: string
  environment_name: string
}

export async function createEnvironment(
  projectId: string,
  name: string,
): Promise<CreateEnvironmentResponse> {
  const { data } = await api.post<CreateEnvironmentResponse>(
    `/v1/management/projects/${projectId}/environments`,
    { name },
  )
  return data
}

export async function renameEnvironment(environmentId: string, name: string): Promise<void> {
  await api.patch(`/v1/management/environments/${environmentId}`, { name })
}

export async function deleteEnvironment(environmentId: string): Promise<void> {
  await api.delete(`/v1/management/environments/${environmentId}`)
}

// ─── SDK Keys ─────────────────────────────────────────────────────────────────

export interface SdkKeySummary {
  sdk_key_id: string
  is_active: boolean
  created_at: string
  revoked_at: string | null
}

export async function listSdkKeys(environmentId: string): Promise<SdkKeySummary[]> {
  const { data } = await api.get<{ items: SdkKeySummary[] }>(
    `/v1/management/environments/${environmentId}/sdk-keys`,
  )
  return data.items ?? []
}

export interface CreateSdkKeyResponse {
  sdk_key_id: string
  raw_key: string
}

export async function createSdkKey(environmentId: string, name?: string): Promise<CreateSdkKeyResponse> {
  const { data } = await api.post<CreateSdkKeyResponse>(
    `/v1/management/environments/${environmentId}/sdk-keys`,
    name ? { name } : undefined,
  )
  return data
}

export async function revokeSdkKey(environmentId: string, sdkKeyId: string): Promise<void> {
  await api.delete(`/v1/management/environments/${environmentId}/sdk-keys/${sdkKeyId}`)
}

// ─── Superadmin — Orgs ───────────────────────────────────────────────────────

export interface OrgSummary {
  org_id: string
  org_name: string
  created_at: string | null
}

export async function listOrgs(signal?: AbortSignal): Promise<OrgSummary[]> {
  const { data } = await api.get<{ orgs: OrgSummary[] }>('/v1/superadmin/orgs', { signal })
  return data.orgs
}

export async function getOrg(orgId: string, signal?: AbortSignal): Promise<OrgSummary> {
  const { data } = await api.get<OrgSummary>(`/v1/superadmin/orgs/${orgId}`, { signal })
  return data
}

export interface OrgUserSummary {
  user_id: string
  email: string
  display_name: string
  role: string
  created_at: string
}

export async function listOrgUsers(orgId: string, signal?: AbortSignal): Promise<OrgUserSummary[]> {
  const { data } = await api.get<{ users: OrgUserSummary[] }>(`/v1/superadmin/orgs/${orgId}/users`, { signal })
  return data.users
}

export async function removeOrgUser(orgId: string, userId: string): Promise<void> {
  await api.delete(`/v1/superadmin/orgs/${orgId}/users/${userId}`)
}

export async function createOrg(name: string): Promise<OrgSummary> {
  const { data } = await api.post<{ org_id: string; org_name: string; created_at?: string }>(
    '/v1/superadmin/orgs',
    { name },
  )
  return { org_id: data.org_id, org_name: data.org_name, created_at: data.created_at ?? null }
}

export interface SeedUserResponse {
  user_id: string
  email: string
  display_name: string
}

export async function seedUser(
  orgId: string,
  body: { email: string; display_name?: string; password?: string; org_role?: string },
): Promise<SeedUserResponse> {
  const { data } = await api.post<SeedUserResponse>(`/v1/superadmin/orgs/${orgId}/users`, body)
  return data
}

// ─── Auth Providers ───────────────────────────────────────────────────────────

export interface AuthProviderSummary {
  auth_provider_id: string
  provider_type: string
  display_name: string
  is_enabled: boolean
  created_at: string
}

export async function listAuthProviders(orgId: string, signal?: AbortSignal): Promise<AuthProviderSummary[]> {
  const { data } = await api.get<AuthProviderSummary[]>(`/v1/orgs/${orgId}/auth-providers`, { signal })
  return data
}

export async function getAuthProvider(orgId: string, authProviderId: string, signal?: AbortSignal): Promise<AuthProviderSummary> {
  const { data } = await api.get<AuthProviderSummary>(
    `/v1/orgs/${orgId}/auth-providers/${authProviderId}`,
    { signal },
  )
  return data
}

export async function createAuthProvider(
  orgId: string,
  body: { provider_type: string; display_name: string; config: Record<string, unknown> },
): Promise<AuthProviderSummary> {
  const { data } = await api.post<AuthProviderSummary>(`/v1/orgs/${orgId}/auth-providers`, body)
  return data
}

export async function updateAuthProvider(
  orgId: string,
  authProviderId: string,
  body: { display_name?: string; config?: Record<string, unknown>; is_enabled?: boolean },
): Promise<AuthProviderSummary> {
  const { data } = await api.put<AuthProviderSummary>(
    `/v1/orgs/${orgId}/auth-providers/${authProviderId}`,
    body,
  )
  return data
}

export async function deleteAuthProvider(orgId: string, authProviderId: string): Promise<void> {
  await api.delete(`/v1/orgs/${orgId}/auth-providers/${authProviderId}`)
}

export async function getSamlSpMetadata(orgId: string, authProviderId: string): Promise<string> {
  const { data } = await api.get<string>(
    `/v1/orgs/${orgId}/auth-providers/${authProviderId}/saml/metadata`,
  )
  return data
}

// ─── Flags ────────────────────────────────────────────────────────────────────

export async function listFlags(
  projectId: string,
  params?: { cursor?: string | null; limit?: number; include_archived?: boolean },
  signal?: AbortSignal,
): Promise<CursorPage<AdminFlagResponse>> {
  const qs = new URLSearchParams()
  if (params?.cursor != null) qs.set('cursor', params.cursor)
  if (params?.limit != null) qs.set('limit', String(params.limit))
  if (params?.include_archived) qs.set('include_archived', 'true')
  const { data } = await api.get<CursorPage<AdminFlagResponse>>(
    `/v1/projects/${projectId}/flags?${qs}`,
    { signal },
  )
  return data
}

export async function getFlag(projectId: string, flagKey: string, signal?: AbortSignal): Promise<AdminFlagResponse> {
  const { data } = await api.get<AdminFlagResponse>(
    `/v1/projects/${projectId}/flags/${flagKey}`,
    { signal },
  )
  return data
}

export async function createFlag(
  projectId: string,
  body: {
    key: string
    name: string
    description?: string
    enabled?: boolean
    value_type: string
    variants?: { key: string; value: unknown }[]
  },
): Promise<AdminFlagResponse> {
  const { data } = await api.post<AdminFlagResponse>(`/v1/projects/${projectId}/flags`, body)
  return data
}

export async function updateFlag(
  projectId: string,
  flagKey: string,
  body: Partial<AdminFlagResponse> & { version: number },
): Promise<AdminFlagResponse> {
  const { data } = await api.put<AdminFlagResponse>(
    `/v1/projects/${projectId}/flags/${flagKey}`,
    body,
  )
  return data
}

export async function deleteFlag(projectId: string, flagKey: string): Promise<void> {
  await api.delete(`/v1/projects/${projectId}/flags/${flagKey}`)
}

export async function archiveFlag(projectId: string, flagKey: string): Promise<void> {
  await api.post(`/v1/projects/${projectId}/flags/${flagKey}/archive`, {})
}

export async function updateFlagVariants(
  projectId: string,
  flagKey: string,
  body: { variants: { key: string; value: unknown }[]; version: number },
): Promise<AdminFlagResponse> {
  const { data } = await api.put<AdminFlagResponse>(
    `/v1/projects/${projectId}/flags/${flagKey}/variants`,
    body,
  )
  return data
}

export async function updateFlagRules(
  projectId: string,
  flagKey: string,
  body: { rules: unknown[]; version: number },
): Promise<AdminFlagResponse> {
  const { data } = await api.put<AdminFlagResponse>(
    `/v1/projects/${projectId}/flags/${flagKey}/rules`,
    body,
  )
  return data
}

export async function evaluatePreview(
  projectId: string,
  flagKey: string,
  body: { contexts: unknown[]; environment_id: string },
): Promise<{ results: unknown[] }> {
  const { data } = await api.post<{ results: unknown[] }>(
    `/v1/projects/${projectId}/flags/${flagKey}/evaluate-preview`,
    body,
  )
  return data
}

export async function getEvalStats(
  projectId: string,
  flagKey: string,
  params: { from: string; to: string; granularity?: string },
  signal?: AbortSignal,
): Promise<unknown> {
  const qs = new URLSearchParams(params as Record<string, string>)
  const { data } = await api.get<unknown>(
    `/v1/projects/${projectId}/flags/${flagKey}/eval-stats?${qs}`,
    { signal },
  )
  return data
}

// ─── Segments ─────────────────────────────────────────────────────────────────

export async function listSegments(
  environmentId: string,
  params?: { cursor?: string | null; limit?: number },
  signal?: AbortSignal,
): Promise<CursorPage<Segment>> {
  const qs = new URLSearchParams({ env_id: environmentId })
  if (params?.cursor != null) qs.set('cursor', params.cursor)
  if (params?.limit != null) qs.set('limit', String(params.limit))
  const { data } = await api.get<CursorPage<Segment>>(
    `/v1/segments?${qs}`,
    { signal },
  )
  return data
}

export async function getSegment(segmentId: string, signal?: AbortSignal): Promise<Segment> {
  const { data } = await api.get<Segment>(
    `/v1/segments/${segmentId}`,
    { signal },
  )
  return data
}

export async function createSegment(
  body: CreateSegmentRequest,
): Promise<Segment> {
  const { data } = await api.post<Segment>('/v1/segments', body)
  return data
}

export async function updateSegment(
  segmentId: string,
  body: UpdateSegmentRequest,
): Promise<Segment> {
  const { data } = await api.put<Segment>(
    `/v1/segments/${segmentId}`,
    body,
  )
  return data
}

export async function deleteSegment(segmentId: string): Promise<void> {
  await api.delete(`/v1/segments/${segmentId}`)
}

export async function patchSegmentEntries(
  segmentId: string,
  body: { op: string; keys: string[] },
): Promise<{ include_count: number; exclude_count: number }> {
  const { data } = await api.post<{ include_count: number; exclude_count: number }>(
    `/v1/segments/${segmentId}/entries`,
    body,
  )
  return data
}

export async function lookupSegmentEntry(
  segmentId: string,
  params: { context_type: string; key: string },
): Promise<{ in_include: boolean; in_exclude: boolean }> {
  const qs = new URLSearchParams(params)
  const { data } = await api.get<{ in_include: boolean; in_exclude: boolean }>(
    `/v1/segments/${segmentId}/entries/lookup?${qs}`,
  )
  return data
}

export async function createSegmentInEnv(
  environmentId: string,
  body: CreateSegmentRequest,
): Promise<Segment> {
  const { data } = await api.post<Segment>(
    `/v1/environments/${environmentId}/segments`,
    body,
  )
  return data
}

// ─── Experiments ──────────────────────────────────────────────────────────────

export interface ExperimentSummary {
  /** UUID. The API returns this as `id`; `key` is an alias equal to `id`. */
  id: string
  /** Alias for `id` returned by the gateway for UI back-compat. */
  key: string
  environment_id: string
  name: string
  description: string
  flag_key: string
  status: string
  model: string
  /**
   * UUIDs referencing `metric_definitions` rows. Empty until the
   * experimentation-service proto carries metric_ids (Phase 7 gap).
   */
  metric_ids: string[]
  /** Number of variants (derived from variant_keys.length on the backend). */
  variants: number
  variant_keys: string[]
  started_at: string | null
  ended_at: string | null
  created_at: string
  updated_at: string
  unit_context_types: string[]
  /** UUID of the mutual-exclusion group this experiment is assigned to, when
   *  any. Absent / null when ungrouped. */
  exclusion_group_id?: string | null
}

export async function listExperiments(
  environmentId: string,
  signal?: AbortSignal,
): Promise<{ items: ExperimentSummary[]; total: number }> {
  const { data } = await api.get<{ items: ExperimentSummary[]; total: number }>(
    `/v1/environments/${environmentId}/experiments`,
    { signal },
  )
  return data
}

export async function getExperiment(
  environmentId: string,
  experimentKey: string,
  signal?: AbortSignal,
): Promise<ExperimentSummary> {
  const { data } = await api.get<ExperimentSummary>(
    `/v1/environments/${environmentId}/experiments/${experimentKey}`,
    { signal },
  )
  return data
}

/**
 * Per-context-type experiment results envelope.
 *
 * Returns the Phase 7 shape:
 *   `{ results_by_context_type: { [ct]: { variants, srm, guardrails } }, bound_target, pre_period_days }`.
 *
 * Errors:
 *   404 — experiment not found
 *   409 — no completed iteration yet (computed_at is null on the latest iteration)
 */
export async function getExperimentResults(
  environmentId: string,
  experimentKey: string,
  signal?: AbortSignal,
): Promise<ExperimentResults> {
  const { data } = await api.get<ExperimentResults>(
    `/v1/environments/${environmentId}/experiments/${experimentKey}/results`,
    { signal },
  )
  return data
}

/**
 * Paginated list of first-exposure rows for an experiment, scoped to one
 * context type. Each row represents the moment a context was first assigned
 * to a variant (sticky bucket).
 *
 * `contextType` is required by the gateway — passing a value not in the
 * experiment's `unit_context_types` returns an empty page.
 */
export async function listExperimentExposures(
  environmentId: string,
  experimentKey: string,
  contextType: string,
  params?: { page?: number; per_page?: number },
  signal?: AbortSignal,
): Promise<PaginatedExposures> {
  const qs = new URLSearchParams({ context_type: contextType })
  if (params?.page != null) qs.set('page', String(params.page))
  if (params?.per_page != null) qs.set('per_page', String(params.per_page))
  const { data } = await api.get<PaginatedExposures>(
    `/v1/environments/${environmentId}/experiments/${experimentKey}/exposures?${qs}`,
    { signal },
  )
  return data
}

/**
 * Daily per-variant time-series for a single metric on an experiment, scoped
 * to one context type. `days` is the trailing window — typically 14 or 30.
 */
export async function getExperimentTimeseries(
  environmentId: string,
  experimentKey: string,
  params: { metric_id: string; context_type: string; days: number },
  signal?: AbortSignal,
): Promise<Timeseries> {
  const qs = new URLSearchParams({
    metric_id: params.metric_id,
    context_type: params.context_type,
    days: String(params.days),
  })
  const { data } = await api.get<Timeseries>(
    `/v1/environments/${environmentId}/experiments/${experimentKey}/timeseries?${qs}`,
    { signal },
  )
  return data
}

/**
 * Raw JSON row shape returned by
 * `GET /v1/environments/{env}/experiments/{exp}/iterations`. Mirrors the
 * gateway's `IterationJson` 1:1 — the IterationsTab consumes a wider
 * `IterationSummary` shape, so callers map via [`mapIterationRow`] before
 * passing to the UI.
 */
export interface IterationJsonRow {
  id: string
  experiment_id: string
  iteration_number: number
  started_at_ms: number
  ended_at_ms: number
  traffic_allocation: number
  /** Snapshot of `unit_context_types` at iteration start. */
  unit_context_types: string[]
}

/**
 * Iteration row in the shape the admin IterationsTab consumes
 * (`IterationSummary` keeps timestamps ISO + nullable). Exposed so callers can
 * type their state without importing the tab module.
 */
export interface IterationSummaryDto {
  iteration_id: string
  iteration_number: number
  started_at: string
  ended_at: string | null
  unit_context_types: string[]
  /**
   * Per-iteration "stats last computed at" timestamp. Currently null — the
   * `experiment_iterations` table does not track this column yet. Reserved for
   * a future migration that surfaces stats-schedule data per-iteration.
   */
  computed_at: string | null
}

/** Map a raw `IterationJsonRow` from the gateway to the admin's view shape. */
export function mapIterationRow(row: IterationJsonRow): IterationSummaryDto {
  return {
    iteration_id: row.id,
    iteration_number: row.iteration_number,
    started_at: new Date(row.started_at_ms).toISOString(),
    // `ended_at_ms == 0` is the proto sentinel for "still running".
    ended_at:
      row.ended_at_ms === 0 ? null : new Date(row.ended_at_ms).toISOString(),
    unit_context_types: row.unit_context_types,
    computed_at: null,
  }
}

/**
 * Paginated list of iterations for an experiment. Drives the Iterations tab in
 * the admin's experiment-detail page.
 *
 * The gateway returns rows in the proto-millis shape; callers typically push
 * the result through [`mapIterationRow`] before handing to the UI. This
 * wrapper returns both the mapped + raw forms so callers can pick whichever
 * is more convenient.
 */
export async function listExperimentIterations(
  environmentId: string,
  experimentKey: string,
  params?: { page?: number; per_page?: number },
  signal?: AbortSignal,
): Promise<{
  items: IterationSummaryDto[]
  total: number
  page: number
  per_page: number
}> {
  const qs = new URLSearchParams()
  if (params?.page != null) qs.set('page', String(params.page))
  if (params?.per_page != null) qs.set('per_page', String(params.per_page))
  const suffix = qs.toString() ? `?${qs}` : ''
  const { data } = await api.get<{
    items: IterationJsonRow[]
    total: number
    page: number
    per_page: number
  }>(
    `/v1/environments/${environmentId}/experiments/${experimentKey}/iterations${suffix}`,
    { signal },
  )
  return {
    items: data.items.map(mapIterationRow),
    total: data.total,
    page: data.page,
    per_page: data.per_page,
  }
}

/** Response envelope shared by `POST /recompute` and `GET /recompute/{job_id}`. */
export interface RecomputeJobResponse {
  job_id: string
  /** "queued" | "running" | "succeeded" | "failed" — string typed to allow future variants without UI breakage. */
  status: string
  started_at?: string | null
  finished_at?: string | null
  /** Populated on `failed`; null otherwise. */
  error?: string | null
}

/**
 * Triggers an out-of-band recompute job for the experiment via the stats
 * service. The gateway returns immediately with `{ job_id, status: "queued" }`;
 * callers poll `getRecomputeStatus` to observe progress.
 */
export async function triggerExperimentRecompute(
  environmentId: string,
  experimentKey: string,
): Promise<RecomputeJobResponse> {
  const { data } = await api.post<RecomputeJobResponse>(
    `/v1/environments/${environmentId}/experiments/${experimentKey}/recompute`,
    {},
  )
  return data
}

/** Polls the status of a previously-triggered recompute job. */
export async function getRecomputeStatus(
  environmentId: string,
  experimentKey: string,
  jobId: string,
  signal?: AbortSignal,
): Promise<RecomputeJobResponse> {
  const { data } = await api.get<RecomputeJobResponse>(
    `/v1/environments/${environmentId}/experiments/${experimentKey}/recompute/${jobId}`,
    { signal },
  )
  return data
}

// ─── Flag default-rule distribution ───────────────────────────────────────────

/**
 * Sets (or clears) the default-rule percentage distribution on a flag.
 *
 * Pass a `RolloutDistribution` to enable a percentage fallthrough; pass `null`
 * to revert to single-variant default behaviour.
 *
 * Errors:
 *   409 `FLAG_LOCKED_BY_EXPERIMENT` — an experiment is currently bound to this
 *     flag (running or paused); the distribution cannot change until the
 *     experiment stops.
 *   422 — invalid distribution (percentages don't sum to 100, unknown
 *     variant_key, empty allocations).
 */
export async function setDefaultRuleDistribution(
  projectId: string,
  flagKey: string,
  distribution: RolloutDistribution | null,
): Promise<void> {
  await api.post(
    `/v1/projects/${projectId}/flags/${flagKey}/default-rule-distribution`,
    { distribution },
  )
}

// ─── Context intelligence ─────────────────────────────────────────────────────

export interface ContextTypeSuggestion {
  context_type: string
  last_seen_at: string
}

export interface ContextParamSuggestion {
  param_key: string
  inferred_type: string
  is_private: boolean
  last_seen_at: string
}

export async function listContextTypes(
  environmentId: string,
  signal?: AbortSignal,
): Promise<ContextTypeSuggestion[]> {
  const { data } = await api.get<ContextTypeSuggestion[]>(
    `/v1/environments/${environmentId}/context-types`,
    { signal },
  )
  return data
}

export async function listContextParams(
  environmentId: string,
  contextType: string,
  signal?: AbortSignal,
): Promise<ContextParamSuggestion[]> {
  const { data } = await api.get<ContextParamSuggestion[]>(
    `/v1/environments/${environmentId}/context-types/${contextType}/params`,
    { signal },
  )
  return data
}

// ─── Scheduled changes (flag_lifecycle_20260604, Phase 8) ─────────────────────
//
// Environment-scoped CRUD over scheduled changes for flags/segments/experiments.
// A change targets `(entity_type, entity_id, env_id)` + a mutation payload + a
// schedule spec. See `crates/stitchd-gateway/src/routes/schedules.rs`.

/** List scheduled changes for one entity in an environment. */
export async function listSchedules(
  environmentId: string,
  entityKind: LifecycleEntityKind,
  entityId: string,
  signal?: AbortSignal,
): Promise<ScheduledChange[]> {
  const { data } = await api.get<ScheduledChange[]>(
    `/v1/environments/${environmentId}/${entityKind}/${entityId}/schedules`,
    { signal },
  )
  return data
}

/** Get a single scheduled change (with run history). */
export async function getSchedule(
  environmentId: string,
  scheduleId: string,
  signal?: AbortSignal,
): Promise<ScheduledChange> {
  const { data } = await api.get<ScheduledChange>(
    `/v1/environments/${environmentId}/schedules/${scheduleId}`,
    { signal },
  )
  return data
}

/** Create a scheduled change for one entity. Returns the created change (201). */
export async function createSchedule(
  environmentId: string,
  entityKind: LifecycleEntityKind,
  entityId: string,
  body: CreateScheduleBody,
): Promise<ScheduledChange> {
  const { data } = await api.post<ScheduledChange>(
    `/v1/environments/${environmentId}/${entityKind}/${entityId}/schedules`,
    body,
  )
  return data
}

/** Cancel a pending one-shot schedule (optimistic `version`). */
export async function cancelSchedule(
  environmentId: string,
  scheduleId: string,
  version: number,
): Promise<ScheduledChange> {
  const { data } = await api.post<ScheduledChange>(
    `/v1/environments/${environmentId}/schedules/${scheduleId}/cancel`,
    { version },
  )
  return data
}

/** Pause a recurring schedule (optimistic `version`). */
export async function pauseSchedule(
  environmentId: string,
  scheduleId: string,
  version: number,
): Promise<ScheduledChange> {
  const { data } = await api.post<ScheduledChange>(
    `/v1/environments/${environmentId}/schedules/${scheduleId}/pause`,
    { version },
  )
  return data
}

/** Resume a paused recurring schedule (optimistic `version`). */
export async function resumeSchedule(
  environmentId: string,
  scheduleId: string,
  version: number,
): Promise<ScheduledChange> {
  const { data } = await api.post<ScheduledChange>(
    `/v1/environments/${environmentId}/schedules/${scheduleId}/resume`,
    { version },
  )
  return data
}

// ─── Flag prerequisites (flag_lifecycle_20260604, Phase 8) ────────────────────
//
// Project-scoped. PUT replaces the full prerequisite set + fallback variant.
// A cycle ⇒ HTTP 400 (message names the cycle path);
// flag locked by an experiment ⇒ HTTP 409.
// See `crates/stitchd-gateway/src/routes/flags.rs`.

/** Read a flag's prerequisite gate (deps + fallback variant). */
export async function getPrerequisites(
  projectId: string,
  flagKey: string,
  signal?: AbortSignal,
): Promise<PrerequisiteGate> {
  const { data } = await api.get<PrerequisiteGate>(
    `/v1/projects/${projectId}/flags/${flagKey}/prerequisites`,
    { signal },
  )
  return data
}

/** Replace a flag's prerequisite gate. Throws on 400 (cycle) / 409 (locked). */
export async function setPrerequisites(
  projectId: string,
  flagKey: string,
  body: SetPrerequisitesBody,
): Promise<PrerequisiteGate> {
  const { data } = await api.put<PrerequisiteGate>(
    `/v1/projects/${projectId}/flags/${flagKey}/prerequisites`,
    body,
  )
  return data
}

// ─── Dependency graph (flag_lifecycle_20260604, Phase 8) ──────────────────────
//
// Project-scoped read of upstream prerequisites + downstream dependents for a
// flag / segment / experiment. See `crates/stitchd-gateway/src/routes/dependencies.rs`.

/** Get the upstream + downstream dependency graph for one entity. */
export async function getDependencies(
  projectId: string,
  entityKind: LifecycleEntityKind,
  entityId: string,
  signal?: AbortSignal,
): Promise<DependencyGraph> {
  const { data } = await api.get<DependencyGraph>(
    `/v1/projects/${projectId}/${entityKind}/${entityId}/dependencies`,
    { signal },
  )
  return data
}
