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
import { EditEventModal } from './EditEventModal'
import { ArchiveEventModal } from './ArchiveEventModal'

// ── Types ─────────────────────────────────────────────────────────────────────

interface EventDefinitionDetail {
  event_key: string
  /** Set by the gateway response; needed by EditEventModal/Archive to scope
   *  the PATCH/DELETE URL (`?env_id=…`) for org-scoped JWTs. */
  environment_id?: string
  name?: string
  metric_type: string
  description?: string
  schema?: string | null
  archived: boolean
  deleted_at?: string | null
  created_at: string
  updated_at?: string
  /** Optimistic-locking version echoed back to the PATCH so the gateway can
   *  return HTTP 409 on a stale edit. */
  version: number
}

/** Wire shape returned by `GET /v1/events/{key}/firings`. The gateway
 *  ships value + properties as pre-serialised JSON strings (so the proto
 *  contract stays stable across schema evolution) plus the canonical
 *  timestamp pair (`occurred_at` = client wall-clock, `ingested_at` =
 *  server). Multi-context attribution is a flat type→key map matching
 *  the write-side `TrackEvent.contexts` shape. */
interface EventFiring {
  /** Client-side wall-clock timestamp (RFC3339 UTC). */
  occurred_at: string
  /** Server-side ingestion timestamp (RFC3339 UTC). */
  ingested_at: string
  /** Multi-dimensional attribution as type→key. */
  contexts: Record<string, string>
  /** Pre-serialised JSON scalar (`"42"`, `"true"`, `"1.5"`); empty
   *  string for occurrence-only events. */
  value_json: string
  /** Pre-serialised JSON object ("{}" when empty). */
  properties_json: string
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

/** A metric that directly references this event (aggregation event_key or
 *  any funnel step). Ratio metrics are surfaced indirectly via the
 *  aggregations they reference and not included here. */
interface ReferencingMetric {
  id: string
  key: string
  name: string
  kind: string
  goal_direction?: string
}

// ── Helpers (mirrored in EventDetail.test.ts) ────────────────────────────────

function isArchived(event: EventDefinitionDetail): boolean {
  return event.archived === true || event.deleted_at != null
}

function displayName(event: EventDefinitionDetail): string {
  return event.name && event.name.trim() ? event.name : event.event_key
}

/** The gateway ships properties as `"{...}"` (a JSON-encoded object
 *  string, or `"{}"` when empty). Render it raw — admin users want to
 *  read the exact wire payload, not a re-formatted view. */
function formatFiringProperties(rawJson: string | null | undefined): string {
  if (!rawJson || rawJson === '{}' || rawJson.trim() === '') return '—'
  return rawJson
}

/** value_json is `""` for occurrence-only events; otherwise a JSON
 *  scalar like `"42"`, `"true"`, or `"1.5"`. Display as-is so the user
 *  sees the actual wire shape. */
function formatFiringValue(rawJson: string | null | undefined): string {
  if (!rawJson || rawJson.trim() === '') return '—'
  return rawJson
}

function formatFiringTs(iso: string | null | undefined): string {
  if (!iso) return '—'
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString()
}

/** Render the contexts map as `type=key, type=key, …`. Sorts by type
 *  for stable output. Empty map renders as `—`. */
function formatContexts(ctx: Record<string, string> | null | undefined): string {
  if (!ctx) return '—'
  const entries = Object.entries(ctx)
  if (entries.length === 0) return '—'
  return entries
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([t, k]) => `${t}=${k}`)
    .join(', ')
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
  const { orgId, envId } = useOrgContext()
  const { eventKey } = useParams<{ eventKey: string }>()

  const [event, setEvent] = useState<EventDefinitionDetail | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Firings — backed by the firings endpoint (feature-flag-uz3, landed).
  const [firings, setFirings] = useState<EventFiring[] | null>(null)
  const [firingsLoading, setFiringsLoading] = useState(false)
  // Bumped by the TestEventWidget on successful submit so the firings effect
  // re-runs and the log refreshes.
  const [firingsRefreshTick, setFiringsRefreshTick] = useState(0)

