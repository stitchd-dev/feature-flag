import { useState, useEffect, useCallback } from 'react'
import { useNavigate } from 'react-router-dom'
import { createOrg, listOrgs } from '../../lib/api'
import type { OrgSummary } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'

export function OrgsList() {
  const navigate = useNavigate()
  const [orgs, setOrgs] = useState<OrgSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const fetchOrgs = useCallback(async () => {
    setLoading(true)
    try {
      setOrgs(await listOrgs())
    } catch {
      setError('Failed to load organisations')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void fetchOrgs() }, [fetchOrgs])

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault()
    if (!name.trim()) return
    setCreating(true)
    setError(null)
    try {
      await createOrg(name.trim())
      setName('')
      setShowForm(false)
      await fetchOrgs()
    } catch {
      setError('Failed to create organisation')
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="page-content">
      <PageHeader
        title="Organisations"
        subtitle="Manage customer organisations on the Stitchd platform"
        actions={
          <button className="btn primary" onClick={() => { setShowForm((v) => !v); setError(null) }}>
            <I.plus size={14} /> New Organisation
          </button>
        }
      />

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
                disabled={creating}
                required
              />
            </div>
            <button className="btn primary" type="submit" disabled={creating || !name.trim()}>
              {creating ? 'Creating…' : 'Create'}
            </button>
            <button className="btn" type="button" onClick={() => setShowForm(false)} disabled={creating}>
              Cancel
            </button>
          </form>
          {error && <div className="alert error" style={{ marginTop: 10 }}>{error}</div>}
        </div>
      )}

      {loading ? (
        <div style={{ fontSize: 13, color: 'var(--fg-muted)', padding: '32px 0', textAlign: 'center' }}>
          Loading organisations…
        </div>
      ) : orgs.length === 0 ? (
        <div className="empty">
          <div className="empty-icon"><I.home size={20} /></div>
          <div className="empty-title">No organisations yet</div>
          <div className="empty-desc">Create your first organisation to get started.</div>
          <button className="btn primary" style={{ marginTop: 8 }} onClick={() => setShowForm(true)}>
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
                    {org.created_at ? new Date(org.created_at).toLocaleDateString() : '—'}
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
