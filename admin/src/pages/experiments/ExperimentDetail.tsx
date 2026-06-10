import { useState, useEffect, useCallback, useRef } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { useTweaks } from '../../hooks/useTweaks'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { usePermissions } from '../../hooks/usePermissions'
import { LifecycleActions } from './LifecycleActions'
import { lifecycleTimeline } from './lifecycleHelpers'
import {
  ContextTypeProvider,
  useActiveContextType,
} from '../../context/ContextTypeContext'
import { ContextTypeTabs } from '../../components/ContextTypeTabs'
import { ScheduleBuilder } from '../../components/schedule/ScheduleBuilder'
import { DependencyGraph } from '../../components/dependency/DependencyGraph'
import type { MutationPreset } from '../../components/schedule/ScheduleBuilder'
import {
  getExperiment,
  getExperimentResults,
  listExperimentExposures,
  getExperimentTimeseries,
  listExperimentIterations,
  triggerExperimentRecompute,
  getRecomputeStatus,
  api,
} from '../../lib/api'
import type { ExperimentSummary } from '../../lib/api'
import type {
  ExperimentResults,
  PaginatedExposures,
  Timeseries as TimeseriesData,
} from '../../lib/types'
import { extractErrorMessage } from '../../lib/errors'
import {
  buildExperimentDisplay,
  listContextTypes,
  pickContextTypeResult,
} from './ExperimentDetail.helpers'
import { Results } from './tabs/Results'
import type { ResultsView, GoalDirection } from './tabs/Results'
import { ExposuresPanel } from './tabs/Exposures'
import { Timeseries } from './tabs/Timeseries'
import type { MetricRowEntry } from './tabs/Timeseries'
import { IterationsTab, shouldKeepPolling } from './tabs/Iterations'
import type { IterationSummary } from './tabs/Iterations'
import { InteractionsTab, hasSignificantInteraction } from './tabs/Interactions'
import { listExperimentInteractions } from '../../lib/api/exclusionGroups'
import type { ExperimentInteraction } from '../../lib/api/exclusionGroups'
import { BanditTab } from './tabs/Bandit'
import {
  getBanditState,
  getBanditAllocationHistory,
} from '../../lib/api/bandit'
import type { BanditState, BanditAllocationHistory } from '../../lib/api/bandit'

/**
 * Experiment lifecycle-transition presets for the schedule builder
 * (flag_lifecycle_20260604 A3: start, pause, resume, stop, archive). The
 * schedule-service dispatches each `transition` to the experimentation-service's
 * TransitionExperiment RPC at fire time.
 */
const EXPERIMENT_TRANSITION_PRESETS: MutationPreset[] = [
  { id: 'start', label: 'Start experiment', payload: { transition: 'start' } },
  { id: 'pause', label: 'Pause experiment', payload: { transition: 'pause' } },
  { id: 'resume', label: 'Resume experiment', payload: { transition: 'resume' } },
  { id: 'stop', label: 'Stop experiment', payload: { transition: 'stop' } },
  { id: 'archive', label: 'Archive experiment', payload: { transition: 'archive' } },
]

/**
 * Minimal projection of a `metric_definitions` row used by ExperimentDetail
 * to resolve display names + goal direction from the IDs referenced by an
 * experiment.
 */
interface MetricNameRow {
  id: string
  name: string
  kind: 'aggregation' | 'ratio' | 'funnel'
  goal_direction?: GoalDirection
}

type Tab =
  | 'results'
  | 'bandit'
  | 'exposures'
  | 'timeseries'
  | 'iterations'
  | 'interactions'
  | 'config'
  | 'metrics'
  | 'events'
  | 'schedule'

// ─── Config tab ──────────────────────────────────────────────────────────────

function fmtDate(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}