  // 14-day daily-count sparkline — backed by the stats endpoint.
  const [stats, setStats] = useState<EventStats | null>(null)
  const [statsLoading, setStatsLoading] = useState(false)

  // Experiments depending on this event
  const [dependents, setDependents] = useState<ExperimentDependent[]>([])
  const [dependentsLoading, setDependentsLoading] = useState(false)

  // Metrics that directly reference this event (back-link from EventDetail
  // to the metrics layer). Powered by GET /v1/metrics?env_id=…&event_key=…
  // which hits MetricRepository::list_referencing_event server-side.
  const [refMetrics, setRefMetrics] = useState<ReferencingMetric[]>([])
  const [refMetricsLoading, setRefMetricsLoading] = useState(false)

  // Edit / Archive modals — wired off the actions panel below.
  const [showEdit, setShowEdit] = useState(false)
  const [showArchive, setShowArchive] = useState(false)

  // ── Load the event itself ─────────────────────────────────────────────────
  useEffect(() => {
    if (!eventKey) return
    const ac = new AbortController()
    setLoading(true)
    setError(null)
    api.get<EventDefinitionDetail>(`/v1/events/${encodeURIComponent(eventKey)}?env_id=${envId ?? ''}`, { signal: ac.signal })
      .then(({ data }) => setEvent(data))
      .catch((err: unknown) => {
        if (ac.signal.aborted) return
        setError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ac.signal.aborted) setLoading(false)
      })
    return () => ac.abort()
  }, [eventKey, envId])

  // ── Load firings — `feature-flag-uz3` landed; data path is real now.
  // The `env_id=` query param is required by the gateway (`/v1/events/.../firings`
  // mirrors the `/v1/metrics` pattern); without it the gateway returns
  // 400 and the UI falls through to the empty state.
  useEffect(() => {
    if (!eventKey || !envId) return
    const ac = new AbortController()
    setFiringsLoading(true)
    api.get<{ firings: EventFiring[] }>(
      `/v1/events/${encodeURIComponent(eventKey)}/firings?env_id=${envId}&limit=${FIRINGS_LIMIT}`,
      { signal: ac.signal },
    )
      .then(({ data }) => setFirings(data.firings ?? []))
      .catch(() => {
        if (!ac.signal.aborted) setFirings(null)
      })
      .finally(() => {
        if (!ac.signal.aborted) setFiringsLoading(false)
      })
    return () => ac.abort()
  }, [eventKey, envId, firingsRefreshTick])

  // ── Load stats — same endpoint pair as firings, lands on the 14-day
  // sparkline. Also gated on a non-null envId.
  useEffect(() => {
    if (!eventKey || !envId) return
    const ac = new AbortController()
    setStatsLoading(true)
    api.get<EventStats>(
      `/v1/events/${encodeURIComponent(eventKey)}/stats?env_id=${envId}&days=${SPARKLINE_DAYS}`,
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
  }, [eventKey, envId])

  // ── Load metrics that reference this event ────────────────────────────────
  useEffect(() => {
    if (!eventKey || !envId) return
    const ac = new AbortController()
    setRefMetricsLoading(true)
    api
      .get<{ items: ReferencingMetric[] }>(
        `/v1/metrics?env_id=${envId}&event_key=${encodeURIComponent(eventKey)}&per_page=200`,
        { signal: ac.signal },
      )
      .then(({ data }) => {
        if (!ac.signal.aborted) setRefMetrics(data.items ?? [])
      })
      .catch(() => {
        if (!ac.signal.aborted) setRefMetrics([])
      })
      .finally(() => {
        if (!ac.signal.aborted) setRefMetricsLoading(false)
      })
    return () => ac.abort()
  }, [eventKey, envId])

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
          <>
            <button className="btn" onClick={() => { void navigator.clipboard.writeText(event.event_key) }}>
              <I.copy size={13} /> Copy key
            </button>
            {!isArchived(event) && (
              <>
                <button className="btn" onClick={() => setShowEdit(true)} data-testid="edit-event-btn">
                  <I.pencil size={13} /> Edit
                </button>
                <button className="btn" onClick={() => setShowArchive(true)} data-testid="archive-event-btn">
                  <I.trash size={13} /> Archive
                </button>
              </>
            )}
          </>
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
                icon={<I.alert size={20} />}
                title="Could not load firings"
                desc="The firings request failed. Check the analytics service logs or try again."
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
                      <th>Contexts</th>
                      <th>Value</th>
                      <th>Properties</th>
                    </tr>
                  </thead>
                  <tbody>
                    {firings.map((f, i) => (
                      <tr key={`${f.occurred_at}-${i}`}>
                        <td style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--fg-muted)' }}>
                          {formatFiringTs(f.occurred_at)}
                        </td>
                        <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                          {formatContexts(f.contexts)}
                        </td>
                        <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                          {formatFiringValue(f.value_json)}
                        </td>
                        <td style={{
                          fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--fg-muted)',
                          maxWidth: 320, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                        }}>
                          {formatFiringProperties(f.properties_json)}
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
          environmentId={envId ?? undefined}
          onSubmitted={() => setFiringsRefreshTick((t) => t + 1)}
        />

        {/* Metrics referencing this event — symmetric with the
            CreateMetricModal's event-key picker. A metric appears here
            if it aggregates on this event_key, or if it's a funnel with
            this event in any step. Ratio metrics are not surfaced
            directly because their event references are transitive. */}
        <div className="card" style={{ marginBottom: 18 }}>
          <div className="card-header">
            <div className="card-title"><I.metric size={14} /> Metrics referencing this event</div>
            <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
              {refMetrics.length === 0
                ? 'No metrics reference this event'
                : `${refMetrics.length} metric${refMetrics.length === 1 ? '' : 's'}`}
            </div>
          </div>
          <div style={{ padding: 0 }}>
            {refMetricsLoading && refMetrics.length === 0 && (
              <div style={{ padding: 24, textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
                Loading…
              </div>
            )}

            {!refMetricsLoading && refMetrics.length === 0 && (
              <EmptyState
                icon={<I.metric size={20} />}
                title="No metrics yet"
                desc="Create a metric that aggregates this event (or includes it in a funnel) and it will appear here."
              />
            )}

            {refMetrics.length > 0 && (
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Key</th>
                      <th>Name</th>
                      <th>Kind</th>
                      <th>Goal</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {refMetrics.map((m) => (
                      <tr
                        key={m.id}
                        className="row-clickable"
                        onClick={() => navigate(`/org/${orgId}/metrics/${m.key}`)}
                        data-testid={`ref-metric-${m.key}`}
                      >
                        <td><span className="mono-key">{m.key}</span></td>
                        <td><span style={{ fontSize: 13 }}>{m.name}</span></td>
                        <td><span className={`type-pill ${m.kind}`}>{m.kind}</span></td>
                        <td style={{ fontSize: 13 }}>
                          {m.goal_direction === 'increase' && '↑'}
                          {m.goal_direction === 'decrease' && '↓'}
                          {m.goal_direction === 'neutral' && '→'}
                          {!m.goal_direction && '—'}
                        </td>
                        <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>

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

      {showEdit && (
        <EditEventModal
          event={{
            event_key: event.event_key,
            environment_id: event.environment_id ?? envId ?? undefined,
            name: event.name,
            metric_type: event.metric_type,
            description: event.description ?? '',
            schema: event.schema ?? null,
            version: event.version,
            created_at: event.created_at,
            updated_at: event.updated_at,
          }}
          onClose={() => setShowEdit(false)}
          onSaved={(updated) => {
            setShowEdit(false)
            // Merge the patched fields back into the page-level event so the
            // header / stat panel / next-edit `version` all reflect the save.
            setEvent({
              ...event,
              name: updated.name ?? event.event_key,
              metric_type: updated.metric_type,
              description: updated.description ?? '',
              schema: typeof updated.schema === 'string'
                ? updated.schema
                : (updated.schema == null ? null : JSON.stringify(updated.schema)),
              version: updated.version,
              updated_at: updated.updated_at,
            })
          }}
        />
      )}

      {showArchive && (
        <ArchiveEventModal
          eventKey={event.event_key}
          onClose={() => setShowArchive(false)}
          onArchived={() => {
            setShowArchive(false)
            navigate(`/org/${orgId}/events`)
          }}
        />
      )}
    </>
  )
}
