import { useState, useEffect } from 'react'
import { useNavigate, useParams, Link } from 'react-router-dom'
import { api } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { auth } from '../../lib/auth'
import type { StoredOrg } from './OrgsList'

// ── Helpers ───────────────────────────────────────────────────────────────────
const ORGS_KEY = 'stitchd_admin_orgs'

function getStoredOrg(orgId: string): StoredOrg | null {
  try {
    const raw = localStorage.getItem(ORGS_KEY)
    if (!raw) return null
    const orgs = JSON.parse(raw) as StoredOrg[]
    return orgs.find((o) => o.org_id === orgId) ?? null
  } catch {
    return null
  }
}

// ── Component ─────────────────────────────────────────────────────────────────

export function OrgDetail() {
  const { orgId } = useParams<{ orgId: string }>()
  const navigate = useNavigate()
  const [org, setOrg] = useState<StoredOrg | null>(null)
  const [loginLoading, setLoginLoading] = useState(false)
  const [loginError, setLoginError] = useState<string | null>(null)

  useEffect(() => {
    if (orgId) setOrg(getStoredOrg(orgId))
  }, [orgId])

  /** Re-issue a login with the target org_id so the UI switches context */
  async function handleLoginAsOrg() {
    if (!orgId) return
    setLoginLoading(true)
    setLoginError(null)

    // We need credentials to re-authenticate as an org user.
    // Superadmin cannot directly "become" an org user without org-user credentials;
    // redirect to the SeedUser page which lets you create a user first.
    try {
      // Try to get a token scoped to this org — the server will reject if the
      // current superadmin JWT isn't a member of that org.  In that case, the
      // admin should seed a user first and log in as them.
      const { data: res } = await api.post<{ access_token: string; org_id: string; user_id: string }>(
        '/v1/auth/login',
        { org_id: orgId },
      )
      const isSystem = auth.decodeIsSystem(res.access_token)
      auth.setSession({ token: res.access_token, orgId: res.org_id, isSystem, userId: res.user_id })
      if (org) auth.addOrgToHistory({ orgId: org.org_id, orgName: org.org_name })
      navigate(`/org/${res.org_id}`)
    } catch {
      setLoginError(
        'Cannot switch to this org directly — please seed an org user first, then log in with those credentials.',
      )
    } finally {
      setLoginLoading(false)
    }
  }

  if (!org) {
    return (
      <div className="page-content">
        <PageHeader title="Organisation" />
        <div className="empty-state">
          <div className="empty-title">Organisation not found in local cache</div>
          <div className="empty-sub">
            The org may have been created in another session. Go back to the orgs list to see all cached organisations.
          </div>
          <button className="btn" style={{ marginTop: 16 }} onClick={() => navigate('/superadmin/orgs')}>
            ← Back to Organisations
          </button>
        </div>
      </div>
    )
  }

  return (
    <div className="page-content">
      <PageHeader
        title={org.org_name}
        subtitle={`Organisation ID: ${org.org_id}`}
        actions={
          <div style={{ display: 'flex', gap: 8 }}>
            <Link to={`/superadmin/orgs/${orgId}/users`}>
              <button className="btn primary">
                <I.plus size={14} /> Seed User
              </button>
            </Link>
            <button className="btn" onClick={() => navigate('/superadmin/orgs')}>
              ← Organisations
            </button>
          </div>
        }
      />

      {/* Org overview card */}
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(260px, 1fr))', gap: 16, marginBottom: 24 }}>
        <div className="card" style={{ padding: 20 }}>
          <div style={{ fontSize: 11, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>Organisation ID</div>
          <div className="mono-key" style={{ fontSize: 13 }}>{org.org_id}</div>
        </div>
        <div className="card" style={{ padding: 20 }}>
          <div style={{ fontSize: 11, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>Name</div>
          <div style={{ fontWeight: 600, fontSize: 15 }}>{org.org_name}</div>
        </div>
        <div className="card" style={{ padding: 20 }}>
          <div style={{ fontSize: 11, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 }}>Created</div>
          <div style={{ fontSize: 13 }}>{new Date(org.created_at).toLocaleString()}</div>
        </div>
      </div>

      {/* Actions */}
      <div className="card" style={{ padding: 24 }}>
        <div className="card-title" style={{ marginBottom: 16 }}>Actions</div>

        {/* Seed a user */}
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: '16px 0', borderBottom: '1px solid var(--border)' }}>
          <div>
            <div style={{ fontWeight: 500, marginBottom: 4 }}>Seed a User</div>
            <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
              Create the first (admin) user in this organisation so they can log in.
            </div>
          </div>
          <Link to={`/superadmin/orgs/${orgId}/users`}>
            <button className="btn primary">
              <I.plus size={14} /> Seed User
            </button>
          </Link>
        </div>

        {/* Login as this org */}
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', paddingTop: 16 }}>
          <div>
            <div style={{ fontWeight: 500, marginBottom: 4 }}>Switch to this Org</div>
            <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
              Re-authenticate as an org user and navigate to the org dashboard.
              You must have seeded a user first.
            </div>
            {loginError && (
              <div className="alert error" style={{ marginTop: 8, maxWidth: 460 }}>{loginError}</div>
            )}
          </div>
          <button className="btn" onClick={handleLoginAsOrg} disabled={loginLoading}>
            {loginLoading ? 'Switching…' : <><I.chevronRight size={14} /> Switch</>}
          </button>
        </div>
      </div>
    </div>
  )
}
