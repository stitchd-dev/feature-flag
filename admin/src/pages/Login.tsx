import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { I } from '../components/icons'
import { StitchdMark } from '../components/primitives'
import { loginWithPassword, initiateOidc, initiateSaml, listUserOrgs } from '../lib/api'
import { auth } from '../lib/auth'

type AuthMethod = 'password' | 'oidc' | 'saml'

const DOCKER_SNIPPET = `$ docker compose up stitchd
✓ stitchd-gateway     :8080
✓ stitchd-flag-service
✓ stitchd-event-service
✓ stitchd-experimentation-service
✓ stitchd-segmentation-service
✓ stitchd-auth-service
─ ready in 1.4s`

export function LoginPage() {
  const navigate = useNavigate()
  const [method, setMethod] = useState<AuthMethod>('password')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [orgId, setOrgId] = useState('')
  const [showPw, setShowPw] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    setError(null)
    setLoading(true)
    try {
      if (method === 'password') {
        const res = await loginWithPassword(email, password, orgId || undefined)
        const isSystem = auth.decodeIsSystem(res.access_token)
        auth.setSession({ token: res.access_token, orgId: res.org_id, isSystem, userId: res.user_id })
        // Fetch org list for seamless switching — non-blocking
        try {
          const orgs = await listUserOrgs()
          auth.setOrgs(orgs)
        } catch {
          auth.setOrgs([])
        }
        if (isSystem) {
          navigate('/superadmin')
        } else {
          auth.addOrgToHistory({ orgId: res.org_id, orgName: res.org_id })
          navigate(`/org/${res.org_id}`)
        }
      } else if (method === 'oidc') {
        const res = await initiateOidc(orgId)
        window.location.href = res.redirect_url
      } else {
        const res = await initiateSaml(orgId)
        window.location.href = res.redirect_url
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Sign in failed'
      setError(msg)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div style={{ minHeight: '100%', display: 'grid', gridTemplateColumns: '1fr 1.05fr', background: 'var(--bg)' }}>
      {/* Left panel */}
      <div style={{ padding: '48px 56px', display: 'flex', flexDirection: 'column', justifyContent: 'space-between', borderRight: '1px solid var(--border)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <StitchdMark size={32} radius={8} />
          <div className="brand-text" style={{ fontSize: 22 }}>Stitchd</div>
        </div>
        <div style={{ maxWidth: 420 }}>
          <div style={{ fontFamily: 'var(--font-display)', fontWeight: 800, fontSize: 36, lineHeight: 1.1, letterSpacing: '-0.025em', marginBottom: 12 }}>
            Feature flags &amp; experiments,<br />self-hosted.
          </div>
          <div style={{ color: 'var(--fg-muted)', fontSize: 15, lineHeight: 1.55, marginBottom: 24 }}>
            Sign in to your organization to manage flags, run rigorous A/B experiments, and ship without flinching.
          </div>
          <div className="code" style={{ fontSize: 11.5 }}>{DOCKER_SNIPPET}</div>
        </div>
        <div style={{ color: 'var(--fg-subtle)', fontSize: 12, fontFamily: 'var(--font-mono)' }}>
          self-hosted · v0.9.0 · 6 services healthy
        </div>
      </div>

      {/* Right panel */}
      <div style={{ display: 'grid', placeItems: 'center', padding: 48 }}>
        <div className="card" style={{ width: 420, padding: 32 }}>
          <div style={{ fontFamily: 'var(--font-display)', fontSize: 22, fontWeight: 800, letterSpacing: '-0.02em', marginBottom: 4 }}>Sign in</div>
          <div style={{ color: 'var(--fg-muted)', fontSize: 13, marginBottom: 20 }}>Use your work email to continue.</div>

          {/* Method tabs */}
          <div style={{ display: 'flex', gap: 6, marginBottom: 18, padding: 3, background: 'var(--bg-sunken)', borderRadius: 8 }}>
            {(['password', 'oidc', 'saml'] as const).map((m) => (
              <button
                key={m}
                onClick={() => { setMethod(m); setError(null) }}
                style={{ flex: 1, padding: '6px 8px', border: 'none', background: method === m ? 'var(--surface)' : 'transparent', borderRadius: 6, fontSize: 12, fontWeight: method === m ? 600 : 500, color: method === m ? 'var(--fg)' : 'var(--fg-muted)', boxShadow: method === m ? 'var(--shadow-xs)' : 'none', cursor: 'pointer' }}
              >
                {m === 'password' ? 'Password' : m === 'oidc' ? 'OIDC / SSO' : 'SAML'}
              </button>
            ))}
          </div>

          <form onSubmit={handleSubmit}>
            <div style={{ marginBottom: 12 }}>
              <label className="label">Email</label>
              <input className="input" type="email" value={email} onChange={(e) => setEmail(e.target.value)} placeholder="you@company.com" required />
            </div>

            {(method === 'oidc' || method === 'saml') && (
              <div style={{ marginBottom: 12 }}>
                <label className="label">Organization ID</label>
                <input className="input mono" value={orgId} onChange={(e) => setOrgId(e.target.value)} placeholder="org_…" required />
                <div className="helper">{method === 'oidc' ? 'Your OIDC provider will be resolved from your org.' : 'Your IdP metadata must be configured by an admin.'}</div>
              </div>
            )}

            {method === 'password' && (
              <div style={{ marginBottom: 12 }}>
                <label className="label">Password</label>
                <div style={{ position: 'relative' }}>
                  <input
                    className="input"
                    type={showPw ? 'text' : 'password'}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="••••••••"
                    style={{ paddingRight: 36 }}
                    required
                  />
                  <button
                    type="button"
                    className="icon-btn"
                    onClick={() => setShowPw(!showPw)}
                    style={{ position: 'absolute', right: 4, top: 4 }}
                  >
                    {showPw ? <I.eyeOff size={14} /> : <I.eye size={14} />}
                  </button>
                </div>
                <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 6 }}>
                  <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, color: 'var(--fg-muted)', cursor: 'pointer' }}>
                    <input type="checkbox" defaultChecked /> Remember me
                  </label>
                  <a style={{ fontSize: 12, color: 'var(--accent)', cursor: 'pointer' }}>Forgot password?</a>
                </div>
              </div>
            )}

            {error && (
              <div style={{ padding: '10px 12px', background: 'var(--danger-bg)', border: '1px solid var(--danger)', borderRadius: 6, color: 'var(--danger)', fontSize: 13, marginBottom: 12, display: 'flex', alignItems: 'center', gap: 8 }}>
                <I.alert size={14} />
                {error}
              </div>
            )}

            <button type="submit" className="btn primary lg" style={{ width: '100%' }} disabled={loading}>
              {loading ? 'Signing in…' : method === 'password' ? 'Sign in' : method === 'oidc' ? 'Continue with OIDC' : 'Continue to IdP'}
              {!loading && <I.arrowRight size={14} />}
            </button>
          </form>

          <div style={{ display: 'flex', alignItems: 'center', gap: 10, margin: '16px 0', color: 'var(--fg-faint)', fontSize: 11 }}>
            <div style={{ flex: 1, height: 1, background: 'var(--border)' }} /> OR <div style={{ flex: 1, height: 1, background: 'var(--border)' }} />
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="btn" style={{ flex: 1 }}><I.github size={14} /> GitHub</button>
            <button className="btn" style={{ flex: 1 }}><I.mail size={14} /> Magic link</button>
          </div>
          <div style={{ marginTop: 18, fontSize: 12, color: 'var(--fg-muted)', textAlign: 'center' }}>
            New here? <a style={{ color: 'var(--accent)', cursor: 'pointer' }}>Ask your admin</a> for an invite.
          </div>
        </div>
      </div>
    </div>
  )
}
