import axios from 'axios'
import { auth } from './auth'

export const api = axios.create({
  baseURL: import.meta.env.VITE_API_BASE_URL ?? '/api',
})

// Inject JWT on every request
api.interceptors.request.use((config) => {
  const token = auth.getToken()
  if (token) config.headers.Authorization = `Bearer ${token}`
  return config
})

// On 401, clear session and redirect to login
api.interceptors.response.use(
  (r) => r,
  (err) => {
    if (err.response?.status === 401) {
      auth.clearSession()
      window.location.href = '/login'
    }
    return Promise.reject(err)
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
  const { data } = await api.get<{ sdk_keys: SdkKeySummary[] }>(
    `/v1/management/environments/${environmentId}/sdk-keys`,
  )
  return data.sdk_keys
}

export interface CreateSdkKeyResponse {
  sdk_key_id: string
  raw_key: string
}

export async function createSdkKey(environmentId: string): Promise<CreateSdkKeyResponse> {
  const { data } = await api.post<CreateSdkKeyResponse>(
    `/v1/management/environments/${environmentId}/sdk-keys`,
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
  const { data } = await api.get<{ orgs: OrgSummary[] }>('/v1/admin/orgs', { signal })
  return data.orgs
}

export async function getOrg(orgId: string, signal?: AbortSignal): Promise<OrgSummary> {
  const { data } = await api.get<OrgSummary>(`/v1/admin/orgs/${orgId}`, { signal })
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
  const { data } = await api.get<{ users: OrgUserSummary[] }>(`/v1/admin/orgs/${orgId}/users`, { signal })
  return data.users
}

export async function removeOrgUser(orgId: string, userId: string): Promise<void> {
  await api.delete(`/v1/admin/orgs/${orgId}/users/${userId}`)
}

export async function createOrg(name: string): Promise<OrgSummary> {
  const { data } = await api.post<{ org_id: string; org_name: string; created_at?: string }>(
    '/v1/admin/orgs',
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
  const { data } = await api.post<SeedUserResponse>(`/v1/admin/orgs/${orgId}/users`, body)
  return data
}
