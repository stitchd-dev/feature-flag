import { useState, useEffect } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { api } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import type { StoredOrg } from './OrgsList'

// ── Helpers ───────────────────────────────────────────────────────────────────
const ORGS_KEY = 'stitchd_admin_orgs'

function getStoredOrg(orgId: string): StoredOrg | null {
  try {
    const raw = localStorage.getItem(ORGS_KEY)
    if (!raw) return null
    return (JSON.parse(raw) as StoredOrg[]).find((o) => o.org_id === orgId) ?? null
  } catch {
    return null
  }
}

interface CreatedUser {
  user_id: string
  email: string
  display_name: string
}

// ── Component ─────────────────────────────────────────────────────────────────

export function SeedUser() {
  const { orgId } = useParams<{ orgId: string }>()
  const navigate = useNavigate()

  const [org, setOrg] = useState<StoredOrg | null>(null)

  const [email, setEmail] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [password, setPassword] = useState('')
  const [orgRole, setOrgRole] = useState<'org_admin' | 'member'>('org_admin')
  const [showPassword, setShowPassword] = useState(false)

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [created, setCreated] = useState<CreatedUser | null>(null)

  useEffect(() => {
    if (orgId) setOrg(getStoredOrg(orgId))
  }, [orgId])

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!orgId) return
    setLoading(true)
    setError(null)
    setCreated(null)
    try {
      const { data: res } = await api.post<CreatedUser>(`/v1/admin/orgs/${orgId}/users`, {
        email: email.trim(),
        display_name: displayName.trim(),
        password,
        org_role: orgRole,
      })
      setCreated(res)
      setEmail('')
      setDisplayName('')
      setPassword('')
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to create user')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="page-content">
      <PageHeader
        title="Seed User"
        subtitle={org ? `Create the first user for "${org.org_name}"` : 'Create the first user for this organisation'}
        actions={
          <button className="btn" onClick={() => navigate(`/superadmin/orgs/${orgId}`)}>
            ← Back to Org
          </button>
        }
      />

      {/* Success banner */}
      {created && (
        <div className="card" style={{ padding: 20, marginBottom: 24, border: '1px solid var(--success, #22c55e)', background: 'var(--success-bg, #f0fdf4)' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
            <I.check size={18} style={{ color: 'var(--success, #22c55e)' }} />
            <span style={{ fontWeight: 600, color: 'var(--success, #16a34a)' }}>User created successfully!</span>
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', gap: '6px 0', fontSize: 13 }}>
            <span style={{ color: 'var(--fg-muted)' }}>User ID</span>
            <span className="mono-key">{created.user_id}</span>
            <span style={{ color: 'var(--fg-muted)' }}>Email</span>
            <span>{created.email}</span>
            <span style={{ color: 'var(--fg-muted)' }}>Name</span>
            <span>{created.display_name}</span>
          </div>
          <div style={{ marginTop: 14, display: 'flex', gap: 8 }}>
            <button
              className="btn primary sm"
              onClick={() => navigate(`/superadmin/orgs/${orgId}`)}
            >
              Back to Org
            </button>
            <button className="btn sm" onClick={() => setCreated(null)}>
              Create Another User
            </button>
          </div>
        </div>
      )}

      {/* Form */}
      {!created && (
        <div className="card" style={{ padding: 24, maxWidth: 520 }}>
          <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
            <div>
              <label className="field-label">Email *</label>
              <input
                className="input"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="user@example.com"
                required
                disabled={loading}
                autoFocus
              />
            </div>

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
                  type={showPassword ? 'text' : 'password'}
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
                  onClick={() => setShowPassword((v) => !v)}
                  style={{ position: 'absolute', right: 10, top: '50%', transform: 'translateY(-50%)', background: 'none', border: 'none', cursor: 'pointer', color: 'var(--fg-muted)' }}
                >
                  {showPassword ? <I.eyeOff size={14} /> : <I.eye size={14} />}
                </button>
              </div>
            </div>

            <div>
              <label className="field-label">Org Role</label>
              <select
                className="input"
                value={orgRole}
                onChange={(e) => setOrgRole(e.target.value as 'org_admin' | 'member')}
                disabled={loading}
              >
                <option value="org_admin">org_admin (can manage flags, experiments, users)</option>
                <option value="member">member (read-only)</option>
              </select>
            </div>

            {error && (
              <div className="alert error">{error}</div>
            )}

            <div style={{ display: 'flex', gap: 8, paddingTop: 4 }}>
              <button
                className="btn primary"
                type="submit"
                disabled={loading || !email.trim() || !displayName.trim() || !password}
              >
                {loading ? 'Creating…' : <><I.plus size={14} /> Create User</>}
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
