/**
 * EventDetail — `/org/:orgId/events/:eventKey`.
 *
 * Per-event detail page reached by clicking a row in the Events list
 * (Phase 6 Task 1 — merged). Composed of:
 *
 *   1. Header: event_key (mono), name (fallback to key), metric_type chip,
 *      archived badge (when `archived` flag or `deleted_at` is set).
 *   2. Recent firings table (last 50) — calls
 *      `GET /v1/events/{key}/firings?limit=50` (NOT YET IMPLEMENTED — see
 *      bug `feature-flag-uz3`). Renders a skeleton/dash state until the
 *      endpoint lands.
 *   3. 14-day daily-count sparkline — calls
 *      `GET /v1/events/{key}/stats?days=14` (NOT YET IMPLEMENTED — same bug).
 *      Renders a skeleton state until the endpoint lands.
 *   4. Experiments depending on this event — calls
 *      `GET /v1/experiments?metric_event_key={key}` (existing list endpoint
 *      with a new query param). Filtered defensively client-side via
 *      `dependentsForEvent` from the test file's mirror helpers.
 *
 * Pure logic (display helpers, sparkline zero-fill, label pluralisation)
 * lives in `EventDetail.test.ts` mirroring functions — see that file for the
 * full set covered by Vitest.
 *
 * @see feature-flag-7an.6.2  – this task
 * @see feature-flag-uz3       – backend gap: firings + stats endpoints
 */
import { useEffect, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { PageHeader, Sparkline } from '../../components/primitives'
import { I } from '../../components/icons'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import { TestEventWidget } from './TestEventWidget'

// ── Types ─────────────────────────────────────────────────────────────────────

interface EventDefinitionDetail {
  event_key: string
  name?: string
  metric_type: string
  description?: string
  schema?: string | null
  archived: boolean
  deleted_at?: string | null
  created_at: string
}

interface EventFiring {
  ts: string
  context_type: string
  context_key: string
  value?: number | null
  properties?: Record<string, unknown> | null
}

interface EventStatsBucket {
  day: string
  count: number
}

interface EventStats {
  buckets: EventStatsBucket[]
}

interface ExperimentDependent {
  experiment_id: string
  key: string
  name: string
  status: string
}

// ── Helpers (mirrored in EventDetail.test.ts) ────────────────────────────────

function isArchived(event: EventDefinitionDetail): boolean {
  return event.archived === true || event.deleted_at != null
}

function displayName(event: EventDefinitionDetail): string {
  return event.name && event.name.trim() ? event.name : event.event_key
}

function formatFiringProperties(props: Record<string, unknown> | null | undefined): string {
  if (props === null || props === undefined) return '—'
  const keys = Object.keys(props)
  if (keys.length === 0) return '{}'
  return JSON.stringify(props)
}

function formatFiringValue(v: number | null | undefined): string {
  if (v === null || v === undefined) return '—'
  return String(v)
}

function formatFiringTs(iso: string): string {
  return new Date(iso).toLocaleString()
}

function sparklineData(stats: EventStats | null, days: number): number[] {
  if (!stats || stats.buckets.length === 0) return []
  const byDay = new Map<string, number>()
  for (const b of stats.buckets) byDay.set(b.day, b.count)
  const out: number[] = []
  const now = new Date()
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now.getTime() - i * 24 * 60 * 60 * 1000)
    const key = d.toISOString().slice(0, 10)
    out.push(byDay.get(key) ?? 0)
  }
  return out
}

function statsTotal(stats: EventStats | null): number {
  if (!stats) return 0
  return stats.buckets.reduce((sum, b) => sum + b.count, 0)
}

function firingsCountLabel(count: number): string {
  return `${count} firing${count === 1 ? '' : 's'}`
}

function dependentsLabel(count: number): string {
  if (count === 0) return 'No experiments depend on this event'
  return `${count} experiment${count === 1 ? '' : 's'} depend${count === 1 ? 's' : ''} on this event`
}

// ── Page ──────────────────────────────────────────────────────────────────────

const SPARKLINE_DAYS = 14
const FIRINGS_LIMIT = 50

