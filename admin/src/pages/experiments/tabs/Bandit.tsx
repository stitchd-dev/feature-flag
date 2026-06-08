/**
 * Bandit Results view — FR7, Phase 12 Task 12.2.
 *
 * Rendered under the experiment-detail "Bandit" tab, only when the experiment
 * is in bandit mode (`is_bandit`). Surfaces:
 *
 *   • Convergence / commit badge — Exploring / Converged / Committed / Rolled out.
 *   • Allocation-over-time chart — per-arm weight_bp from `history.runs`
 *     (reuses the Timeseries SVG-polyline pattern, no chart dependency).
 *   • Current weights table — `current_allocation`.
 *   • Reward posteriors per objective — per-variant mean ± CI + guardrail badge.
 *   • Lifecycle + campaign timeline — the history runs as a timeline + campaign
 *     status (iterations_spawned, status) when a campaign is attached.
 *
 * Data is fetched by the parent (ExperimentDetail) and passed in, mirroring the
 * Exposures / Iterations / Interactions tab convention so this component stays
 * presentational + `renderToString`-testable. Pure helpers are exported for
 * direct unit testing.
 */
import type {
  BanditState,
  BanditAllocationHistory,
  BanditAllocationRun,
  BanditObjective,
} from '../../../lib/api/bandit'

// ── Constants ────────────────────────────────────────────────────────────────

/** Reserved key in a run's allocation map — NOT an arm. */
export const BANDIT_OBJECTIVES_KEY = 'bandit_objectives'

const VARIANT_PALETTE = [
  '#8B8D96', // control grey
  '#5BAEF5', // blue
  '#3DD68C', // green
  '#A892FF', // violet
  '#E6B14F', // gold
  '#F26B5E', // coral
]

function paletteFor(variantKey: string, allKeys: string[]): string {
  const idx = allKeys.indexOf(variantKey)
  return VARIANT_PALETTE[idx === -1 ? 0 : idx % VARIANT_PALETTE.length]
}

// ── Pure helpers (exported for direct unit testing) ─────────────────────────

/** A single point in the allocation-over-time series. */
export interface AllocationPoint {
  /** RFC3339 timestamp of the run. */
  firedAt: string
  /** Per-arm weight in basis points. */
  weights: Record<string, number>
}

/**
 * Build the per-arm allocation series from the history runs (chronological
 * order, oldest → newest). Only `reallocate`/`commit`/`rollout` runs that
 * actually carry a `new_allocation` contribute a point. The reserved
 * `bandit_objectives` key is excluded; only numeric arm weights are kept.
 *
 * `runs` arrive newest-first from the gateway; the returned series is sorted
 * oldest-first so the chart reads left → right in time.
 */
export function buildAllocationSeries(
  runs: BanditAllocationRun[],
): AllocationPoint[] {
  const points: AllocationPoint[] = []
  for (const run of runs) {
    const alloc = run.new_allocation
    if (!alloc || typeof alloc !== 'object') continue
    const weights: Record<string, number> = {}
    for (const [key, value] of Object.entries(alloc)) {
      if (key === BANDIT_OBJECTIVES_KEY) continue
      if (typeof value === 'number' && Number.isFinite(value)) {
        weights[key] = value
      }
    }
    if (Object.keys(weights).length === 0) continue
    points.push({ firedAt: run.fired_at, weights })
  }
  // Oldest-first for a left→right time axis.
  return points
    .slice()
    .sort((a, b) => a.firedAt.localeCompare(b.firedAt))
}

/** The set of arm keys present across all points, in first-seen order. */
export function allocationArmKeys(points: AllocationPoint[]): string[] {
  const seen = new Set<string>()
  const order: string[] = []
  for (const p of points) {
    for (const k of Object.keys(p.weights)) {
      if (!seen.has(k)) {
        seen.add(k)
        order.push(k)
      }
    }
  }
  return order.sort()
}

export type ConvergenceState =
  | { kind: 'rolled_out'; variant: string; prob?: number }
  | { kind: 'committed'; variant?: string; prob?: number }
  | { kind: 'converged'; variant: string; prob?: number }
  | { kind: 'exploring' }

