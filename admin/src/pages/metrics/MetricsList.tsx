import { useState, useMemo } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { Pagination } from '../../components/Pagination'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { usePaginatedList } from '../../hooks/usePaginatedList'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { CreateMetricModal } from './CreateMetricModal'
import { EditMetricModal } from './EditMetricModal'

const PER_PAGE = 50

// ─── Types ───────────────────────────────────────────────────────────────────

/**
 * Mirrors the gateway's `MetricDefinition` JSON shape — the kind-discriminated
 * union from `stitchd-core::metric::MetricDefinition` serialised with
 * `#[serde(flatten)] kind: MetricKind` (tag = "kind").
 */
export interface MetricResponse {
  id: string
  environment_id: string
  key: string
  name: string
  description?: string | null
  kind: 'aggregation' | 'ratio' | 'funnel'
  goal_direction: 'increase' | 'decrease' | 'neutral'
  version: number
  created_at: string
  updated_at: string
  deleted_at?: string | null
  // Kind-specific fields (flattened — only one set is present per row).
  event_key?: string
  aggregator?: string
  on_field?: string | null
  where_clause?: unknown
  numerator_metric_id?: string
  denominator_metric_id?: string
  min_denominator?: number
  steps?: { event_key: string; where_clause?: unknown }[]
  window_seconds?: number
  count_repeats?: boolean
}

interface ListMetricsResponse {
  items: MetricResponse[]
  next_cursor: string | null
}

type KindFilter = 'all' | 'aggregation' | 'ratio' | 'funnel'

// ─── Helpers (mirrored in MetricsList.test.ts) ───────────────────────────────

function kindChipStyle(kind: MetricResponse['kind']): { background: string; color: string; border: string } {
  switch (kind) {
    case 'aggregation':
      return {
        background: 'rgba(59,130,246,0.1)',
        color: '#2563eb',
        border: '1px solid rgba(59,130,246,0.25)',
      }
    case 'ratio':
      return {
        background: 'rgba(168,85,247,0.1)',
        color: '#9333ea',
        border: '1px solid rgba(168,85,247,0.25)',
      }
    case 'funnel':
      return {
        background: 'rgba(34,197,94,0.1)',
        color: '#16a34a',
        border: '1px solid rgba(34,197,94,0.25)',
      }
  }
}

function goalDirectionArrow(d: MetricResponse['goal_direction']): { glyph: string; tooltip: string } {
  switch (d) {
    case 'increase':
      return { glyph: '↑', tooltip: 'Higher is better' }
    case 'decrease':
      return { glyph: '↓', tooltip: 'Lower is better' }
    case 'neutral':
      return { glyph: '→', tooltip: 'Tracking only' }
  }
}

// ─── Page ────────────────────────────────────────────────────────────────────

