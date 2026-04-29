import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'

interface SegmentResponse {
  segment_id: string
  environment_id: string
  key: string
  name: string
  description: string
  segment_type: string // "rule_based" | "list_based"
  version: number
  created_at: string
  updated_at: string
}

export function SegmentsList() {
  const navigate = useNavigate()
  const { envId, orgId } = useOrgContext()
  const [search, setSearch] = useState('')
  const [segments, setSegments] = useState<SegmentResponse[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!envId) return
    setLoading(true)
    setError(null)
    api.get<SegmentResponse[]>(`/v1/environments/${envId}/segments`)
      .then(({ data }) => setSegments(data))
      .catch((err) => setError(err?.response?.data?.message ?? err.message ?? 'Failed to load segments'))
      .finally(() => setLoading(false))
  }, [envId])

  const filtered = segments.filter((s) =>
    !search || s.key.includes(search) || s.name.toLowerCase().includes(search.toLowerCase())
  )

  if (!envId) {
    return (
      <>
        <PageHeader
          crumbs={['Segments']}
          title="Segments"
          subtitle="Reusable groups of contexts."
        />
        <div className="page-body">
          <div className="card">
            <div className="empty">
              <div className="empty-icon"><I.segment size={20} /></div>
              <div className="empty-title">No environment selected</div>
              <div className="empty-desc">No environment selected — set an environment ID in environments settings</div>
              <button className="btn primary" style={{ marginTop: 8 }} onClick={() => navigate(`/org/${orgId}/environments`)}>
                Go to Environments
              </button>
            </div>
          </div>
        </div>
      </>
    )
  }

  return (
    <>
      <PageHeader
        crumbs={['Segments']}
        title="Segments"
        subtitle="Reusable groups of contexts. Rule-based segments evaluate against context fields. List-based segments are explicit include/exclude lists."
        actions={
          <>
            <button className="btn"><I.upload size={13} /> Import list</button>
            <button className="btn primary"><I.plus size={14} /> New segment</button>
          </>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading segments…</span>
          </div>
        )}

        {error && !loading && (
          <div style={{ padding: '12px 16px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 8, color: 'var(--danger)', fontSize: 13, marginBottom: 16 }}>
            <I.alert size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
            {error}
          </div>
        )}

        {!loading && !error && (
          <>
            <div style={{ display: 'flex', gap: 12, marginBottom: 16 }}>
              <div className="search-input">
                <I.search size={14} />
                <input
                  className="input"
                  placeholder="Filter segments…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>
              <button className="btn"><I.filter size={13} /> All types</button>
            </div>

            {segments.length === 0 && (
              <div className="card">
                <div className="empty">
                  <div className="empty-icon"><I.segment size={20} /></div>
                  <div className="empty-title">No segments yet</div>
                  <div className="empty-desc">Create your first segment to start grouping contexts for targeting.</div>
                  <button className="btn primary" style={{ marginTop: 8 }}><I.plus size={13} /> New segment</button>
                </div>
              </div>
            )}

            {segments.length > 0 && (
              <div className="card">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Key</th>
                      <th>Type</th>
                      <th>Description</th>
                      <th>Version</th>
                      <th>Updated</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {filtered.map((s) => (
                      <tr key={s.key} className="row-clickable" onClick={() => navigate(`/org/${orgId}/segments/${s.key}`)}>
                        <td>
                          <div>
                            <span className="mono-key">{s.key}</span>
                            <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 2 }}>{s.name}</div>
                          </div>
                        </td>
                        <td><span className={`badge ${s.segment_type === 'rule_based' ? 'info' : ''}`}>{s.segment_type === 'rule_based' ? 'rule-based' : 'list-based'}</span></td>
                        <td style={{ color: 'var(--fg-muted)', fontSize: 12 }}>{s.description || '—'}</td>
                        <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>v{s.version}</td>
                        <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>{new Date(s.updated_at).toLocaleDateString()}</td>
                        <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </>
        )}
      </div>
    </>
  )
}
