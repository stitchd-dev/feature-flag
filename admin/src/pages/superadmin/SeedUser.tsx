/**
 * SeedUser — two-mode form for adding users to an organisation.
 *
 * "Add existing" — enter an email that already has a platform account.
 *   The backend finds the user and adds them to the org (no password needed).
 *
 * "Create new" — full form; creates a new platform user and adds them.
 *
 * The same endpoint (POST /v1/admin/orgs/:orgId/users) handles both: if the
 * email is known the backend reuses the existing record; if not it requires a
 * password to create a fresh one.
 */
import { useState, useEffect } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { api, getOrg } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'

interface CreatedUser {
  user_id: string
  email: string
  display_name: string
}

type Mode = 'existing' | 'new'
type OrgRole = 'org_admin' | 'member'

const ROLE_OPTIONS: { value: OrgRole; label: string; desc: string }[] = [
  { value: 'org_admin', label: 'Org Admin', desc: 'Can manage flags, experiments, members' },
  { value: 'member',    label: 'Member',    desc: 'Read-only access' },
]

// ── component ─────────────────────────────────────────────────────────────────

export function SeedUser() {
  const { orgId } = useParams<{ orgId: string }>()
  const navigate = useNavigate()
  const [orgName, setOrgName] = useState<string | null>(null)
  const [mode, setMode] = useState<Mode>('existing')

  // shared
  const [email, setEmail]     = useState('')
  const [orgRole, setOrgRole] = useState<OrgRole>('org_admin')
  // create-new only
  const [displayName, setDisplayName] = useState('')
  const [password, setPassword]       = useState('')
  const [showPw, setShowPw]           = useState(false)

  const [loading, setLoading]   = useState(false)
  const [error, setError]       = useState<string | null>(null)
  const [result, setResult]     = useState<CreatedUser | null>(null)

  useEffect(() => {
    if (!orgId) return
    const controller = new AbortController()
    getOrg(orgId, controller.signal)
      .then((org) => setOrgName(org.org_name))
      .catch(() => { /* non-critical — subtitle stays generic */ })
    return () => controller.abort()
  }, [orgId])

  // reset state when switching mode
  function switchMode(m: Mode) {
    setMode(m)
    setError(null)
    setResult(null)
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!orgId) return
    setLoading(true)
    setError(null)
    setResult(null)

    try {
      const body: Record<string, string> = { email: email.trim(), org_role: orgRole }
      if (mode === 'new') {
        body.display_name = displayName.trim()
        body.password     = password
      }
      const { data } = await api.post<CreatedUser>(`/v1/admin/orgs/${orgId}/users`, body)
      setResult(data)
      setEmail(''); setDisplayName(''); setPassword('')
    } catch (err: unknown) {
      // If backend says "password required" the user doesn't exist → nudge to create-new
      const msg: string = (err as { response?: { data?: { message?: string } } })
        ?.response?.data?.message ?? (err instanceof Error ? err.message : 'Request failed')
      if (mode === 'existing' && msg.toLowerCase().includes('password')) {
        setError(`No platform account found for "${email.trim()}". Switch to Create New to register them first.`)
      } else {
        setError(msg)
      }
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="page-content">
      <PageHeader
        title="Seed User"
        subtitle={orgName ? `Add a user to "${orgName}"` : 'Add a user to this organisation'}
        actions={
          <button className="btn" onClick={() => navigate(`/superadmin/orgs/${orgId}`)}>
            ← Back to Org
          </button>
        }
      />

      {/* Success banner */}
      {result && (
        <div className="card" style={{ padding: 20, marginBottom: 24, border: '1px solid #22c55e', background: '#f0fdf4' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
            <I.check size={18} style={{ color: '#16a34a' }} />
            <span style={{ fontWeight: 600, color: '#16a34a' }}>
              {mode === 'existing' ? 'User added to organisation!' : 'User created and added!'}
            </span>
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', gap: '6px 0', fontSize: 13 }}>
            <span style={{ color: 'var(--fg-muted)' }}>User ID</span>
            <span className="mono-key">{result.user_id}</span>
            <span style={{ color: 'var(--fg-muted)' }}>Email</span>
            <span>{result.email}</span>
            <span style={{ color: 'var(--fg-muted)' }}>Name</span>
            <span>{result.display_name}</span>
          </div>
          <div style={{ marginTop: 14, display: 'flex', gap: 8 }}>
            <button className="btn primary sm" onClick={() => navigate(`/superadmin/orgs/${orgId}`)}>
              Back to Org
            </button>
            <button className="btn sm" onClick={() => setResult(null)}>
              Add Another
            </button>
          </div>
        </div>
      )}

      {!result && (
        <div className="card" style={{ padding: 24, maxWidth: 540 }}>

          {/* Mode tabs */}
          <div style={{ display: 'flex', gap: 0, marginBottom: 24, border: '1px solid var(--border)', borderRadius: 8, overflow: 'hidden' }}>
            {(['existing', 'new'] as Mode[]).map((m) => (
              <button
                key={m}
                onClick={() => switchMode(m)}
                style={{
                  flex: 1, padding: '9px 0', fontSize: 13, fontWeight: 500,
                  background: mode === m ? 'var(--accent)' : 'transparent',
                  color: mode === m ? '#fff' : 'var(--fg-muted)',
                  border: 'none', cursor: 'pointer', transition: 'background 0.15s',
                }}
              >
                {m === 'existing' ? '＋ Add Existing User' : '✦ Create New User'}
              </button>
            ))}
          </div>

          {/* Mode description */}
          <p style={{ fontSize: 12, color: 'var(--fg-muted)', marginBottom: 20, marginTop: -8 }}>
            {mode === 'existing'
              ? 'The user already has a Stitchd account. Enter their email to grant them access to this org.'
              : 'Create a brand-new platform account and add them to this org in one step.'}
          </p>

          <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>

            {/* Email — shared */}
            <div>
              <label className="field-label">Email *</label>
              <input
                className="input"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="user@example.com"
                required
                autoFocus
                disabled={loading}
              />
            </div>

            {/* Create-new only fields */}
            {mode === 'new' && (
              <>
                <div>
                  <label className="field-label">Display Name *</label>
                  <input
                    className="input"
                    type="text"
                    value={displayName}
                    onChange={(e) => setDisplayName(e.target.value)}
                    placeholder="Jane Smith"
                    required
                    disabled={loading}
                  />
                </div>

                <div>
                  <label className="field-label">Password *</label>
                  <div style={{ position: 'relative' }}>
                    <input
                      className="input"
                      type={showPw ? 'text' : 'password'}
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      placeholder="Minimum 8 characters"
                      required
                      minLength={8}
                      disabled={loading}
                      style={{ paddingRight: 40 }}
                    />
                    <button
                      type="button"
                      onClick={() => setShowPw((v) => !v)}
                      style={{ position: 'absolute', right: 10, top: '50%', transform: 'translateY(-50%)', background: 'none', border: 'none', cursor: 'pointer', color: 'var(--fg-muted)' }}
                    >
                      {showPw ? <I.eyeOff size={14} /> : <I.eye size={14} />}
                    </button>
                  </div>
                </div>
              </>
            )}

            {/* Role — shared */}
            <div>
              <label className="field-label">Role in this Organisation</label>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 6 }}>
                {ROLE_OPTIONS.map((opt) => (
                  <label
                    key={opt.value}
                    style={{
                      display: 'flex', alignItems: 'center', gap: 12,
                      padding: '10px 14px', borderRadius: 8, cursor: 'pointer',
                      border: `1px solid ${orgRole === opt.value ? 'var(--accent)' : 'var(--border)'}`,
                      background: orgRole === opt.value ? 'color-mix(in srgb, var(--accent) 6%, transparent)' : 'transparent',
                      transition: 'border-color 0.15s, background 0.15s',
                    }}
                  >
                    <input
                      type="radio"
                      name="orgRole"
                      value={opt.value}
                      checked={orgRole === opt.value}
                      onChange={() => setOrgRole(opt.value)}
                      style={{ accentColor: 'var(--accent)' }}
                    />
                    <div>
                      <div style={{ fontWeight: 500, fontSize: 13 }}>{opt.label}</div>
                      <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{opt.desc}</div>
                    </div>
                  </label>
                ))}
              </div>
            </div>

            {error && <div className="alert error">{error}</div>}

            <div style={{ display: 'flex', gap: 8, paddingTop: 4 }}>
              <button
                className="btn primary"
                type="submit"
                disabled={loading || !email.trim() || (mode === 'new' && (!displayName.trim() || !password))}
                style={{ flex: 1 }}
              >
                {loading
                  ? (mode === 'existing' ? 'Adding…' : 'Creating…')
                  : (mode === 'existing' ? 'Add to Organisation' : 'Create & Add')}
              </button>
              <button
                className="btn"
                type="button"
                onClick={() => navigate(`/superadmin/orgs/${orgId}`)}
                disabled={loading}
              >
                Cancel
              </button>
            </div>

          </form>
        </div>
      )}
    </div>
  )
}