/**
 * Derive the convergence/commit badge state from the bandit state.
 *
 * Precedence: rolled out (committed + campaign concluded / status) →
 * committed → converged (winner known, not yet committed) → exploring.
 *
 * "Rolled out" is committed AND the experiment is no longer running
 * (campaign_status concluded/finalized OR the commit fully promoted the
 * winner). We treat a committed bandit whose campaign reports a terminal
 * status as rolled out; otherwise a committed (but still-listed) bandit is
 * simply "Committed".
 */
export function deriveConvergenceState(state: BanditState): ConvergenceState {
  const winner = state.converged_variant
  const prob = state.converged_prob
  if (state.committed) {
    const status = (state.campaign_status ?? '').toLowerCase()
    if (status === 'concluded' || status === 'finalized' || status === 'completed') {
      return { kind: 'rolled_out', variant: winner ?? '', prob }
    }
    return { kind: 'committed', variant: winner, prob }
  }
  if (state.has_converged && winner) {
    return { kind: 'converged', variant: winner, prob }
  }
  return { kind: 'exploring' }
}

/** Human label for a convergence state, e.g. "Converged: treatment (96%)". */
export function convergenceLabel(s: ConvergenceState): string {
  const pct = (p?: number) =>
    p != null && Number.isFinite(p) ? ` (${Math.round(p * 100)}%)` : ''
  switch (s.kind) {
    case 'rolled_out':
      return s.variant ? `Rolled out: ${s.variant}${pct(s.prob)}` : 'Rolled out'
    case 'committed':
      return s.variant ? `Committed: ${s.variant}${pct(s.prob)}` : 'Committed'
    case 'converged':
      return `Converged: ${s.variant}${pct(s.prob)}`
    case 'exploring':
      return 'Exploring'
  }
}

function bpToPct(bp: number): string {
  return `${(bp / 100).toFixed(1)}%`
}

// ── Convergence badge ────────────────────────────────────────────────────────

function ConvergenceBadge({ state }: { state: BanditState }) {
  const s = deriveConvergenceState(state)
  const tone =
    s.kind === 'rolled_out' || s.kind === 'committed'
      ? 'success'
      : s.kind === 'converged'
        ? 'accent'
        : ''
  return (
    <span
      className={`badge ${tone}`}
      data-convergence={s.kind}
      style={{ fontSize: 12 }}
    >
      {convergenceLabel(s)}
    </span>
  )
}

// ── Allocation-over-time chart ───────────────────────────────────────────────

/**
 * SVG line chart of per-arm allocation (bp) over time. Reuses the Timeseries
 * polyline pattern. The Y-axis is fixed to [0, 10000] bp (0–100%).
 */
function AllocationChart({ runs }: { runs: BanditAllocationRun[] }) {
  const points = buildAllocationSeries(runs)
  const armKeys = allocationArmKeys(points)

  if (points.length === 0 || armKeys.length === 0) {
    return (
      <div
        data-testid="allocation-chart"
        className="card"
        style={{
          padding: 32,
          textAlign: 'center',
          color: 'var(--fg-muted)',
          fontSize: 13,
        }}
      >
        No allocation history yet. Once the bandit reallocates, per-arm weights
        will appear here over time.
      </div>
    )
  }

  const w = 720
  const h = 240
  const padL = 44
  const padR = 12
  const padT = 14
  const padB = 28
  const yMax = 10000

  const xScale = (i: number) =>
    points.length === 1
      ? padL + (w - padL - padR) / 2
      : padL + (i / (points.length - 1)) * (w - padL - padR)
  const yScale = (bp: number) =>
    padT + (1 - bp / yMax) * (h - padT - padB)

  return (
    <div data-testid="allocation-chart" className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title">Allocation over time</div>
        <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
          {points.length} reallocation{points.length === 1 ? '' : 's'} ·{' '}
          {armKeys.length} arm{armKeys.length === 1 ? '' : 's'}
        </span>
      </div>
      <div style={{ padding: 14, overflowX: 'auto' }}>
        <svg viewBox={`0 0 ${w} ${h}`} style={{ width: '100%', maxWidth: w }}>
          {/* Gridlines + Y-axis labels at 0/25/50/75/100% */}
          {[0, 0.25, 0.5, 0.75, 1].map((t) => {
            const y = padT + t * (h - padT - padB)
            return (
              <g key={t}>
                <line
                  x1={padL}
                  x2={w - padR}
                  y1={y}
                  y2={y}
                  stroke="var(--border-faint)"
                  strokeDasharray={t === 1 ? undefined : '2 3'}
                />
                <text
                  x={padL - 6}
                  y={y + 3}
                  fontSize="10"
                  fill="var(--fg-muted)"
                  textAnchor="end"
                >
                  {Math.round((1 - t) * 100)}%
                </text>
              </g>
            )
          })}
          {/* Per-arm polylines */}
          {armKeys.map((key) => {
            const color = paletteFor(key, armKeys)
            const pts = points
              .map((p, i) =>
                p.weights[key] != null
                  ? `${xScale(i)},${yScale(p.weights[key])}`
                  : null,
              )
              .filter((s): s is string => s != null)
              .join(' ')
            return (
              <g key={key}>
                <polyline
                  points={pts}
                  fill="none"
                  stroke={color}
                  strokeWidth="2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
                {points.map((p, i) =>
                  p.weights[key] != null ? (
                    <circle
                      key={`${key}-${i}`}
                      cx={xScale(i)}
                      cy={yScale(p.weights[key])}
                      r={2.5}
                      fill={color}
                    >
                      <title>{`${key} · ${p.firedAt}: ${bpToPct(p.weights[key])}`}</title>
                    </circle>
                  ) : null,
                )}
              </g>
            )
          })}
        </svg>
        <div
          style={{
            display: 'flex',
            gap: 14,
            fontSize: 11,
            color: 'var(--fg-muted)',
            marginTop: 8,
            flexWrap: 'wrap',
          }}
        >
          {armKeys.map((key) => (
            <span
              key={key}
              style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
            >
              <span
                style={{ width: 10, height: 2, background: paletteFor(key, armKeys) }}
              />
              {key}
            </span>
          ))}
        </div>
      </div>
    </div>
  )
}

