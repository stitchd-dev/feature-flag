import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../lib/api'
import { auth } from '../lib/auth'

interface CallbackResponse {
  access_token: string
  refresh_token: string
  expires_in: number
  user_id: string
  org_id: string
}

export function OidcCallbackPage() {
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const code = searchParams.get('code')
    const state = searchParams.get('state') // provider_id is embedded in state by gateway
    // The gateway callback URL is GET /v1/auth/oidc/{provider_id}/callback?code=…&state=…
    // state param contains provider_id encoded by the gateway
    const exchange = async () => {
      if (!code || !state) {
        setError('Missing code or state in callback URL')
        return
      }
      try {
        const { data } = await api.get<CallbackResponse>(`/v1/auth/oidc/${state}/callback`, { params: { code, state } })
        const isSystem = auth.decodeIsSystem(data.access_token)
        auth.setSession({ token: data.access_token, orgId: data.org_id, isSystem, userId: data.user_id })
        if (isSystem) navigate('/superadmin', { replace: true })
        else navigate(`/org/${data.org_id}`, { replace: true })
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : 'OIDC callback failed')
      }
    }
    void exchange()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  if (error) {
    return (
      <div style={{ minHeight: '100vh', display: 'grid', placeItems: 'center', background: 'var(--bg)' }}>
        <div style={{ textAlign: 'center', color: 'var(--danger)' }}>
          <div style={{ fontFamily: 'var(--font-display)', fontWeight: 800, fontSize: 20, marginBottom: 8 }}>Authentication failed</div>
          <div style={{ fontSize: 13, marginBottom: 16 }}>{error}</div>
          <a href="/login" style={{ color: 'var(--accent)', fontSize: 13 }}>← Back to sign in</a>
        </div>
      </div>
    )
  }

  return (
    <div style={{ minHeight: '100vh', display: 'grid', placeItems: 'center', background: 'var(--bg)' }}>
      <div style={{ textAlign: 'center', color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 13 }}>
        Completing sign in…
      </div>
    </div>
  )
}