export function MetricsList() {
  const navigate = useNavigate()
  const { envId, orgId } = useOrgContext()
  const [searchParams, setSearchParams] = useSearchParams()
  const [showCreate, setShowCreate] = useState(false)
  const [editMetric, setEditMetric] = useState<MetricResponse | null>(null)

  const kindFilter: KindFilter = useMemo(() => {
    const k = searchParams.get('kind') ?? 'all'
    return k === 'aggregation' || k === 'ratio' || k === 'funnel' ? k : 'all'
  }, [searchParams])

  const {
    data: metrics,
    loading,
    error,
    hasNext,
    hasPrev,
    onNext,
    onPrev,
    refresh: loadMetrics,
  } = usePaginatedList<MetricResponse>(
    async ({ cursor, limit, signal }) => {
      if (!envId) return { items: [], next_cursor: null }
      const qs = new URLSearchParams({
        env_id: envId,
        limit: String(limit),
      })
      if (cursor != null) qs.set('cursor', cursor)
      if (kindFilter !== 'all') qs.set('kind', kindFilter)
      const { data } = await api.get<ListMetricsResponse>(`/v1/metrics?${qs}`, { signal })
      return { items: data.items ?? [], next_cursor: data.next_cursor ?? null }
    },
    [envId, kindFilter],
    PER_PAGE,
  )

  function setKindFilter(k: KindFilter) {
    setSearchParams((prev) => {
      if (k === 'all') prev.delete('kind')
      else prev.set('kind', k)
      // Reset cursor when the filter changes so we start from the first page.
      prev.delete('cursor')
      return prev
    })
  }

  if (!envId) {
    return (
      <>
        <PageHeader
          crumbs={['Metrics']}
          title="Metrics"
          subtitle="Composable definitions used by experiments."
        />
        <div className="page-body">
          <div className="card">
            <EmptyState
              icon={<I.metric size={20} />}
              title="No environment selected"
              desc="Select an environment to view metrics."
              action={
                <button className="btn primary" onClick={() => navigate(`/org/${orgId}/environments`)}>
                  Go to Environments
                </button>
              }
            />
          </div>
        </div>
      </>
    )
  }

  return (
    <>
      <PageHeader
        crumbs={['Metrics']}
        title="Metrics"
        subtitle="Define aggregation, ratio, and funnel primitives that experiments reference."
        actions={
          <button className="btn primary" onClick={() => setShowCreate(true)}>
            <I.plus size={14} /> New metric
          </button>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <LoadingSpinner label="Loading metrics…" />
          </div>
        )}

        {error && !loading && (
          <ErrorBanner message={error} icon={<I.alert size={14} />} />
        )}

        {!loading && !error && (
          <>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 16, flexWrap: 'wrap' }}>
              {/* Kind filter chips */}
              <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
                {(['all', 'aggregation', 'ratio', 'funnel'] as const).map((k) => (
                  <button
                    key={k}
                    className={`btn sm ${kindFilter === k ? 'active' : ''}`}
                    onClick={() => setKindFilter(k)}
                  >
                    {k}
                  </button>
                ))}
              </div>
            </div>

            {metrics.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.metric size={20} />}
                  title={kindFilter === 'all' ? 'No metrics yet' : `No ${kindFilter} metrics`}
                  desc={
                    kindFilter === 'all'
                      ? 'Create your first metric to start measuring experiments.'
                      : 'Try a different kind filter or create a new metric.'
                  }
                  action={
                    <button className="btn primary" onClick={() => setShowCreate(true)}>
                      <I.plus size={13} /> New metric
                    </button>
                  }
                />
              </div>
            )}

            {metrics.length > 0 && (
              <div className="card">
                <div className="table-wrap">
                  <table className="table">
                    <thead>
                      <tr>
                        <th>Key</th>
                        <th>Name</th>
                        <th>Kind</th>
                        <th>Goal</th>
                        <th>Version</th>
                        <th>Created</th>
                        <th></th>
                      </tr>
                    </thead>
                    <tbody>
                      {metrics.map((m) => {
                        const chip = kindChipStyle(m.kind)
                        const goal = goalDirectionArrow(m.goal_direction)
                        return (
                          <tr
                            key={m.id}
                            className="row-clickable"
                            style={{ cursor: 'pointer' }}
                            onClick={() => setEditMetric(m)}
                          >
                            <td>
                              <span className="mono-key">{m.key}</span>
                            </td>
                            <td>
                              <span style={{ fontWeight: 600, fontSize: 13 }}>{m.name}</span>
                            </td>
                            <td>
                              <span
                                style={{
                                  fontSize: 10,
                                  fontWeight: 600,
                                  padding: '2px 7px',
                                  borderRadius: 4,
                                  textTransform: 'uppercase',
                                  letterSpacing: 0.3,
                                  ...chip,
                                }}
                              >
                                {m.kind}
                              </span>
                            </td>
                            <td>
                              <span
                                title={goal.tooltip}
                                style={{
                                  fontFamily: 'var(--font-mono)',
                                  fontWeight: 600,
                                  color:
                                    m.goal_direction === 'increase'
                                      ? '#16a34a'
                                      : m.goal_direction === 'decrease'
                                        ? '#dc2626'
                                        : 'var(--fg-muted)',
                                  fontSize: 14,
                                }}
                              >
                                {goal.glyph}
                              </span>
                            </td>
                            <td
                              style={{
                                fontFamily: 'var(--font-mono)',
                                fontSize: 12,
                                color: 'var(--fg-muted)',
                              }}
                            >
                              v{m.version}
                            </td>
                            <td
                              style={{
                                color: 'var(--fg-muted)',
                                fontFamily: 'var(--font-mono)',
                                fontSize: 11,
                              }}
                            >
                              {new Date(m.created_at).toLocaleDateString()}
                            </td>
                            <td>
                              <I.chevronRight size={14} stroke="var(--fg-subtle)" />
                            </td>
                          </tr>
                        )
                      })}
                    </tbody>
                  </table>
                </div>
                <Pagination hasPrev={hasPrev} hasNext={hasNext} onPrev={onPrev} onNext={onNext} />
              </div>
            )}
          </>
        )}
      </div>

      {showCreate && envId && (
        <CreateMetricModal
          envId={envId}
          onClose={() => setShowCreate(false)}
          onCreated={() => {
            setShowCreate(false)
            loadMetrics()
          }}
        />
      )}

      {editMetric && (
        <EditMetricModal
          metric={editMetric}
          onClose={() => setEditMetric(null)}
          onSaved={() => {
            setEditMetric(null)
            loadMetrics()
          }}
          onDeleted={() => {
            setEditMetric(null)
            loadMetrics()
          }}
        />
      )}
    </>
  )
}
