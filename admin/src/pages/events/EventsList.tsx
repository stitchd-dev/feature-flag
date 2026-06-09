/**
 * EventsList — `/org/:orgId/events`.
 *
 * Lists env-scoped event definitions registered via the `CreateEventModal`.
 * Filters: search-by-key/name, metric_type chip-bar, status (active /
 * archived / all). Pagination is URL-driven via `usePaginatedList`
 * (?cursor=<opaque>). Click row → `/org/:orgId/events/:eventKey` (detail page lands
 * in Task 6.2 — for now navigation just changes the URL).
 *
 * Pure logic (filters / pagination math / modal state machine) lives in
 * `EventsList.test.ts` mirroring functions; the component below composes
 * them with the existing admin UI primitives (`<PageHeader>`, `<Table>`-
 * shaped HTML, `<Pagination>`, `<EmptyState>`, `<LoadingSpinner>`,
 * `<ErrorBanner>`).
 *
 * Wire-up: GET `/v1/events` returns a `CursorPage<EventDefinitionListItem>`.
 * `last_fired_at` and `count_24h` come from the analytics-service summary
 * endpoint (Phase 6 Task 5 — surfaced as `null` here until that ships).
 */
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { Pagination } from '../../components/Pagination'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { usePaginatedList } from '../../hooks/usePaginatedList'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import type { CursorPage } from '../../lib/types'
import { CreateEventModal } from './CreateEventModal'

const PER_PAGE = 50

const METRIC_TYPES = ['all', 'count', 'conversion', 'revenue', 'duration', 'numeric', 'custom'] as const
type MetricTypeFilter = typeof METRIC_TYPES[number]

const STATUS_FILTERS = ['active', 'archived', 'all'] as const
type StatusFilter = typeof STATUS_FILTERS[number]

interface EventDefinitionListItem {
  event_key: string
  name?: string
  metric_type: string
  description?: string
  schema?: string | null
  archived: boolean
  /** Mock null until analytics-service summary endpoint ships (Phase 6.5). */
  last_fired_at: string | null
  /** Mock null until analytics-service summary endpoint ships. */
  count_24h: number | null
  created_at: string
}

function formatLastFired(iso: string | null | undefined): string {
  if (!iso) return '—'
  return new Date(iso).toLocaleDateString()
}

function formatCount24h(count: number | null | undefined): string {
  if (count === null || count === undefined) return '—'
  return String(count)
}