function ExpConfig({ exp, metricNames }: { exp: ExperimentSummary; metricNames: string[] }) {
  // All rows are sourced from the real ExperimentSummary — no fabricated
  // allocation / targeting / MDE / CUPED values (those have no backend source).
  const primaryLabel = metricNames[0] ?? exp.metric_ids[0] ?? '—'
  const secondary = metricNames.slice(1)
  const dash = (v: React.ReactNode) => <span style={{ fontSize: 12, color: 'var(--fg-muted)' }}>{v}</span>
  const rows: [string, React.ReactNode][] = [
    ['Bound flag', <span className="mono-key">{exp.flag_key}</span>],
    ['Status', <span className={`badge ${exp.status === 'running' ? 'info' : 'success'}`}>{exp.status}</span>],
    ['Statistical model', <span className="badge">{exp.model}</span>],
    ['Primary metric', exp.metric_ids.length > 0 ? <span style={{ fontWeight: 600 }}>{primaryLabel}</span> : dash('none attached')],
    ['Secondary metrics', secondary.length > 0 ? <span style={{ fontSize: 12 }}>{secondary.join(' · ')}</span> : dash('—')],
    ['Variants', <span style={{ fontFamily: 'var(--font-mono)' }}>{exp.variant_keys.length > 0 ? exp.variant_keys.join(', ') : exp.variants}</span>],
    ['Randomisation unit', exp.unit_context_types.length > 0 ? <span style={{ fontSize: 12 }}>{exp.unit_context_types.join(' · ')}</span> : dash('—')],
    ['Exclusion group', exp.exclusion_group_id ? <span className="mono-key">{exp.exclusion_group_id}</span> : dash('none')],
    ['Created', <span style={{ fontSize: 12 }}>{fmtDate(exp.created_at)}</span>],
    ['Started', exp.started_at ? <span style={{ fontSize: 12 }}>{fmtDate(exp.started_at)}</span> : dash('not started')],
    ['Ended', exp.ended_at ? <span style={{ fontSize: 12 }}>{fmtDate(exp.ended_at)}</span> : dash('—')],
  ]
  const timeline = lifecycleTimeline(exp)
  return (
    <div className="split-2-third">
      <div className="card">
        <div className="card-header"><div className="card-title">Configuration</div></div>
        <div className="card-body" style={{ padding: 0 }}>
          {rows.map(([k, v], i) => (
            <div key={i} style={{ display: 'flex', padding: '10px 18px', borderBottom: i < rows.length - 1 ? '1px solid var(--border-faint)' : 'none', alignItems: 'center' }}>
              <div style={{ width: 200, color: 'var(--fg-muted)', fontSize: 12 }}>{k}</div>
              <div style={{ flex: 1 }}>{v}</div>
            </div>
          ))}
        </div>
      </div>
      <div className="card">
        <div className="card-header"><div className="card-title">Lifecycle</div></div>
        <div className="card-body">
          {timeline.length === 0 ? (
            <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>No lifecycle events yet.</div>
          ) : timeline.map((stage, i) => (
            <div key={stage.label} style={{ display: 'flex', gap: 10, padding: '8px 0', borderBottom: i < timeline.length - 1 ? '1px solid var(--border-faint)' : 'none' }}>
              <div style={{ width: 8, height: 8, borderRadius: '50%', background: i === timeline.length - 1 ? 'var(--accent)' : 'var(--success)', marginTop: 4, flexShrink: 0 }} />
              <div style={{ flex: 1, fontSize: 12 }}>
                <div style={{ fontWeight: 600 }}>{stage.label}</div>
                <div style={{ color: 'var(--fg-muted)' }}>{fmtDate(stage.at)}</div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

// ─── ExperimentDetail ────────────────────────────────────────────────────────

export function ExperimentDetail() {
  const { key } = useParams<{ key: string }>()
  const { envId } = useOrgContext()
  const [apiExp, setApiExp] = useState<ExperimentSummary | null>(null)
  const [apiResults, setApiResults] = useState<ExperimentResults | null>(null)
  const [apiLoading, setApiLoading] = useState(false)
  const [apiError, setApiError] = useState<string | null>(null)
  const [metricNames, setMetricNames] = useState<string[]>([])
  // Primary metric's goal_direction — drives winner highlighting on the
  // Results tab. Defaults to "increase" when the metric lookup hasn't
  // resolved yet (matches the metric-form default).
  const [primaryGoalDirection, setPrimaryGoalDirection] =
    useState<GoalDirection>('increase')

  useEffect(() => {
    if (!envId || !key) return
    setApiLoading(true)
    setApiError(null)
    const ctrl = new AbortController()
    Promise.all([
      getExperiment(envId, key, ctrl.signal),
      getExperimentResults(envId, key, ctrl.signal).catch((err) => {
        // 409 = no completed iteration yet, 404 = no results endpoint result.
        // Both are non-fatal — render the page with summary-only data.
        if (ctrl.signal.aborted) throw err
        return null
      }),
    ])
      .then(([exp, results]) => {
        setApiExp(exp)
        setApiResults(results)
      })
      .catch((err) => {
        if (ctrl.signal.aborted) return
        setApiError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ctrl.signal.aborted) setApiLoading(false)
      })
    return () => ctrl.abort()
  }, [envId, key])

  // Phase 7: resolve metric IDs → names by fetching the metric list once.
  useEffect(() => {
    const ids = apiExp?.metric_ids
    if (!envId || !ids || ids.length === 0) {
      setMetricNames([])
      return
    }
    const ctrl = new AbortController()
    api
      .get<{ items: MetricNameRow[] }>(
        `/v1/metrics?env_id=${encodeURIComponent(envId)}&per_page=200`,
        { signal: ctrl.signal },
      )
      .then(({ data }) => {
        const byId = new Map((data.items ?? []).map((m) => [m.id, m]))
        setMetricNames(ids.map((id) => byId.get(id)?.name ?? id))
        const primary = byId.get(ids[0])
        if (primary?.goal_direction) {
          setPrimaryGoalDirection(primary.goal_direction)
        }
      })
      .catch(() => {
        // Best-effort — config tab falls back to the raw metric_id.
      })
    return () => ctrl.abort()
  }, [envId, apiExp])

  if (apiLoading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading experiment…</span>
      </div>
    )
  }

  if (apiError || !apiExp) {
    return (
      <div className="page-body">
        <div style={{ padding: '12px 16px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 8, color: 'var(--danger)', fontSize: 13 }}>
          <I.alert size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
          {apiError ?? 'Experiment not found'}
        </div>
      </div>
    )
  }

  // Context types come from the results envelope when present, otherwise fall
  // back to ['user'] so the provider has at least one valid value.
  const contextTypes = listContextTypes(apiResults)
  const resolvedContextTypes = contextTypes.length > 0 ? contextTypes : ['user']

  return (
    <ContextTypeProvider
      contextTypes={resolvedContextTypes}
      experimentId={apiExp.key}
    >
      <ExperimentDetailBody
        apiExp={apiExp}
        apiResults={apiResults}
        metricNames={metricNames}
        primaryGoalDirection={primaryGoalDirection}
        onExperimentUpdated={setApiExp}
      />
    </ContextTypeProvider>
  )
}

interface BodyProps {
  apiExp: ExperimentSummary
  apiResults: ExperimentResults | null
  metricNames: string[]
  primaryGoalDirection: GoalDirection
  onExperimentUpdated: (exp: ExperimentSummary) => void
}

function ExperimentDetailBody({
  apiExp,
  apiResults,
  metricNames,
  primaryGoalDirection,
  onExperimentUpdated,
}: BodyProps) {
  const navigate = useNavigate()
  const { tweaks } = useTweaks()
  const { orgId, envId, projectId } = useOrgContext()
  const { roles } = usePermissions()
  const canManage = roles.includes('org_admin')
  const [transitionError, setTransitionError] = useState<string | null>(null)
  const { contextTypes, activeContextType, setActiveContextType } =
    useActiveContextType()

  const [tab, setTab] = useState<Tab>('results')

  // Default Frequentist/Bayesian view picker passed to the per-tab <Results>
  // component (which manages its own localStorage-persisted toggle on top of
  // this default). Reads the user's global tweak + the experiment model so
  // the initial render matches the user's preference.
  const resultsDefaultModel: ResultsView =
    apiExp.model === 'bayesian' || tweaks.expViz === 'bayesian'
      ? 'bayesian'
      : 'frequentist'

  // ── Exposures pagination + fetch ──────────────────────────────────────────
  const [exposuresPage, setExposuresPage] = useState(1)
  const [exposures, setExposures] = useState<PaginatedExposures | null>(null)
  const [exposuresLoading, setExposuresLoading] = useState(false)
  const [exposuresError, setExposuresError] = useState<string | null>(null)

  useEffect(() => {
    if (tab !== 'exposures' || !envId || !apiExp.key) return
    const ctrl = new AbortController()
    setExposuresLoading(true)
    setExposuresError(null)
    listExperimentExposures(
      envId,
      apiExp.key,
      activeContextType,
      { page: exposuresPage, per_page: 50 },
      ctrl.signal,
    )
      .then((data) => {
        if (ctrl.signal.aborted) return
        setExposures(data)
      })
      .catch((err) => {
        if (ctrl.signal.aborted) return
        setExposuresError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ctrl.signal.aborted) setExposuresLoading(false)
      })
    return () => ctrl.abort()
  }, [tab, envId, apiExp.key, activeContextType, exposuresPage])

  // Reset to page 1 when active context type changes (different cohort).
  useEffect(() => {
    setExposuresPage(1)
  }, [activeContextType])

  // ── Iterations fetch ──────────────────────────────────────────────────────
  // The Iterations tab consumes a list of past + active iterations. We fetch
  // lazily when the tab is selected (cheap query, but no reason to preload).
  const [iterations, setIterations] = useState<IterationSummary[]>([])
  const [iterationsLoading, setIterationsLoading] = useState(false)
  const [iterationsError, setIterationsError] = useState<string | null>(null)
  const [selectedIteration, setSelectedIteration] =
    useState<IterationSummary | null>(null)

  useEffect(() => {
    if (tab !== 'iterations' || !envId || !apiExp.key) return
    const ctrl = new AbortController()
    setIterationsLoading(true)
    setIterationsError(null)
    listExperimentIterations(envId, apiExp.key, undefined, ctrl.signal)
      .then((data) => {
        if (ctrl.signal.aborted) return
        // The DTO shape matches `IterationSummary` 1:1; the cast is just to
        // bridge the duplicate type declarations (admin/lib vs. tab module).
        setIterations(data.items as IterationSummary[])
      })
      .catch((err) => {
        if (ctrl.signal.aborted) return
        setIterationsError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ctrl.signal.aborted) setIterationsLoading(false)
      })
    return () => ctrl.abort()
  }, [tab, envId, apiExp.key])

  // Clear the past-iteration drill-down when the user switches context types
  // (the snapshot is keyed by iteration_id, but the active context type drives
  // which per-context-type results bucket renders inside <Results>).
  useEffect(() => {
    setSelectedIteration(null)
  }, [activeContextType])

  // ── Interactions fetch ────────────────────────────────────────────────────
  // Fetched on mount (not lazily) so the Results tab can surface a "significant
  // interaction" warning banner without the user opening the Interactions tab.
  const [interactions, setInteractions] = useState<ExperimentInteraction[]>([])
  const [interactionsLoading, setInteractionsLoading] = useState(false)
  const [interactionsError, setInteractionsError] = useState<string | null>(null)

  useEffect(() => {
    if (!envId || !apiExp.key) return
    const ctrl = new AbortController()
    setInteractionsLoading(true)
    setInteractionsError(null)
    listExperimentInteractions(envId, apiExp.key, ctrl.signal)
      .then((data) => {
        if (ctrl.signal.aborted) return
        setInteractions(data.interactions ?? [])
      })
      .catch((err) => {
        if (ctrl.signal.aborted) return
        setInteractionsError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ctrl.signal.aborted) setInteractionsLoading(false)
      })
    return () => ctrl.abort()
  }, [envId, apiExp.key])

  const showInteractionWarning = hasSignificantInteraction(interactions)

  // ── Bandit state + history fetch ──────────────────────────────────────────
  // Fetched on mount so the "Bandit" tab can be shown/hidden based on
  // `is_bandit`. Allocation history rides along (it backs the chart + timeline).
  const [banditState, setBanditState] = useState<BanditState | null>(null)
  const [banditHistory, setBanditHistory] =
    useState<BanditAllocationHistory | null>(null)
  const [banditLoading, setBanditLoading] = useState(false)
  const [banditError, setBanditError] = useState<string | null>(null)

  useEffect(() => {
    if (!envId || !apiExp.key) return
    const ctrl = new AbortController()
    setBanditLoading(true)
    setBanditError(null)
    getBanditState(envId, apiExp.key, ctrl.signal)
      .then((state) => {
        if (ctrl.signal.aborted) return
        setBanditState(state)
        // Only chase the history when the experiment is actually a bandit.
        if (!state.is_bandit) return
        return getBanditAllocationHistory(
          envId,
          apiExp.key,
          { limit: 200 },
          ctrl.signal,
        ).then((history) => {
          if (ctrl.signal.aborted) return
          setBanditHistory(history)
        })
      })
      .catch((err) => {
        if (ctrl.signal.aborted) return
        // A non-bandit experiment may 404/422 here — treat as "not a bandit"
        // rather than a hard error so the rest of the page renders.
        setBanditState(null)
        setBanditError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ctrl.signal.aborted) setBanditLoading(false)
      })
    return () => ctrl.abort()
  }, [envId, apiExp.key])

  const isBandit = banditState?.is_bandit === true

  // ── Timeseries fetch ──────────────────────────────────────────────────────
  const [tsDays, setTsDays] = useState(7)
  const [tsSeries, setTsSeries] = useState<Record<string, TimeseriesData>>({})
  const [tsSelectedMetric, setTsSelectedMetric] = useState<string | null>(null)
  const [tsLoadingMetric, setTsLoadingMetric] = useState<string | null>(null)

  // Reset cached series whenever the active context type or window changes —
  // the data is keyed differently and stale values should not bleed across.
  useEffect(() => {
    setTsSeries({})
  }, [activeContextType, tsDays])

  // Fetch series for every metric when the timeseries tab is active.
  useEffect(() => {
    if (tab !== 'timeseries' || !envId || !apiExp.key) return
    const ids = apiExp.metric_ids
    if (ids.length === 0) return
    const ctrl = new AbortController()
    // Sequentially fetch — keeps the loading marker meaningful and avoids
    // hammering the gateway with N parallel requests.
    let cancelled = false
    ;(async () => {
      for (const metricId of ids) {
        if (cancelled || ctrl.signal.aborted) return
        setTsLoadingMetric(metricId)
        try {
          const data = await getExperimentTimeseries(
            envId,
            apiExp.key,
            { metric_id: metricId, context_type: activeContextType, days: tsDays },
            ctrl.signal,
          )
          if (cancelled || ctrl.signal.aborted) return
          setTsSeries((prev) => ({ ...prev, [metricId]: data }))
        } catch {
          // Best-effort per metric — surface errors via empty series.
        } finally {
          if (!cancelled && !ctrl.signal.aborted) setTsLoadingMetric(null)
        }
      }
    })()
    return () => {
      cancelled = true
      ctrl.abort()
    }
  }, [tab, envId, apiExp.key, apiExp.metric_ids, activeContextType, tsDays])

  // ── Recompute lifecycle ───────────────────────────────────────────────────
  const [recomputeStatus, setRecomputeStatus] = useState<string | null>(null)
  const [recomputeError, setRecomputeError] = useState<string | null>(null)
  const [recomputeJobId, setRecomputeJobId] = useState<string | null>(null)
  const pollAbortRef = useRef<AbortController | null>(null)

  // Cleanup polling on unmount.
  useEffect(() => {
    return () => {
      pollAbortRef.current?.abort()
    }
  }, [])

  const startRecompute = useCallback(async () => {
    if (!envId || !apiExp.key) return
    setRecomputeError(null)
    setRecomputeStatus('queued')
    try {
      const job = await triggerExperimentRecompute(envId, apiExp.key)
      setRecomputeJobId(job.job_id)
      setRecomputeStatus(job.status)
    } catch (err) {
      setRecomputeStatus('failed')
      setRecomputeError(extractErrorMessage(err))
    }
  }, [envId, apiExp.key])

  // Polling effect — runs whenever a job_id appears + status is non-terminal.
  useEffect(() => {
    if (!envId || !apiExp.key || !recomputeJobId) return
    if (recomputeStatus != null && !shouldKeepPolling(recomputeStatus)) return
    pollAbortRef.current?.abort()
    const ctrl = new AbortController()
    pollAbortRef.current = ctrl
    let cancelled = false
    const poll = async () => {
      while (!cancelled && !ctrl.signal.aborted) {
        try {
          const r = await getRecomputeStatus(
            envId,
            apiExp.key,
            recomputeJobId,
            ctrl.signal,
          )
          if (cancelled || ctrl.signal.aborted) return
          setRecomputeStatus(r.status)
          if (r.error) setRecomputeError(r.error)
          if (!shouldKeepPolling(r.status)) return
        } catch (err) {
          if (cancelled || ctrl.signal.aborted) return
          setRecomputeStatus('failed')
          setRecomputeError(extractErrorMessage(err))
          return
        }
        // 2s interval between polls.
        await new Promise((resolve) => setTimeout(resolve, 2000))
      }
    }
    void poll()
    return () => {
      cancelled = true
      ctrl.abort()
    }
  }, [envId, apiExp.key, recomputeJobId, recomputeStatus])

  // Page-level Bayesian heuristic — used only by the stat-grid summary
  // (the per-tab <Results> component owns its own toggle).
  const useBayesian = resultsDefaultModel === 'bayesian'

  // Build the display model from real API responses. Phase 9 routes the
  // per-tab rendering through the new <Results>/<ExposuresPanel>/<Timeseries>/
  // <IterationsTab> components; the legacy display model still feeds the
  // header subtitle + stat-grid summary on the Results tab.
  const display = buildExperimentDisplay(apiExp, apiResults, activeContextType, useBayesian)

  // Picked block — used to drive samples accurately when results are present.
  const picked = pickContextTypeResult(apiResults, activeContextType)
  const sampleCount = picked?.block.variants.reduce((a, v) => a + (v.participant_count ?? 0), 0) ?? 0

  const displayName = apiExp.name
  const displayKey = apiExp.key
  const displayStatus = apiExp.status

  // Show the context-type switcher only when results actually carry more than
  // one context type — otherwise the picker would have a single static option.
  const showCtxSwitcher = contextTypes.length > 1

  return (
    <>
      <PageHeader
        crumbs={[<a key="1" onClick={() => navigate(`/org/${orgId}/experiments`)} style={{ cursor: 'pointer' }}>Experiments</a>, displayKey]}
        title={displayName}
        subtitle={`Bound to flag ${display.flag} · ${display.model} model · started ${display.started}`}
        badge={<span className={`badge ${displayStatus === 'running' ? 'info' : 'success'}`}>{displayStatus}</span>}
        actions={
          <>
            {displayStatus === 'running' && (
              <button
                className="btn"
                onClick={startRecompute}
                disabled={
                  recomputeStatus != null && shouldKeepPolling(recomputeStatus)
                }
              >
                <I.refresh size={13} /> Recompute
              </button>
            )}
            <LifecycleActions
              envId={envId ?? ''}
              experiment={apiExp}
              canManage={canManage && !!envId}
              onUpdated={(exp) => { setTransitionError(null); onExperimentUpdated(exp) }}
              onError={setTransitionError}
            />
          </>
        }
      />
      <div className="page-body">
        {transitionError && (
          <div style={{ fontSize: 12, color: 'var(--danger)', marginBottom: 12, padding: '8px 12px', background: 'var(--danger-bg)', borderRadius: 6 }}>
            {transitionError}
          </div>
        )}
        {showCtxSwitcher && (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              marginBottom: 14,
              fontSize: 12,
              color: 'var(--fg-muted)',
            }}
          >
            <span>Context type</span>
            <ContextTypeTabs
              contextTypes={contextTypes}
              activeContextType={activeContextType}
              onChange={setActiveContextType}
            />
          </div>
        )}

        <div className="tabs">
          <button className={`tab ${tab === 'results' ? 'active' : ''}`} onClick={() => setTab('results')}>Results</button>
          {isBandit && (
            <button className={`tab ${tab === 'bandit' ? 'active' : ''}`} onClick={() => setTab('bandit')}>Bandit</button>
          )}
          <button className={`tab ${tab === 'exposures' ? 'active' : ''}`} onClick={() => setTab('exposures')}>Exposures · SRM</button>
          <button className={`tab ${tab === 'timeseries' ? 'active' : ''}`} onClick={() => setTab('timeseries')}>Time-series</button>
          <button className={`tab ${tab === 'iterations' ? 'active' : ''}`} onClick={() => setTab('iterations')}>Iterations</button>
          <button className={`tab ${tab === 'interactions' ? 'active' : ''}`} onClick={() => setTab('interactions')}>Interactions{showInteractionWarning && <span className="count" style={{ background: 'var(--warning-bg, rgba(217,119,6,0.12))', color: 'var(--warning, #b45309)' }}>!</span>}</button>
          <button className={`tab ${tab === 'config' ? 'active' : ''}`} onClick={() => setTab('config')}>Configuration</button>
          <button className={`tab ${tab === 'metrics' ? 'active' : ''}`} onClick={() => setTab('metrics')}>Metrics <span className="count">{metricNames.length || apiExp.metric_ids.length || 0}</span></button>
          <button className={`tab ${tab === 'events' ? 'active' : ''}`} onClick={() => setTab('events')}>Events</button>
          <button className={`tab ${tab === 'schedule' ? 'active' : ''}`} onClick={() => setTab('schedule')}>Schedule</button>
        </div>

        {tab === 'results' && (
          <>
            {showInteractionWarning && (
              <div
                role="alert"
                data-testid="interaction-warning"
                style={{
                  display: 'flex',
                  alignItems: 'flex-start',
                  gap: 8,
                  padding: '10px 14px',
                  marginBottom: 14,
                  borderRadius: 8,
                  background: 'var(--warning-bg, rgba(217,119,6,0.1))',
                  border: '1px solid rgba(217,119,6,0.3)',
                  color: 'var(--warning, #b45309)',
                  fontSize: 13,
                }}
              >
                <I.alert size={15} style={{ flexShrink: 0, marginTop: 1 }} />
                <span>
                  A significant interaction with one or more concurrent
                  experiments was detected (frequentist or Bayesian). Interpret
                  these results with caution — the estimated effect may be
                  confounded. See the Interactions tab for details.
                </span>
              </div>
            )}
            <div className="stat-grid" style={{ marginBottom: 14 }}>
              <div className="stat"><div className="stat-label">Lift (relative)</div><div className="stat-value" style={{ color: 'var(--success)' }}>{display.lift}</div><div className="stat-delta">vs control</div></div>
              <div className="stat"><div className="stat-label">{useBayesian ? 'P(variant > control)' : 'P-value'}</div><div className="stat-value">{useBayesian ? `${display.confidence}%` : (display.confidence > 0 ? (1 - display.confidence / 100).toFixed(4) : '—')}</div><div className="stat-delta">{useBayesian ? 'Bayesian posterior' : 'two-sided'}</div></div>
              <div className="stat"><div className="stat-label">Samples</div><div className="stat-value">{sampleCount > 0 ? sampleCount.toLocaleString('en-US') : display.samples}</div><div className="stat-delta">control + {display.variants - 1} variant{display.variants > 2 ? 's' : ''}</div></div>
              <div className="stat"><div className="stat-label">Time remaining</div><div className="stat-value" style={{ fontSize: 22 }}>{display.remaining}</div><div className="stat-delta">{display.remaining === 'ready' ? 'min sample size reached' : 'of 14d'}</div></div>
            </div>

            <Results
              experimentId={apiExp.key}
              activeContextType={activeContextType}
              results={apiResults}
              defaultModel={resultsDefaultModel}
              goalDirection={primaryGoalDirection}
              metricNames={metricNames.length > 0 ? metricNames : apiExp.metric_ids}
            />
          </>
        )}

        {tab === 'bandit' && (
          <BanditTab
            state={banditState}
            history={banditHistory}
            loading={banditLoading}
            error={banditError}
          />
        )}

        {tab === 'exposures' && (
          <ExposuresPanel
            results={apiResults}
            activeContextType={activeContextType}
            exposures={exposures}
            page={exposuresPage}
            onPageChange={setExposuresPage}
            loading={exposuresLoading}
            error={exposuresError}
          />
        )}

        {tab === 'timeseries' && (
          <Timeseries
            activeContextType={activeContextType}
            metrics={apiExp.metric_ids.map<MetricRowEntry>((id, i) => ({
              id,
              name: metricNames[i] ?? id,
            }))}
            seriesByMetric={tsSeries}
            days={tsDays}
            onDaysChange={setTsDays}
            selectedMetricId={tsSelectedMetric}
            onMetricSelect={setTsSelectedMetric}
            loadingMetricId={tsLoadingMetric}
          />
        )}

        {tab === 'iterations' && (
          <IterationsTab
            iterations={iterations}
            loading={iterationsLoading}
            error={iterationsError}
            selectedIteration={selectedIteration}
            // Per-iteration drill-down results are reserved for a future
            // `getIterationResults(iter_id)` wrapper. The tab gracefully
            // renders a loading placeholder until then; selecting a row only
            // toggles the highlight + header banner.
            selectedResults={null}
            selectedLoading={false}
            onIterationSelect={setSelectedIteration}
            onRecompute={startRecompute}
            recomputeStatus={recomputeStatus}
            recomputeError={recomputeError}
            activeContextType={activeContextType}
            goalDirection={primaryGoalDirection}
            experimentId={apiExp.key}
            defaultModel={resultsDefaultModel}
            metricNames={metricNames.length > 0 ? metricNames : apiExp.metric_ids}
          />
        )}

        {tab === 'interactions' && (
          <InteractionsTab
            interactions={interactions}
            loading={interactionsLoading}
            error={interactionsError}
            isBandit={isBandit}
          />
        )}

        {tab === 'config' && (
          <>
            <ExpConfig exp={apiExp} metricNames={metricNames} />
            {projectId && (
              <div className="card" style={{ marginTop: 16 }}>
                <div className="card-header">
                  <div className="card-title">Start prerequisites &amp; dependencies</div>
                </div>
                <div className="card-body">
                  <p style={{ fontSize: 12, color: 'var(--fg-muted)', marginTop: 0 }}>
                    Conditions that must hold for this experiment to start (manual or
                    scheduled). A start is refused while any prerequisite is unmet.
                  </p>
                  <DependencyGraph
                    projectId={projectId}
                    entityKind="experiments"
                    entityId={apiExp.id}
                  />
                </div>
              </div>
            )}
          </>
        )}

        {tab === 'metrics' && (
          <div className="card">
            {/* Only columns with a real backing source are shown — primary vs
                secondary is derived from metric order. Type/aggregation/threshold
                live on the Metrics page; we don't render empty columns here. */}
            <table className="table">
              <thead><tr><th>Metric</th><th>Role</th></tr></thead>
              <tbody>
                {(metricNames.length > 0 ? metricNames : apiExp.metric_ids).map((m, i) => (
                  <tr key={m}>
                    <td><span className="mono-key">{m}</span></td>
                    <td><span className={`badge ${i === 0 ? 'accent' : ''}`}>{i === 0 ? 'primary' : 'secondary'}</span></td>
                  </tr>
                ))}
                {apiExp.metric_ids.length === 0 && (
                  <tr><td colSpan={2} style={{ textAlign: 'center', color: 'var(--fg-muted)', padding: 24 }}>No metrics attached</td></tr>
                )}
              </tbody>
            </table>
          </div>
        )}

        {tab === 'events' && (
          <div className="card">
            <div className="empty">
              <div className="empty-icon"><I.event size={20} /></div>
              <div className="empty-title">No per-experiment event stream</div>
              <div className="empty-desc">
                Event firings and definitions are managed on the <strong>Events</strong> page,
                and metric configuration on the <strong>Metrics</strong> page. This experiment's
                metrics are listed under the Metrics tab.
              </div>
            </div>
          </div>
        )}
        {tab === 'schedule' && envId && (
          <ScheduleBuilder
            envId={envId}
            entityKind="experiments"
            entityId={apiExp.id}
            presets={EXPERIMENT_TRANSITION_PRESETS}
          />
        )}
      </div>
    </>
  )
}
