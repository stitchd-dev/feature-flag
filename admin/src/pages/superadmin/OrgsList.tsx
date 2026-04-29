import { useState, useEffect, useCallback } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'

// ── Local org store ───────────────────────────────────────────────────────────
// Since there is no GET /v1/admin/orgs endpoint, we cache created orgs in
// localStorage so the superadmin can browse them across page refreshes.
const ORGS_KEY = 'stitchd_admin_orgs'

export interface StoredOrg {
  org_id: string
  org_name: string
  created_at: string
}

function loadOrgs(): StoredOrg[] {
  try {
    const raw = localStorage.getItem(ORGS_KEY)
    return raw ? (JSON.parse(raw) as StoredOrg[]) : []
  } catch {
    return []
  }
}

function saveOrg(org: StoredOrg): void {
  const existing = loadOrgs().filter((o) => o.org_id !== org.org_id)
  localStorage.setItem(ORGS_KEY, JSON.stringify([org, ...existing]))
}

// ── Component ─────────────────────────────────────────────────────────────────

export function OrgsList() {
  const navigate = useNavigate()
  const [orgs, setOrgs] = useState<StoredOrg[]>(() => loadOrgs())
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  // Keep UI in sync with localStorage (in case another tab added an org)
  useEffect(() => {
    setOrgs(loadOrgs())
  }, [])

  const handleCreate = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault()
      if (!name.trim()) return
      setLoading(true)
      setError(null)
      setSuccess(null)
      try {
        const { data: res } = await api.post<{ org_id: string; org_name: string }>('/v1/admin/orgs', {
          name: name.trim(),
        })
        const newOrg: StoredOrg = {
          org_id: res.org_id,
          org_name: res.org_name,
          created_at: new Date().toISOString(),
        }
        saveOrg(newOrg)
        setOrgs(loadOrgs())
        setName('')
        setShowForm(false)
        setSuccess(`Organisation "${res.org_name}" created successfully.`)
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : 'Failed to create organisation')
      } finally {
        setLoading(false)
      }
    },
    [name],
  )

  return (
    <div className="page-content">
      <PageHeader
        title="Organisations"
        subtitle="Manage customer organisations on the Stitchd platform"
        actions={
          <button className="btn primary" onClick={() => { setShowForm((v) => !v); setError(null) }}>
            <I.plus size={14} />
            New Organisation
          </button>
        }
      />

      {/* Create form */}
      {showForm && (
        <div className="card" style={{ marginBottom: 24, padding: 20 }}>
          <div className="card-title" style={{ marginBottom: 12 }}>Create Organisation</div>
          <form onSubmit={handleCreate} style={{ display: 'flex', gap: 12, alignItems: 'flex-end' }}>
            <div style={{ flex: 1 }}>
              <label className="field-label">Organisation Name</label>
              <input
                className="input"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Acme Corp"
                autoFocus
                disabled={loading}
                required
              />
            </div>
            <button className="btn primary" type="submit" disabled={loading || !name.trim()}>
              {loading ? 'Creating…' : 'Create'}
            </button>
            <button className="btn" type="button" onClick={() => setShowForm(false)} disabled={loading}>
              Cancel
            </button>
          </form>
          {error && <div className="alert error" style={{ marginTop: 10 }}>{error}</div>}
        </div>
      )}

      {success && (
        <div className="alert success" style={{ marginBottom: 16 }}>{success}</div>
      )}

      {/* Orgs table */}
      {orgs.length === 0 ? (
        <div className="empty-state">
          <I.home size={32} style={{ opacity: 0.3, marginBottom: 12 }} />
          <div className="empty-title">No organisations yet</div>
          <div className="empty-sub">Create your first organisation to get started</div>
          <button className="btn primary" style={{ marginTop: 16 }} onClick={() => setShowForm(true)}>
            <I.plus size={14} /> New Organisation
          </button>
        </div>
      ) : (
        <div className="card">
          <table className="data-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>ID</th>
                <th>Created</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {orgs.map((org) => (
                <tr
                  key={org.org_id}
                  className="row-clickable"
                  onClick={() => navigate(`/superadmin/orgs/${org.org_id}`)}
                >
                  <td>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                      <div className="org-avatar" style={{ width: 28, height: 28, fontSize: 11, borderRadius: 6 }}>
                        {org.org_name.slice(0, 2).toUpperCase()}
                      </div>
                      <span style={{ fontWeight: 500 }}>{org.org_name}</span>
                    </div>
                  </td>
                  <td><span className="mono-key">{org.org_id}</span></td>
                  <td style={{ color: 'var(--fg-muted)', fontSize: 12 }}>
                    {new Date(org.created_at).toLocaleDateString()}
                  </td>
                  <td>
                    <button
                      className="btn sm"
                      onClick={(e) => { e.stopPropagation(); navigate(`/superadmin/orgs/${org.org_id}`) }}
                    >
                      Manage
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
