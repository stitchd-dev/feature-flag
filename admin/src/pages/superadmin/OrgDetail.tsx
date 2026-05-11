import { useState, useEffect, useCallback } from 'react'
import { useNavigate, useParams, Link } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { auth } from '../../lib/auth'
import { listOrgs } from '../../lib/api'
import type { OrgSummary } from '../../lib/api'

// ── Component ─────────────────────────────────────────────────────────────────

export function OrgDetail() {
  const { orgId } = useParams<{ orgId: string }>()
  const navigate = useNavigate()
  const [org, setOrg] = useState<OrgSummary | null>(null)
  const [loading, setLoading] = useState(true)
  const [notFound, setNotFound] = useState(false)
  const [showSwitchModal, setShowSwitchModal] = useState(false)

  const fetchOrg = useCallback(async () => {
    if (!orgId) return
    setLoading(true)
    try {
      const orgs = await listOrgs()
      const found = orgs.find((o) => o.org_id === orgId) ?? null
      setOrg(found)
      setNotFound(!found)
    } catch {
      setNotFound(true)
    } finally {
      setLoading(false)
    }
  }, [orgId])

  useEffect(() => { void fetchOrg() }, [fetchOrg])

  /** Clear the superadmin session and go to login so the user re-authenticates as an org member. */
  function confirmSwitch() {
    if (org) auth.addOrgToHistory({ orgId: org.org_id, orgName: org.org_name })
    auth.clearSession()
    // Pass the target org hint so the login page can pre-fill or context-switch
    navigate(`/login?hint_org=${orgId}`)
  }

  if (loading) {
    return (
      <div className="page-content">
        <PageHeader title="Organisation" />
        <div style={{ fontSize: 13, color: 'var(--fg-muted)', padding: '32px 0', textAlign: 'center' }}>
          Loading…
        </div>
      </div>
    )
  }

  if (notFound || !org) {
    return (
      <div className="page-content">
        <PageHeader title="Organisation" />
        <div className="empty">
          <div className="empty-icon"><I.home size={20} /></div>
          <div className="empty-title">Organisation not found</div>
          <div className="empty-desc">This organisation may have been deleted or you may not have access.</div>
          <button className="btn primary" style={{ marginTop: 8 }} onClick={() => navigate('/superadmin/orgs')}>
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
          <div style={{ fontSize: 13 }}>{org.created_at ? new Date(org.created_at).toLocaleString() : '—'}</div>
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

        {/* Switch to org mode */}
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', paddingTop: 16 }}>
          <div>
            <div style={{ fontWeight: 500, marginBottom: 4 }}>Switch to this Org</div>
            <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
              Log out of superadmin and re-authenticate as an org user.
              You must have seeded a user for this org first.
            </div>
          </div>
          <button className="btn" onClick={() => setShowSwitchModal(true)}>
            <I.chevronRight size={14} /> Switch
          </button>
        </div>
      </div>

      {/* Switch confirmation modal */}
      {showSwitchModal && (
        <div
          style={{ position: 'fixed', inset: 0, zIndex: 200, background: 'rgba(0,0,0,0.45)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}
          onClick={() => setShowSwitchModal(false)}
        >
          <div
            style={{ background: 'var(--surface)', borderRadius: 12, padding: 28, width: 420, boxShadow: 'var(--shadow-xl)', border: '1px solid var(--border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 16 }}>
              <div style={{ width: 36, height: 36, borderRadius: 10, background: 'color-mix(in srgb, var(--accent) 12%, transparent)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                <I.lock size={18} style={{ color: 'var(--accent)' }} />
              </div>
              <div>
                <div style={{ fontWeight: 600, fontSize: 16 }}>Switch to Org Mode</div>
                <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>You will be signed out of superadmin</div>
              </div>
            </div>

            <p style={{ fontSize: 13, color: 'var(--fg-muted)', lineHeight: 1.6, marginBottom: 20 }}>
              You are about to leave the superadmin session. To access{' '}
              <strong>{org.org_name}</strong>, you'll need to re-authenticate
              using an org user account — use the credentials of a user you
              seeded into this org.
            </p>

            <div style={{ display: 'flex', gap: 10 }}>
              <button className="btn primary" style={{ flex: 1 }} onClick={confirmSwitch}>
                Sign out &amp; go to login
              </button>
              <button className="btn" onClick={() => setShowSwitchModal(false)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
