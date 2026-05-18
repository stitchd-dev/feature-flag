import { useState, useEffect } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { Pagination } from '../../components/Pagination'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import type { PaginatedResponse } from '../../lib/types'
import type { Segment } from './types'
import { CreateSegmentModal } from './CreateSegmentModal'
import { EditSegmentModal } from './EditSegmentModal'
import { DeleteSegmentModal } from './DeleteSegmentModal'

const PER_PAGE = 50

export function SegmentsList() {
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const { envId, orgId, projectId } = useOrgContext()
  const [search, setSearch] = useState('')
  const [segments, setSegments] = useState<Segment[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [showCreate, setShowCreate] = useState(false)
  const [editSegment, setEditSegment] = useState<Segment | null>(null)
  const [deleteSegment, setDeleteSegment] = useState<Segment | null>(null)

  const page = Math.max(1, Number(searchParams.get('page') ?? 1))

  const [refreshTick, setRefreshTick] = useState(0)
  function loadSegments() { setRefreshTick((t) => t + 1) }

  function onPageChange(p: number) {
    setSearchParams((prev) => { prev.set('page', String(p)); return prev })
  }

  useEffect(() => {
    if (!envId) return
    let cancelled = false
    setLoading(true) // eslint-disable-line react-hooks/set-state-in-effect
    setError(null)
    const qs = new URLSearchParams({ env_id: envId, page: String(page), per_page: String(PER_PAGE) })
    api.get<PaginatedResponse<Segment>>(`/v1/segments?${qs}`)
      .then(({ data }) => {
        if (cancelled) return
        const items = data.items ?? (Array.isArray(data) ? data : [])
        setSegments(items)
        setTotal(data.total ?? items.length)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        const e = err as { response?: { data?: { message?: string } }; message?: string }
        setError(e?.response?.data?.message ?? e?.message ?? 'Failed to load segments')
      })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [envId, refreshTick, page])

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
            <EmptyState
              icon={<I.segment size={20} />}
              title="No environment selected"
              desc="Select an environment to view segments."
              action={<button className="btn primary" onClick={() => navigate(`/org/${orgId}/environments`)}>Go to Environments</button>}
            />
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
            <LoadingSpinner label="Loading segments…" />
          </div>
        )}

        {error && !loading && (
          <ErrorBanner
            message={error}
            icon={<I.alert size={14} />}
            onDismiss={() => setError(null)}
          />
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
                <EmptyState
                  icon={<I.segment size={20} />}
                  title="No segments yet"
                  desc="Create your first segment to start grouping users for targeting."
                  action={<button className="btn primary" onClick={() => setShowCreate(true)}><I.plus size={13} /> New segment</button>}
                />
              </div>
            )}

            {segments.length > 0 && (
              <div className="card">
                <table className="table" style={{ marginBottom: 0 }}>
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
                      <tr
                        key={s.id}
                        style={{ cursor: 'pointer' }}
                        onClick={() => navigate(`/org/${orgId}/segments/${s.id}`)}
                      >
                        <td>
                          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                            <span style={{ fontWeight: 600, fontSize: 13 }}>{s.name}</span>
                            {s.segment_type && (
                              <span style={{
                                fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4,
                                background: s.segment_type === 'list' ? 'rgba(22,163,74,0.1)' : 'rgba(59,130,246,0.1)',
                                color: s.segment_type === 'list' ? '#16a34a' : '#2563eb',
                                border: `1px solid ${s.segment_type === 'list' ? 'rgba(22,163,74,0.25)' : 'rgba(59,130,246,0.25)'}`,
                              }}>
                                {s.segment_type === 'list' ? 'list' : 'rule'}
                              </span>
                            )}
                          </div>
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
                              title="Edit metadata"
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
                <Pagination page={page} perPage={PER_PAGE} total={total} onChange={onPageChange} />
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
