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

// On 401, clear token and redirect to login
api.interceptors.response.use(
  (r) => r,
  (err) => {
    if (err.response?.status === 401) {
      auth.clearToken()
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