export function EventsList() {
  const navigate = useNavigate()
  const { orgId, envId } = useOrgContext()
  const [search, setSearch] = useState('')
  const [metricType, setMetricType] = useState<MetricTypeFilter>('all')
  const [status, setStatus] = useState<StatusFilter>('active')
  const [showCreate, setShowCreate] = useState(false)

  const { data: events, loading, error, hasNext, hasPrev, onNext, onPrev, refresh } = usePaginatedList<EventDefinitionListItem>(
    async ({ cursor, limit, signal }) => {
      const qs = new URLSearchParams({ limit: String(limit) })
      if (cursor != null) qs.set('cursor', cursor)
      // env_id is required by the gateway (`/v1/events?env_id=...`) —
      // mirrors the `/v1/metrics` pattern; admin JWTs are org-scoped, not
      // env-scoped, so we surface the EnvSwitcher selection here.
      if (envId) qs.set('env_id', envId)
      if (status === 'archived') qs.set('include_archived', 'true')
      if (status === 'all') qs.set('include_archived', 'true')
      const { data } = await api.get<CursorPage<EventDefinitionListItem>>(
        `/v1/events?${qs}`,
        { signal },
      )
      const items = data.items ?? (Array.isArray(data) ? data : [])
      return { items, next_cursor: data.next_cursor ?? null }
    },
    [status, envId],
    PER_PAGE,
  )

  // Pure filter pipeline — mirrors EventsList.test.ts helpers.
  const filtered = events.filter((e) => {
    if (search) {
      const q = search.toLowerCase()
      const hit = e.event_key.toLowerCase().includes(q) || (e.name ?? '').toLowerCase().includes(q)
      if (!hit) return false
    }
    if (metricType !== 'all' && e.metric_type !== metricType) return false
    if (status === 'archived' && !e.archived) return false
    if (status === 'active' && e.archived) return false
    return true
  })

  return (
    <>
      <PageHeader
        crumbs={['Events']}
        title="Events"
        subtitle="Unknown event keys are rejected at the gateway."
        actions={
          <button className="btn primary" onClick={() => setShowCreate(true)}>
            <I.plus size={14} /> New event
          </button>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <LoadingSpinner label="Loading events…" />
          </div>
        )}

        {error && !loading && (
          <ErrorBanner message={error} icon={<I.alert size={14} />} />
        )}

        {!loading && !error && (
          <>
            <div style={{ display: 'flex', gap: 12, alignItems: 'center', marginBottom: 8, flexWrap: 'wrap' }}>
              <div className="search-input" style={{ maxWidth: 280 }}>
                <I.search size={14} />
                <input
                  className="input"
                  placeholder="Filter by key, name…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>
              <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
                {METRIC_TYPES.map((t) => (
                  <button
                    key={t}
                    className={`btn sm ${metricType === t ? 'active' : ''}`}
                    onClick={() => setMetricType(t)}
                  >
                    {t}
                  </button>
                ))}
              </div>
              <div style={{ display: 'flex', gap: 4, padding: 3, background: 'var(--bg-sunken)', borderRadius: 8, border: '1px solid var(--border)' }}>
                {STATUS_FILTERS.map((s) => (
                  <button
                    key={s}
                    onClick={() => setStatus(s)}
                    className="btn sm"
                    style={{
                      border: 'none',
                      background: status === s ? 'var(--surface)' : 'transparent',
                      color: status === s ? 'var(--fg)' : 'var(--fg-muted)',
                      boxShadow: status === s ? 'var(--shadow-xs)' : 'none',
                    }}
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>

            {events.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.zap size={20} />}
                  title="No events yet"
                  desc="Register your first event to start tracking metrics and powering experiments."
                  action={<button className="btn primary" onClick={() => setShowCreate(true)}><I.plus size={13} /> New event</button>}
                />
              </div>
            )}

            {events.length > 0 && (
              <div className="card">
                <div className="table-wrap">
                  <table className="table">
                    <thead>
                      <tr>
                        <th>Key</th>
                        <th>Name</th>
                        <th>Metric type</th>
                        <th>Last fired</th>
                        <th>24h count</th>
                        <th>Status</th>
                        <th></th>
                      </tr>
                    </thead>
                    <tbody>
                      {filtered.map((e) => (
                        <tr
                          key={e.event_key}
                          className="row-clickable"
                          onClick={() => navigate(`/org/${orgId}/events/${e.event_key}`)}
                        >
                          <td><span className="mono-key">{e.event_key}</span></td>
                          <td>
                            <span style={{ fontSize: 13 }}>{e.name ?? '—'}</span>
                          </td>
                          <td><span className={`type-pill ${e.metric_type}`}>{e.metric_type}</span></td>
                          <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>
                            {formatLastFired(e.last_fired_at)}
                          </td>
                          <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                            {formatCount24h(e.count_24h)}
                          </td>
                          <td>
                            {e.archived ? (
                              <span className="badge">archived</span>
                            ) : (
                              <span className="badge success">active</span>
                            )}
                          </td>
                          <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <Pagination hasPrev={hasPrev} hasNext={hasNext} onPrev={onPrev} onNext={onNext} />
              </div>
            )}
          </>
        )}
      </div>

      {showCreate && (
        <CreateEventModal
          onClose={() => setShowCreate(false)}
          onCreated={() => { setShowCreate(false); refresh() }}
        />
      )}
    </>
  )
}
