import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { createOrg, listOrgs } from '../../lib/api'
import type { OrgSummary } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'

export function OrgsList() {
  const navigate = useNavigate()
  const [orgs, setOrgs] = useState<OrgSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    listOrgs(controller.signal)
      .then(setOrgs)
      .catch((err: unknown) => {
        if ((err as { name?: string }).name !== 'CanceledError') setError('Failed to load organisations')
      })
      .finally(() => setLoading(false))
    return () => controller.abort()
  }, [])

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault()
    if (!name.trim()) return
    setCreating(true)
    setError(null)
    try {
      await createOrg(name.trim())
      setName('')
      setShowForm(false)
      // Re-fetch the list after creating
      const updated = await listOrgs()
      setOrgs(updated)
    } catch {
      setError('Failed to create organisation')
    } finally {
      setCreating(false)
    }
  }

  return (
    <>
      <PageHeader
        crumbs={['Superadmin']}
        title="Organisations"
        subtitle="Manage customer organisations on the Stitchd platform"
        actions={
          <button
            className="btn primary"
            onClick={() => { setShowForm((v) => !v); setError(null) }}
          >
            <I.plus size={14} /> New Organisation
          </button>
        }
      />

      <div className="page-body">
        {/* Inline create form */}
        {showForm && (
          <div className="card" style={{ marginBottom: 16 }}>
            <div className="card-header">
              <span className="card-title"><I.home size={13} /> Create Organisation</span>
            </div>
            <div className="card-body">
              <form onSubmit={handleCreate} style={{ display: 'flex', gap: 10, alignItems: 'flex-end' }}>
                <div style={{ flex: 1 }}>
                  <label className="label">Organisation name</label>
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
                <button className="btn" type="button" onClick={() => { setShowForm(false); setError(null) }} disabled={creating}>
                  Cancel
                </button>
              </form>
              {error && (
                <div style={{ marginTop: 10 }}>
                  <ErrorBanner message={error} onDismiss={() => setError(null)} />
                </div>
              )}
            </div>
          </div>
        )}

        {loading ? (
          <div className="card">
            {[...Array(3)].map((_, i) => (
              <div key={i} style={{ padding: '14px 16px', borderBottom: '1px solid var(--border-faint)', display: 'flex', gap: 12, alignItems: 'center' }}>
                <div className="skel" style={{ width: 28, height: 28, borderRadius: 6, flexShrink: 0 }} />
                <div style={{ flex: 1 }}>
                  <div className="skel" style={{ width: 140, marginBottom: 6 }} />
                  <div className="skel" style={{ width: 220, height: 10 }} />
                </div>
                <div className="skel" style={{ width: 60, height: 26, borderRadius: 6 }} />
              </div>
            ))}
          </div>
        ) : orgs.length === 0 ? (
          <EmptyState
            icon={<I.home size={20} />}
            title="No organisations yet"
            desc="Create your first organisation to onboard a customer team."
            action={<button className="btn primary" onClick={() => setShowForm(true)}><I.plus size={14} /> New Organisation</button>}
          />
        ) : (
          <div className="card">
            <div className="card-header">
              <span className="card-title">
                <I.home size={13} /> All Organisations
              </span>
              <span className="badge">
                {orgs.length} {orgs.length === 1 ? 'org' : 'orgs'}
              </span>
            </div>
            <table className="table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>ID</th>
                  <th>Created</th>
                  <th style={{ width: 80 }} />
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
                      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                        <div
                          className="org-avatar"
                          style={{ width: 28, height: 28, fontSize: 11, borderRadius: 6, flexShrink: 0 }}
                        >
                          {org.org_name.slice(0, 2).toUpperCase()}
                        </div>
                        <span style={{ fontWeight: 600, fontSize: 13 }}>{org.org_name}</span>
                      </div>
                    </td>
                    <td><span className="mono-key">{org.org_id}</span></td>
                    <td style={{ color: 'var(--fg-muted)', fontSize: 12, fontFamily: 'var(--font-mono)' }}>
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
    </>
  )
}