export function EventDetail() {
  const navigate = useNavigate()
  const { orgId } = useOrgContext()
  const { eventKey } = useParams<{ eventKey: string }>()

  const [event, setEvent] = useState<EventDefinitionDetail | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Firings — pending bug feature-flag-uz3
  const [firings, setFirings] = useState<EventFiring[] | null>(null)
  const [firingsLoading, setFiringsLoading] = useState(false)
  // Bumped by the TestEventWidget on successful submit so the firings effect
  // re-runs and the log refreshes.
  const [firingsRefreshTick, setFiringsRefreshTick] = useState(0)

  // Sparkline stats — pending bug feature-flag-uz3
  const [stats, setStats] = useState<EventStats | null>(null)
  const [statsLoading, setStatsLoading] = useState(false)

  // Experiments depending on this event
  const [dependents, setDependents] = useState<ExperimentDependent[]>([])
  const [dependentsLoading, setDependentsLoading] = useState(false)

  // ── Load the event itself ─────────────────────────────────────────────────
  useEffect(() => {
    if (!eventKey) return
    const ac = new AbortController()
    setLoading(true)
    setError(null)
    api.get<EventDefinitionDetail>(`/v1/events/${encodeURIComponent(eventKey)}`, { signal: ac.signal })
      .then(({ data }) => setEvent(data))
      .catch((err: unknown) => {
        if (ac.signal.aborted) return
        setError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ac.signal.aborted) setLoading(false)
      })
    return () => ac.abort()
  }, [eventKey])

  // ── Load firings (best-effort: endpoint may 404 until feature-flag-uz3) ──
  useEffect(() => {
    if (!eventKey) return
    const ac = new AbortController()
    setFiringsLoading(true)
    api.get<{ firings: EventFiring[] }>(
      `/v1/events/${encodeURIComponent(eventKey)}/firings?limit=${FIRINGS_LIMIT}`,
      { signal: ac.signal },
    )
      .then(({ data }) => setFirings(data.firings ?? []))
      .catch(() => {
        // Endpoint not yet implemented (bug feature-flag-uz3) — leave null.
        if (!ac.signal.aborted) setFirings(null)
      })
      .finally(() => {
        if (!ac.signal.aborted) setFiringsLoading(false)
      })
    return () => ac.abort()
  }, [eventKey, firingsRefreshTick])

  // ── Load stats (best-effort, same backend gap) ─────────────────────────────
  useEffect(() => {
    if (!eventKey) return
    const ac = new AbortController()
    setStatsLoading(true)
    api.get<EventStats>(
      `/v1/events/${encodeURIComponent(eventKey)}/stats?days=${SPARKLINE_DAYS}`,
      { signal: ac.signal },
    )
      .then(({ data }) => setStats(data))
      .catch(() => {
        if (!ac.signal.aborted) setStats(null)
      })
      .finally(() => {
        if (!ac.signal.aborted) setStatsLoading(false)
      })
    return () => ac.abort()
  }, [eventKey])

  // ── Load dependent experiments ─────────────────────────────────────────────
  useEffect(() => {
    if (!eventKey) return
    const ac = new AbortController()
    setDependentsLoading(true)
    api.get<{ items: ExperimentDependent[] } | ExperimentDependent[]>(
      `/v1/experiments?metric_event_key=${encodeURIComponent(eventKey)}`,
      { signal: ac.signal },
    )
      .then(({ data }) => {
        const items = Array.isArray(data) ? data : (data.items ?? [])
        setDependents(items)
      })
      .catch(() => {
        if (!ac.signal.aborted) setDependents([])
      })
      .finally(() => {
        if (!ac.signal.aborted) setDependentsLoading(false)
      })
    return () => ac.abort()
  }, [eventKey])

  // ── Render guards ─────────────────────────────────────────────────────────
  if (loading && !event) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <LoadingSpinner label="Loading event…" />
      </div>
    )
  }

  if (error || !event) {
    return (
      <div className="page-body">
        <ErrorBanner
          message={error ?? 'Event not found'}
          icon={<I.alert size={14} />}
        />
      </div>
    )
  }

  const sparkSeries = sparklineData(stats, SPARKLINE_DAYS)

  return (
    <>
      <PageHeader
        crumbs={[
          <a key="1" onClick={() => navigate(`/org/${orgId}/events`)} style={{ cursor: 'pointer' }}>Events</a>,
          event.event_key,
        ]}
        title={displayName(event)}
        mono
        subtitle={event.description ?? undefined}
        badge={
          <>
            <span className={`type-pill ${event.metric_type}`} style={{ fontSize: 11, padding: '2px 7px' }}>
              {event.metric_type}
            </span>
            {isArchived(event) && (
              <span className="badge" style={{ marginLeft: 8 }}>archived</span>
            )}
          </>
        }
        actions={
          <button className="btn" onClick={() => { void navigator.clipboard.writeText(event.event_key) }}>
            <I.copy size={13} /> Copy key
          </button>
        }
      />
      <div className="page-body">
        {/* Stat row */}
        <div className="stat-grid" style={{ marginBottom: 18 }}>
          <div className="stat">
            <div className="stat-label">Metric type</div>
            <div className="stat-value" style={{ fontFamily: 'var(--font-mono)', fontSize: 18 }}>
              {event.metric_type}
            </div>
          </div>
          <div className="stat">
            <div className="stat-label">14-day total</div>
            <div className="stat-value">{statsTotal(stats).toLocaleString()}</div>
          </div>
          <div className="stat" style={{ minWidth: 160 }}>
            <div className="stat-label">14-day trend</div>
            {statsLoading && !stats && (
              <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>Loading…</div>
            )}
            {!statsLoading && sparkSeries.length > 0 && (
              <Sparkline data={sparkSeries} height={36} />
            )}
            {!statsLoading && sparkSeries.length === 0 && (
              <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>—</div>
            )}
          </div>
          <div className="stat">
            <div className="stat-label">Created</div>
            <div className="stat-value" style={{ fontSize: 18 }}>
              {new Date(event.created_at).toLocaleDateString()}
            </div>
          </div>
        </div>

        {/* Recent firings */}
        <div className="card" style={{ marginBottom: 18 }}>
          <div className="card-header">
            <div className="card-title"><I.zap size={14} /> Recent firings</div>
            <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
              {firings === null ? 'No data yet' : firingsCountLabel(firings.length)}
            </div>
          </div>
          <div style={{ padding: 0 }}>
            {firingsLoading && firings === null && (
              <div style={{ padding: 24, textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
                Loading firings…
              </div>
            )}

            {!firingsLoading && firings === null && (
              <EmptyState
                icon={<I.info size={20} />}
                title="Firings endpoint pending"
                desc="The /v1/events/{key}/firings endpoint is tracked in feature-flag-uz3 — firings will appear here once it lands."
              />
            )}

            {firings !== null && firings.length === 0 && (
              <EmptyState
                icon={<I.zap size={20} />}
                title="No firings yet"
                desc={`No events recorded for "${event.event_key}". Track this event from an SDK to populate the firings log.`}
              />
            )}

            {firings !== null && firings.length > 0 && (
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Timestamp</th>
                      <th>Context type</th>
                      <th>Context key</th>
                      <th>Value</th>
                      <th>Properties</th>
                    </tr>
                  </thead>
                  <tbody>
                    {firings.map((f, i) => (
                      <tr key={`${f.ts}-${f.context_key}-${i}`}>
                        <td style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--fg-muted)' }}>
                          {formatFiringTs(f.ts)}
                        </td>
                        <td>{f.context_type}</td>
                        <td><span className="mono-key">{f.context_key}</span></td>
                        <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                          {formatFiringValue(f.value)}
                        </td>
                        <td style={{
                          fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--fg-muted)',
                          maxWidth: 320, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                        }}>
                          {formatFiringProperties(f.properties)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>

        {/* Test-event widget — fire a synthetic event for SDK debugging.
            Per feature-flag-7an.6.4. Slotted between firings + experiments so
            the firings log is right above for visual confirmation. */}
        <TestEventWidget
          eventKey={event.event_key}
          metricType={event.metric_type}
          onSubmitted={() => setFiringsRefreshTick((t) => t + 1)}
        />

        {/* Experiments depending on this event */}
        <div className="card">
          <div className="card-header">
            <div className="card-title"><I.beaker size={14} /> Experiments depending on this event</div>
            <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
              {dependentsLabel(dependents.length)}
            </div>
          </div>
          <div style={{ padding: 0 }}>
            {dependentsLoading && dependents.length === 0 && (
              <div style={{ padding: 24, textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
                Loading…
              </div>
            )}

            {!dependentsLoading && dependents.length === 0 && (
              <EmptyState
                icon={<I.beaker size={20} />}
                title="No experiments use this event"
                desc="When an experiment selects this event as its primary metric, it will appear here."
              />
            )}

            {dependents.length > 0 && (
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Key</th>
                      <th>Name</th>
                      <th>Status</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {dependents.map((d) => (
                      <tr
                        key={d.experiment_id}
                        className="row-clickable"
                        onClick={() => navigate(`/org/${orgId}/experiments/${d.key}`)}
                      >
                        <td><span className="mono-key">{d.key}</span></td>
                        <td><span style={{ fontSize: 13 }}>{d.name}</span></td>
                        <td><span className="badge">{d.status}</span></td>
                        <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>
      </div>
    </>
  )
}
