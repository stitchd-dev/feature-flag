import { useState, useEffect } from 'react'
import { useNavigate, useParams, Link } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { auth } from '../../lib/auth'
import { getOrg, listOrgUsers } from '../../lib/api'
import type { OrgSummary, OrgUserSummary } from '../../lib/api'

// ─── Tab type ─────────────────────────────────────────────────────────────────

type Tab = 'overview' | 'users'

// ─── Users tab ────────────────────────────────────────────────────────────────

function UsersTab({ orgId, orgName }: { orgId: string; orgName: string }) {
  const [users, setUsers] = useState<OrgUserSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    listOrgUsers(orgId, controller.signal)
      .then(setUsers)
      .catch((err: unknown) => {
        if ((err as { name?: string }).name !== 'CanceledError') setError('Failed to load users')
      })
      .finally(() => setLoading(false))
    return () => controller.abort()
  }, [orgId])

  return (
    <div className="stack">
      <div className="card">
        <div className="card-header">
          <span className="card-title"><I.users size={13} /> Members of {orgName}</span>
          <Link to={`/superadmin/orgs/${orgId}/users`}>
            <button className="btn primary sm"><I.plus size={12} /> Add User</button>
          </Link>
        </div>

        {loading ? (
          <div style={{ padding: '12px 16px', fontSize: 12, color: 'var(--fg-subtle)' }}>Loading…</div>
        ) : error ? (
          <div style={{ padding: '12px 16px', fontSize: 12, color: 'var(--danger)' }}>{error}</div>
        ) : users.length === 0 ? (
          <div className="empty" style={{ padding: '40px 24px' }}>
            <div className="empty-icon"><I.users size={18} /></div>
            <div className="empty-title">No users yet</div>
            <div className="empty-desc">Seed the first user so they can log in to this organisation.</div>
            <Link to={`/superadmin/orgs/${orgId}/users`}>
              <button className="btn primary" style={{ marginTop: 8 }}>
                <I.plus size={14} /> Add User
              </button>
            </Link>
          </div>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>User</th>
                <th>Email</th>
                <th>Role</th>
                <th>Joined</th>
              </tr>
            </thead>
            <tbody>
              {users.map((u) => (
                <tr key={u.user_id}>
                  <td>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
                      <div className="user-avatar" style={{ width: 28, height: 28, fontSize: 11, flexShrink: 0 }}>
                        {(u.display_name || u.email).slice(0, 2).toUpperCase()}
                      </div>
                      <span style={{ fontWeight: 500, fontSize: 13 }}>
                        {u.display_name || <span style={{ color: 'var(--fg-muted)' }}>—</span>}
                      </span>
                    </div>
                  </td>
                  <td style={{ fontSize: 13, color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>
                    {u.email}
                  </td>
                  <td>
                    <span className={`badge ${u.role === 'org_admin' ? 'accent' : ''}`}>
                      {u.role === 'org_admin' ? 'Admin' : 'Member'}
                    </span>
                  </td>
                  <td style={{ fontSize: 12, color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>
                    {new Date(u.created_at).toLocaleDateString()}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}

// ─── Overview tab ─────────────────────────────────────────────────────────────

function OverviewTab({ org, orgId }: { org: OrgSummary; orgId: string }) {
  const navigate = useNavigate()
  const [showSwitchModal, setShowSwitchModal] = useState(false)

  function confirmSwitch() {
    auth.addOrgToHistory({ orgId: org.org_id, orgName: org.org_name })
    auth.clearSession()
    navigate(`/login?hint_org=${orgId}`)
  }

  return (
    <div className="stack">
      {/* Stat cards */}
      <div className="stat-grid">
        <div className="stat">
          <div className="stat-label">Organisation ID</div>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, fontWeight: 600, marginTop: 6, wordBreak: 'break-all', color: 'var(--fg)' }}>
            {org.org_id}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">Name</div>
          <div style={{ fontFamily: 'var(--font-display)', fontSize: 22, fontWeight: 700, letterSpacing: '-0.02em', marginTop: 4 }}>
            {org.org_name}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">Created</div>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, fontWeight: 500, marginTop: 6, color: 'var(--fg-muted)' }}>
            {org.created_at ? new Date(org.created_at).toLocaleString() : '—'}
          </div>
        </div>
      </div>

      {/* Actions card */}
      <div className="card">
        <div className="card-header">
          <span className="card-title">Actions</span>
        </div>
        <div>
          {/* Add user */}
          <div className="row-between" style={{ padding: '16px 18px', borderBottom: '1px solid var(--border-faint)' }}>
            <div>
              <div style={{ fontWeight: 500, fontSize: 13, marginBottom: 3 }}>Add User</div>
              <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
                Create or add an existing platform user to this organisation.
              </div>
            </div>
            <Link to={`/superadmin/orgs/${orgId}/users`}>
              <button className="btn primary sm"><I.plus size={12} /> Add User</button>
            </Link>
          </div>

          {/* Switch org */}
          <div className="row-between" style={{ padding: '16px 18px' }}>
            <div>
              <div style={{ fontWeight: 500, fontSize: 13, marginBottom: 3 }}>Switch to this Org</div>
              <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
                Sign out of superadmin and re-authenticate as an org member.
              </div>
            </div>
            <button className="btn sm" onClick={() => setShowSwitchModal(true)}>
              <I.chevronRight size={13} /> Switch
            </button>
          </div>
        </div>
      </div>

      {/* Switch modal */}
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
              using an org user account.
            </p>
            <div style={{ display: 'flex', gap: 10 }}>
              <button className="btn primary" style={{ flex: 1 }} onClick={confirmSwitch}>
                Sign out &amp; go to login
              </button>
              <button className="btn" onClick={() => setShowSwitchModal(false)}>Cancel</button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

// ─── Main component ───────────────────────────────────────────────────────────

export function OrgDetail() {
  const { orgId } = useParams<{ orgId: string }>()
  const navigate = useNavigate()
  const [org, setOrg] = useState<OrgSummary | null>(null)
  const [loading, setLoading] = useState(true)
  const [notFound, setNotFound] = useState(false)
  const [tab, setTab] = useState<Tab>('overview')

  useEffect(() => {
    if (!orgId) return
    const controller = new AbortController()
    setLoading(true)
    setNotFound(false)
    getOrg(orgId, controller.signal)
      .then(setOrg)
      .catch((err: unknown) => {
        if ((err as { name?: string }).name !== 'CanceledError') setNotFound(true)
      })
      .finally(() => setLoading(false))
    return () => controller.abort()
  }, [orgId])

  if (loading) {
    return (
      <>
        <PageHeader crumbs={['Superadmin', 'Organisations']} title="Organisation" />
        <div className="page-body">
          <div className="card" style={{ padding: 48, textAlign: 'center' }}>
            <div style={{ fontSize: 13, color: 'var(--fg-muted)' }}>Loading…</div>
          </div>
        </div>
      </>
    )
  }

  if (notFound || !org) {
    return (
      <>
        <PageHeader crumbs={['Superadmin', 'Organisations']} title="Organisation" />
        <div className="page-body">
          <div className="empty">
            <div className="empty-icon"><I.home size={20} /></div>
            <div className="empty-title">Organisation not found</div>
            <div className="empty-desc">This organisation may have been deleted or you may not have access.</div>
            <button className="btn primary" style={{ marginTop: 8 }} onClick={() => navigate('/superadmin/orgs')}>
              ← Back to Organisations
            </button>
          </div>
        </div>
      </>
    )
  }

  return (
    <>
      <PageHeader
        crumbs={[
          <span key="sa" style={{ cursor: 'pointer' }} onClick={() => navigate('/superadmin/orgs')}>Organisations</span>,
        ]}
        title={org.org_name}
        subtitle={`org_id: ${org.org_id}`}
        actions={
          <div className="row">
            <Link to={`/superadmin/orgs/${orgId}/users`}>
              <button className="btn primary"><I.plus size={14} /> Add User</button>
            </Link>
            <button className="btn" onClick={() => navigate('/superadmin/orgs')}>← Orgs</button>
          </div>
        }
      />

      <div className="page-body">
        {/* Tab bar */}
        <div className="tabs">
          <button className={`tab ${tab === 'overview' ? 'active' : ''}`} onClick={() => setTab('overview')}>
            <I.home size={13} /> Overview
          </button>
          <button className={`tab ${tab === 'users' ? 'active' : ''}`} onClick={() => setTab('users')}>
            <I.users size={13} /> Users
          </button>
        </div>

        {tab === 'overview' && <OverviewTab org={org} orgId={orgId!} />}
        {tab === 'users'    && <UsersTab orgId={orgId!} orgName={org.org_name} />}
      </div>
    </>
  )
}