// ── Current weights table ─────────────────────────────────────────────────────

function CurrentWeights({ state }: { state: BanditState }) {
  const rows = [...state.current_allocation].sort(
    (a, b) => b.weight_bp - a.weight_bp,
  )
  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title">Current weights</div>
      </div>
      {rows.length === 0 ? (
        <div style={{ padding: 24, color: 'var(--fg-muted)', fontSize: 13 }}>
          No current allocation recorded yet.
        </div>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>Variant</th>
              <th style={{ textAlign: 'right' }}>Weight</th>
              <th style={{ textAlign: 'right' }}>bp</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => {
              const isWinner =
                state.converged_variant === r.variant_key &&
                (state.has_converged || state.committed)
              return (
                <tr key={r.variant_key} data-winner={isWinner ? 'true' : undefined}>
                  <td>
                    <span className={`badge ${isWinner ? 'accent' : ''}`}>
                      {r.variant_key}
                    </span>
                  </td>
                  <td style={{ textAlign: 'right', fontFamily: 'var(--font-mono)' }}>
                    {bpToPct(r.weight_bp)}
                  </td>
                  <td style={{ textAlign: 'right', fontFamily: 'var(--font-mono)' }}>
                    {r.weight_bp}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      )}
    </div>
  )
}

// ── Reward posteriors per objective ──────────────────────────────────────────

function ObjectiveCard({ objective }: { objective: BanditObjective }) {
  const roleLabel =
    objective.weight != null
      ? `${objective.role} (w=${objective.weight})`
      : objective.role
  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title">
          Objective · {objective.metric_id}
        </div>
        <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
          {roleLabel} · goal: {objective.goal}
        </span>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th>Variant</th>
            <th style={{ textAlign: 'right' }}>Mean</th>
            <th style={{ textAlign: 'right' }}>95% CI</th>
            <th style={{ textAlign: 'right' }}>n</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {objective.variants.map((v) => (
            <tr
              key={v.variant_key}
              data-guardrail-violated={v.guardrail_violated ? 'true' : undefined}
            >
              <td>
                <span className="badge">{v.variant_key}</span>
              </td>
              <td style={{ textAlign: 'right', fontFamily: 'var(--font-mono)' }}>
                {v.mean.toFixed(4)}
              </td>
              <td style={{ textAlign: 'right', fontFamily: 'var(--font-mono)' }}>
                [{v.ci_lower.toFixed(4)}, {v.ci_upper.toFixed(4)}]
              </td>
              <td style={{ textAlign: 'right', fontFamily: 'var(--font-mono)' }}>
                {Number(v.n).toLocaleString('en-US')}
              </td>
              <td>
                {v.guardrail_violated && (
                  <span className="badge warning" data-testid="guardrail-badge">
                    Guardrail violated
                  </span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// ── Lifecycle + campaign timeline ─────────────────────────────────────────────

function actionLabel(action: string): string {
  switch (action) {
    case 'reallocate':
      return 'Reallocated'
    case 'commit':
      return 'Committed winner'
    case 'rollout':
      return 'Rolled out winner'
    case 'spawn_iteration':
      return 'Spawned next iteration'
    case 'skip':
      return 'Skipped (no change)'
    default:
      return action
  }
}

function Timeline({
  history,
  state,
}: {
  history: BanditAllocationHistory
  state: BanditState
}) {
  const runs = history.runs ?? []
  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title">Lifecycle &amp; campaign timeline</div>
        {state.campaign_id && (
          <span
            className="badge"
            data-testid="campaign-status"
            style={{ fontSize: 11 }}
          >
            Campaign: {state.campaign_status ?? 'active'}
          </span>
        )}
      </div>
      {runs.length === 0 ? (
        <div style={{ padding: 24, color: 'var(--fg-muted)', fontSize: 13 }}>
          No lifecycle actions recorded yet.
        </div>
      ) : (
        <ul
          aria-label="Lifecycle timeline"
          style={{ listStyle: 'none', margin: 0, padding: '8px 14px' }}
        >
          {runs.map((run, i) => (
            <li
              key={`${run.fired_at}-${i}`}
              data-action={run.action}
              data-outcome={run.outcome}
              style={{
                display: 'flex',
                gap: 10,
                alignItems: 'baseline',
                padding: '6px 0',
                borderBottom:
                  i < runs.length - 1 ? '1px solid var(--border-faint)' : undefined,
              }}
            >
              <span
                style={{
                  fontFamily: 'var(--font-mono)',
                  fontSize: 11,
                  color: 'var(--fg-muted)',
                  flexShrink: 0,
                }}
              >
                {run.fired_at}
              </span>
              <span style={{ fontWeight: 600, fontSize: 13 }}>
                {actionLabel(run.action)}
              </span>
              <span
                className={`badge ${run.outcome === 'failed' ? 'warning' : run.outcome === 'applied' ? 'success' : ''}`}
                style={{ fontSize: 10 }}
              >
                {run.outcome}
              </span>
              {run.detail && (
                <span style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
                  {run.detail}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

// ── Bandit tab ────────────────────────────────────────────────────────────────

interface Props {
  state: BanditState | null
  history: BanditAllocationHistory | null
  loading: boolean
  error: string | null
}

export function BanditTab({ state, history, loading, error }: Props) {
  if (loading) {
    return (
      <div
        style={{
          padding: 32,
          textAlign: 'center',
          color: 'var(--fg-muted)',
          fontSize: 13,
        }}
      >
        Loading bandit state…
      </div>
    )
  }

  if (error) {
    return (
      <div role="alert" className="card" style={{ padding: 16, color: 'var(--danger)' }}>
        {error}
      </div>
    )
  }

  // Defensive: the tab should only be mounted for a bandit experiment, but
  // render nothing meaningful when the state says otherwise.
  if (!state || !state.is_bandit) {
    return (
      <div
        data-testid="bandit-not-applicable"
        className="card"
        style={{
          padding: 32,
          textAlign: 'center',
          color: 'var(--fg-muted)',
          fontSize: 13,
        }}
      >
        This experiment is not running in bandit mode.
      </div>
    )
  }

  const objectives = state.objectives?.objectives ?? []
  const algorithm = state.bandit_config?.algorithm

  return (
    <div data-testid="bandit-view">
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: 14,
          padding: '10px 14px',
          background: 'var(--bg-sunken)',
          border: '1px solid var(--border)',
          borderRadius: 8,
        }}
      >
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
          {algorithm ? (
            <>
              Algorithm: <strong style={{ color: 'var(--fg)' }}>{algorithm}</strong>
            </>
          ) : (
            'Adaptive bandit'
          )}
        </div>
        <ConvergenceBadge state={state} />
      </div>

      <AllocationChart runs={history?.runs ?? []} />

      <CurrentWeights state={state} />

      {objectives.map((obj, i) => (
        <ObjectiveCard key={`${obj.metric_id}-${obj.role}-${i}`} objective={obj} />
      ))}

      <Timeline history={history ?? { runs: [] }} state={state} />
    </div>
  )
}
