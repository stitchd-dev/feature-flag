import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import type { Segment } from './types'
import { CreateSegmentModal } from './CreateSegmentModal'
import { EditSegmentModal } from './EditSegmentModal'
import { DeleteSegmentModal } from './DeleteSegmentModal'

export function SegmentsList() {
  const navigate = useNavigate()
  const { envId, orgId, projectId } = useOrgContext()
  const [search, setSearch] = useState('')
  const [segments, setSegments] = useState<Segment[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [showCreate, setShowCreate] = useState(false)
  const [editSegment, setEditSegment] = useState<Segment | null>(null)
  const [deleteSegment, setDeleteSegment] = useState<Segment | null>(null)

  const [refreshTick, setRefreshTick] = useState(0)
  function loadSegments() { setRefreshTick((t) => t + 1) }

  useEffect(() => {
    if (!envId) return
    let cancelled = false
    setLoading(true) // eslint-disable-line react-hooks/set-state-in-effect
    setError(null)
    api.get<Segment[]>(`/v1/segments?env_id=${envId}`)
      .then(({ data }) => { if (!cancelled) setSegments(data) })
      .catch((err: unknown) => {
        if (cancelled) return
        const e = err as { response?: { data?: { message?: string } }; message?: string }
        setError(e?.response?.data?.message ?? e?.message ?? 'Failed to load segments')
      })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [envId, refreshTick])

  const filtered = segments.filter((s) =>
    !search ||
    s.name.toLowerCase().includes(search.toLowerCase()) ||
    (s.description ?? '').toLowerCase().includes(search.toLowerCase()) ||
    s.tags.some((t) => t.toLowerCase().includes(search.toLowerCase()))
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
              <div className="empty-desc">Select an environment to view segments.</div>
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
        subtitle="Reusable groups of users defined by rules or explicit lists."
        actions={
          <button className="btn primary" onClick={() => setShowCreate(true)}>
            <I.plus size={14} /> New segment
          </button>
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
            </div>

            {segments.length === 0 && (
              <div className="card">
                <div className="empty">
                  <div className="empty-icon"><I.segment size={20} /></div>
                  <div className="empty-title">No segments yet</div>
                  <div className="empty-desc">Create your first segment to start grouping users for targeting.</div>
                  <button className="btn primary" style={{ marginTop: 8 }} onClick={() => setShowCreate(true)}>
                    <I.plus size={13} /> New segment
                  </button>
                </div>
              </div>
            )}

            {segments.length > 0 && (
              <div className="card">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Description</th>
                      <th>Tags</th>
                      <th>Conditions</th>
                      <th>Created</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {filtered.map((s) => (
                      <tr key={s.id}>
                        <td>
                          <span style={{ fontWeight: 600, fontSize: 13 }}>{s.name}</span>
                        </td>
                        <td style={{ color: 'var(--fg-muted)', fontSize: 12 }}>{s.description || '—'}</td>
                        <td>
                          {s.tags.length > 0 ? (
                            <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                              {s.tags.map((tag) => (
                                <span key={tag} className="badge">{tag}</span>
                              ))}
                            </div>
                          ) : (
                            <span style={{ color: 'var(--fg-faint)', fontSize: 12 }}>—</span>
                          )}
                        </td>
                        <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>{s.condition_count}</td>
                        <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>
                          {new Date(s.created_at).toLocaleDateString()}
                        </td>
                        <td>
                          <div style={{ display: 'flex', gap: 4, justifyContent: 'flex-end' }}>
                            <button
                              className="btn sm"
                              onClick={(e) => { e.stopPropagation(); setEditSegment(s) }}
                              title="Edit segment"
                            >
                              <I.pencil size={12} />
                            </button>
                            <button
                              className="btn sm"
                              style={{ color: 'var(--danger)' }}
                              onClick={(e) => { e.stopPropagation(); setDeleteSegment(s) }}
                              title="Delete segment"
                            >
                              <I.trash size={12} />
                            </button>
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </>
        )}
      </div>

      {showCreate && envId && orgId && projectId && (
        <CreateSegmentModal
          envId={envId}
          orgId={orgId}
          projectId={projectId}
          onClose={() => setShowCreate(false)}
          onCreated={() => { setShowCreate(false); loadSegments() }}
        />
      )}

      {editSegment && (
        <EditSegmentModal
          segment={editSegment}
          onClose={() => setEditSegment(null)}
          onSaved={() => { setEditSegment(null); loadSegments() }}
        />
      )}

      {deleteSegment && (
        <DeleteSegmentModal
          segment={deleteSegment}
          onClose={() => setDeleteSegment(null)}
          onDeleted={() => {
            setSegments((prev) => prev.filter((s) => s.id !== deleteSegment.id))
            setDeleteSegment(null)
          }}
        />
      )}
    </>
  )
}
