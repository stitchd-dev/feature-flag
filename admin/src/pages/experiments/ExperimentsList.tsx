import { useEffect, useMemo, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { usePaginatedList } from '../../hooks/usePaginatedList'
import { useOrgContext } from '../../context/OrgContext'
import { api, listExperimentExposures } from '../../lib/api'
import type { ExperimentResponse } from '../../lib/types'
import { CreateExperimentModal } from './CreateExperimentModal'
import {
  buildMetricNameLookup,
  computeDaysRemaining,
  formatDaysRemaining,
  matchesFlag,
  matchesStatus,
  pickExposureContextType,
  resolvePrimaryMetricLabel,
  STATUS_FILTERS,
  STATUS_FILTER_LABELS,
  statusBadgeClass,
  type StatusFilter,
} from './ExperimentsList.helpers'

interface MetricsListResponse {
  items: { id: string; key: string; name: string }[]
  total: number
}

interface FlagsListResponse {
  items: { flag_id: string; key: string; name: string }[]
  total: number
}

export function ExperimentsList() {
  const navigate = useNavigate()
  const { envId, orgId, projectId } = useOrgContext()
  const [searchParams, setSearchParams] = useSearchParams()
  const [search, setSearch] = useState('')
  const [showCreate, setShowCreate] = useState(false)

  const statusFilter = ((searchParams.get('status') as StatusFilter) ?? 'all') as StatusFilter
  const flagFilter = searchParams.get('flag') // flag key or flag_id

  // ── List + concurrent metric / flag lookups ───────────────────────────

  const { data: experiments, loading, error } = usePaginatedList<ExperimentResponse>(
    async ({ signal }) => {
      if (!envId) return { items: [], total: 0 }
      const { data } = await api.get<{ items: ExperimentResponse[]; total: number }>(
        `/v1/environments/${envId}/experiments`,
        { signal },
      )
      const items = data.items ?? (Array.isArray(data) ? data : [])
      return { items, total: data.total ?? items.length }
    },
    [envId],
  )

  // Metric name lookup (env-scoped) — built once on mount, refreshed when envId changes.
  const [metricLookup, setMetricLookup] = useState<Map<string, string>>(new Map())
  useEffect(() => {
    if (!envId) return
    const ctrl = new AbortController()
    api
      .get<MetricsListResponse>(`/v1/metrics?env_id=${encodeURIComponent(envId)}&limit=500`, {
        signal: ctrl.signal,
      })
      .then(({ data }) => setMetricLookup(buildMetricNameLookup(data.items ?? [])))
      .catch(() => undefined)
    return () => ctrl.abort()
  }, [envId])

  // Flag-key list (env-scoped) for the flag filter dropdown.
  const [flagOptions, setFlagOptions] = useState<{ flag_id: string; key: string; name: string }[]>(
    [],
  )
  useEffect(() => {
    if (!projectId) return
    const ctrl = new AbortController()
    api
      .get<FlagsListResponse>(`/v1/projects/${projectId}/flags?per_page=500`, {
        signal: ctrl.signal,
      })
      .then(({ data }) => setFlagOptions(data.items ?? []))
      .catch(() => undefined)
    return () => ctrl.abort()
  }, [projectId])

  // Exposure count per experiment — keyed by experiment_key. Lazy: only
  // fetches for the currently-visible rows (after filtering).
  const [exposureCount, setExposureCount] = useState<Map<string, number | null>>(new Map())

  // Filter set (the visible rows we want exposure counts for).
  const filtered = useMemo(
    () =>
      experiments.filter(
        (e) =>
          matchesStatus(e, statusFilter) &&
          matchesFlag(e, flagFilter) &&
          (!search ||
            e.name.toLowerCase().includes(search.toLowerCase()) ||
            e.key.includes(search) ||
            e.flag_key.includes(search)),
      ),
    [experiments, statusFilter, flagFilter, search],
  )

  useEffect(() => {
    if (!envId) return
    const ctrl = new AbortController()
    Promise.all(
      filtered.map(async (exp) => {
        const contextType = pickExposureContextType(exp)
        try {
          const page = await listExperimentExposures(
            envId,
            exp.key,
            contextType,
            { page: 1, per_page: 1 },
            ctrl.signal,
          )
          return [exp.key, page.total] as const
        } catch {
          return [exp.key, null] as const
        }
      }),
    )
      .then((entries) => {
        if (ctrl.signal.aborted) return
        setExposureCount(new Map(entries))
      })
      .catch(() => undefined)
    return () => ctrl.abort()
  }, [filtered, envId])

  // URL-driven filter setters.
  function setStatusFilter(next: StatusFilter) {
    setSearchParams((prev) => {
      if (next === 'all') prev.delete('status')
      else prev.set('status', next)
      return prev
    })
  }

  function setFlagFilter(next: string | null) {
    setSearchParams((prev) => {
      if (!next) prev.delete('flag')
      else prev.set('flag', next)
      return prev
    })
  }

  if (!envId) {
    return (
      <>
        <PageHeader
          crumbs={['Experiments']}
          title="Experiments"
          subtitle="A/B and multivariate tests."
        />
        <div className="page-body">
          <div className="card">
            <EmptyState
              icon={<I.beaker size={20} />}
              title="No environment selected"
              desc="No environment selected — set an environment ID in environments settings"
              action={
                <button
                  className="btn primary"
                  onClick={() => navigate(`/org/${orgId}/environments`)}
                >
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
        crumbs={['Experiments']}
        title="Experiments"
        subtitle="Stats compute every 60 minutes. Active experiments lock their bound flag for the duration."
        actions={
          <button className="btn primary" onClick={() => setShowCreate(true)}>
            <I.plus size={14} /> New experiment
          </button>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <LoadingSpinner label="Loading experiments…" />
          </div>
        )}

        {error && !loading && <ErrorBanner message={error} icon={<I.alert size={14} />} />}

        {!loading && !error && (
          <>
            <div
              style={{
                display: 'flex',
                gap: 12,
                marginBottom: 16,
                alignItems: 'center',
                flexWrap: 'wrap',
              }}
            >
              <div className="search-input">
                <I.search size={14} />
                <input
                  className="input"
                  placeholder="Filter experiments…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>

              <div
                style={{
                  display: 'flex',
                  gap: 4,
                  padding: 3,
                  background: 'var(--bg-sunken)',
                  borderRadius: 8,
                  border: '1px solid var(--border)',
                }}
              >
                {STATUS_FILTERS.map((s) => (
                  <button
                    key={s}
                    className="btn sm"
                    style={{
                      border: 'none',
                      background: statusFilter === s ? 'var(--surface)' : 'transparent',
                      color: statusFilter === s ? 'var(--fg)' : 'var(--fg-muted)',
                      boxShadow: statusFilter === s ? 'var(--shadow-xs)' : 'none',
                    }}
                    onClick={() => setStatusFilter(s)}
                  >
                    {STATUS_FILTER_LABELS[s]}
                  </button>
                ))}
              </div>

              <select
                className="input"
                aria-label="Flag filter"
                style={{ minWidth: 200 }}
                value={flagFilter ?? ''}
                onChange={(e) => setFlagFilter(e.target.value || null)}
              >
                <option value="">All flags</option>
                {flagOptions.map((f) => (
                  <option key={f.flag_id} value={f.key}>
                    {f.name} ({f.key})
                  </option>
                ))}
              </select>
            </div>

            {experiments.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.beaker size={20} />}
                  title="No experiments yet"
                  desc="Create your first experiment to start running A/B tests."
                  action={
                    <button className="btn primary" onClick={() => setShowCreate(true)}>
                      <I.plus size={13} /> New experiment
                    </button>
                  }
                />
              </div>
            )}

            {experiments.length > 0 && filtered.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.search size={20} />}
                  title="No results"
                  desc="No experiments match the current filter."
                  action={<button className="btn" onClick={() => { setSearch(''); setStatusFilter('all'); setFlagFilter(null) }}>Clear filter</button>}
                />
              </div>
            )}

            {experiments.length > 0 && (
              <div className="card">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Experiment</th>
                      <th>Status</th>
                      <th>Flag</th>
                      <th>Primary metric</th>
                      <th>Exposed contexts</th>
                      <th>Days remaining</th>
                      <th>Variants</th>
                      <th>Updated</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {filtered.map((e) => {
                      const daysRemaining = computeDaysRemaining(e.scheduled_end_at ?? null)
                      const exposures = exposureCount.get(e.key)
                      const primaryLabel = resolvePrimaryMetricLabel(e, metricLookup)
                      return (
                        <tr
                          key={e.id ?? e.key}
                          className="row-clickable"
                          onClick={() => navigate(`/org/${orgId}/experiments/${e.id ?? e.key}`)}
                        >
                          <td>
                            <div style={{ fontWeight: 600 }}>{e.name}</div>
                            <div
                              style={{
                                fontSize: 11,
                                color: 'var(--fg-muted)',
                                fontFamily: 'var(--font-mono)',
                              }}
                            >
                              {e.key}
                            </div>
                          </td>
                          <td>
                            <span className={`badge ${statusBadgeClass(e.status)}`}>
                              {e.status === 'running' && (
                                <span className="dot" style={{ background: 'currentColor' }} />
                              )}
                              {e.status}
                            </span>
                          </td>
                          <td>
                            <span className="mono-key" style={{ fontSize: 11 }}>
                              {e.flag_key}
                            </span>
                          </td>
                          <td
                            style={{
                              fontSize: 12,
                              color: 'var(--fg)',
                            }}
                          >
                            {primaryLabel}
                          </td>
                          <td
                            style={{
                              fontFamily: 'var(--font-mono)',
                              fontSize: 11,
                              color:
                                exposures == null ? 'var(--fg-muted)' : 'var(--fg)',
                            }}
                          >
                            {exposures == null
                              ? '…'
                              : Number(exposures).toLocaleString('en-US')}
                          </td>
                          <td
                            style={{
                              fontFamily: 'var(--font-mono)',
                              fontSize: 11,
                              color: 'var(--fg-muted)',
                            }}
                          >
                            {formatDaysRemaining(daysRemaining)}
                          </td>
                          <td style={{ fontFamily: 'var(--font-mono)' }}>{e.variants}</td>
                          <td
                            style={{
                              color: 'var(--fg-muted)',
                              fontFamily: 'var(--font-mono)',
                              fontSize: 11,
                            }}
                          >
                            {new Date(e.updated_at).toLocaleDateString()}
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
            )}
          </>
        )}
      </div>

      {showCreate && <CreateExperimentModal onClose={() => setShowCreate(false)} />}
    </>
  )
}
