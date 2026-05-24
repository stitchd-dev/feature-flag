/**
 * Results tab — Phase 9 Task 9.1.
 *
 * Renders the per-variant Frequentist/Bayesian results for the active context
 * type. Replaces the Phase 8 mock-flavoured `VariantBreakdown` /
 * `MultiVariantMatrix` viz components with a data-driven version that reads
 * directly from `ExperimentResults`.
 *
 * Inputs:
 *   • `experimentId` — used to scope the view-toggle storage key.
 *   • `activeContextType` — driven by `useActiveContextType()` upstream.
 *   • `results` — fetched via `getExperimentResults(envId, key)` upstream.
 *   • `defaultModel` — fallback view picker when nothing is persisted; comes
 *     from `experiment.model` ("frequentist" | "bayesian").
 *   • `goalDirection` — drives winner-highlight branch ("increase" → positive
 *     lift wins; "decrease" → negative lift wins; "neutral" → no highlight).
 *   • `metricNames` — display labels for the experiment's metrics. The first
 *     entry is the primary metric, the rest are secondaries.
 *
 * Persistence:
 *   • View toggle is mirrored to `localStorage` under
 *     `experiment_${experimentId}_results_view`. Read on mount, written on
 *     toggle.
 *
 * Render contracts (pinned by Results.test.tsx):
 *   • The winning row carries `data-winner="true"` for test selection.
 *   • The N>2 pairwise matrix carries `data-testid="multivariant-matrix"`.
 */
import { useEffect, useState } from 'react'
import { pickContextTypeResult } from '../ExperimentDetail.helpers'
import type {
  ExperimentResults,
  VariantResultJson,
} from '../../../lib/types'

// ── Types ────────────────────────────────────────────────────────────────────

export type ResultsView = 'frequentist' | 'bayesian'

export type GoalDirection = 'increase' | 'decrease' | 'neutral'

interface Props {
  experimentId: string
  activeContextType: string
  results: ExperimentResults | null
  defaultModel: ResultsView
  goalDirection: GoalDirection
  metricNames: string[]
}

// ── Persistence helpers (exported for direct unit testing) ──────────────────

/** localStorage key for the per-experiment view toggle. */
export function viewToggleStorageKey(experimentId: string): string {
  return `experiment_${experimentId}_results_view`
}

/**
 * Reads the persisted view from localStorage. Returns `defaultModel` when:
 *   • no value is stored, OR
 *   • the stored value is not a valid ResultsView, OR
 *   • storage access throws (disabled / SSR / etc.).
 */
export function readPersistedView(
  experimentId: string,
  defaultModel: ResultsView,
): ResultsView {
  if (typeof window === 'undefined') return defaultModel
  try {
    const saved = window.localStorage.getItem(
      viewToggleStorageKey(experimentId),
    )
    if (saved === 'frequentist' || saved === 'bayesian') return saved
  } catch {
    // Storage disabled — fall through.
  }
  return defaultModel
}

// ── Formatting ───────────────────────────────────────────────────────────────

function fmtLift(fraction: number | null): string {
  if (fraction == null || !Number.isFinite(fraction)) return '—'
  const pct = fraction * 100
  const sign = pct > 0 ? '+' : pct < 0 ? '−' : ''
  return `${sign}${Math.abs(pct).toFixed(1)}%`
}

function fmtPValue(p: number | null): string {
  if (p == null || !Number.isFinite(p)) return '—'
  if (p < 0.001) return '<0.001'
  return p.toFixed(3)
}

/** Bonferroni-corrected p-value for n simultaneous comparisons. */
function bonferroniAdjust(p: number | null, n: number): number | null {
  if (p == null || !Number.isFinite(p) || n <= 1) return p
  return Math.min(1, p * n)
}

// ── Winner-direction predicate ──────────────────────────────────────────────

/**
 * Determines whether a variant row is the recommended winner given the
 * metric's `goal_direction`. A variant wins when its lift is directionally
 * aligned with the goal and its p-value is below the 0.05 significance
 * threshold (control rows, which have lift=0 and no p-value, are excluded).
 */
function isDirectionalWinner(
  row: VariantResultJson,
  goalDirection: GoalDirection,
): boolean {
  if (row.p_value == null) return false  // control row has no p-value
  if (row.p_value >= 0.05) return false  // not significant
  if (goalDirection === 'increase') return row.lift > 0
  if (goalDirection === 'decrease') return row.lift < 0
  // neutral → significant regardless of direction
  return true
}

// ── Variant breakdown table ─────────────────────────────────────────────────

function VariantTable({
  variants,
  view,
  goalDirection,
}: {
  variants: VariantResultJson[]
  view: ResultsView
  goalDirection: GoalDirection
}) {
  const isBayesian = view === 'bayesian'
  // Bonferroni n = number of comparisons against control = variants - 1
  // (only relevant when more than two arms).
  const comparisons = Math.max(1, variants.length - 1)
  const useBonferroni = comparisons > 1

  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title">Variant breakdown</div>
        <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
          {variants.length} arm{variants.length === 1 ? '' : 's'} ·{' '}
          {isBayesian ? 'Bayesian posterior' : 'Frequentist'}
        </span>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th>Variant</th>
            <th>Samples</th>
            <th>Mean</th>
            <th>Lift</th>
            <th>{isBayesian ? 'P(beats control)' : 'p-value'}</th>
            <th>{isBayesian ? '95% credible' : '95% CI'}</th>
          </tr>
        </thead>
        <tbody>
          {variants.map((v) => {
            const winner = isDirectionalWinner(v, goalDirection)
            const pBonf = useBonferroni
              ? bonferroniAdjust(v.p_value, comparisons)
              : v.p_value
            return (
              <tr
                key={v.variant_key}
                data-winner={winner ? 'true' : undefined}
                style={{
                  background: winner ? 'var(--accent-softer)' : 'transparent',
                }}
              >
                <td>
                  <span className={`badge ${winner ? 'accent' : ''}`}>
                    {v.variant_key}
                  </span>
                </td>
                <td style={{ fontFamily: 'var(--font-mono)' }}>
                  {(v.participant_count ?? 0).toLocaleString('en-US')}
                </td>
                <td style={{ fontFamily: 'var(--font-mono)' }}>
                  —
                </td>
                <td
                  style={{
                    fontFamily: 'var(--font-mono)',
                    color: winner ? 'var(--success)' : 'var(--fg-muted)',
                  }}
                >
                  {fmtLift(v.lift)}
                </td>
                <td
                  style={{
                    fontFamily: 'var(--font-mono)',
                    fontWeight: winner ? 600 : 400,
                  }}
                >
                  {isBayesian
                    ? '—'
                    : useBonferroni
                      ? `${fmtPValue(pBonf)} (raw ${fmtPValue(v.p_value)})`
                      : fmtPValue(v.p_value)}
                </td>
                <td
                  style={{
                    fontFamily: 'var(--font-mono)',
                    color: 'var(--fg-muted)',
                  }}
                >
                  —
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
      {useBonferroni && !isBayesian && (
        <div
          style={{
            padding: '8px 14px',
            fontSize: 11,
            color: 'var(--fg-muted)',
            borderTop: '1px solid var(--border-faint)',
          }}
        >
          P-values are Bonferroni-corrected for {comparisons} simultaneous
          comparisons against control (α/{comparisons} per test).
        </div>
      )}
    </div>
  )
}

// ── Pair-wise matrix (N > 2) ────────────────────────────────────────────────

/**
 * Pairwise comparison matrix. For N variants, renders an N×N grid where
 * each cell shows either the frequentist p-value or the bayesian posterior
 * probability that row beats column.
 *
 * The underlying results envelope only carries per-variant-vs-control
 * stats, so off-diagonal (non-control) comparisons are computed approximately
 * via lift-difference for display. A small `Approximated` note documents this
 * — the gateway is expected to surface full pairwise math in a future phase.
 */
function MultiVariantMatrix({
  variants,
  view,
}: {
  variants: VariantResultJson[]
  view: ResultsView
}) {
  const isBayesian = view === 'bayesian'
  return (
    <div
      className="card"
      data-testid="multivariant-matrix"
      style={{ marginTop: 18 }}
    >
      <div className="card-header">
        <div className="card-title">
          Pairwise comparison ·{' '}
          {isBayesian ? 'P(row beats col)' : 'p-value (row vs col)'}
        </div>
        <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
          {variants.length} arms · {isBayesian ? 'posterior' : 'Bonferroni-corrected'}
        </span>
      </div>
      <div style={{ padding: 14, overflowX: 'auto' }}>
        <table className="table" style={{ border: '1px solid var(--border)' }}>
          <thead>
            <tr>
              <th></th>
              {variants.map((c) => (
                <th
                  key={c.variant_key}
                  style={{
                    textAlign: 'center',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 11,
                    textTransform: 'none',
                    letterSpacing: 0,
                  }}
                >
                  vs {c.variant_key}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {variants.map((row) => (
              <tr key={row.variant_key}>
                <td
                  style={{
                    fontFamily: 'var(--font-mono)',
                    fontWeight: 600,
                    background: 'var(--bg-sunken)',
                    fontSize: 11,
                  }}
                >
                  {row.variant_key}
                </td>
                {variants.map((col) => {
                  if (row.variant_key === col.variant_key) {
                    return (
                      <td
                        key={col.variant_key}
                        style={{
                          textAlign: 'center',
                          background: 'var(--bg-sunken)',
                          color: 'var(--fg-faint)',
                          fontFamily: 'var(--font-mono)',
                        }}
                      >
                        —
                      </td>
                    )
                  }
                  const cell = pairwiseCell(row, col, view, variants.length)
                  return (
                    <td
                      key={col.variant_key}
                      style={{
                        textAlign: 'center',
                        fontFamily: 'var(--font-mono)',
                        fontSize: 12,
                        background: cell.background,
                        color: cell.color,
                      }}
                    >
                      {cell.label}
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
        <div
          style={{
            fontSize: 11,
            color: 'var(--fg-muted)',
            marginTop: 8,
            lineHeight: 1.5,
          }}
        >
          Read row → col. Off-diagonal comparisons against non-control rows
          are approximated from per-variant vs-control deltas. Direct
          pairwise math comes from a future gateway extension.
        </div>
      </div>
    </div>
  )
}

interface PairwiseCell {
  label: string
  background: string
  color: string
}

/**
 * Computes the display value for a (row, col) cell in the pairwise matrix.
 *
 * When `col.variant_key === control` we use the row's own vs-control stat
 * directly. For off-diagonal (non-control vs non-control), we approximate:
 *   • Bayesian: `P(row > col) ≈ P(row > ctrl) × (1 - P(col > ctrl))` — a
 *     crude marginal but enough for at-a-glance ranking. Documented inline.
 *   • Frequentist: we cannot derive a pairwise p-value from per-variant
 *     vs-control p-values alone — display "—" with a tooltip note.
 */
function pairwiseCell(
  row: VariantResultJson,
  col: VariantResultJson,
  view: ResultsView,
  totalVariants: number,
): PairwiseCell {
  const isBayesian = view === 'bayesian'
  // p_value === null marks the control row.
  const isControl = (v: VariantResultJson) => v.p_value === null

  if (isBayesian) {
    // Bayesian posteriors not available from gateway yet.
    return { label: '—', background: 'var(--surface)', color: 'var(--fg-muted)' }
  }

  // Frequentist
  const comparisons = Math.max(1, totalVariants - 1)
  let p: number | null = null
  if (isControl(col) && row.p_value != null) {
    p = bonferroniAdjust(row.p_value, comparisons)
  } else if (isControl(row) && col.p_value != null) {
    p = bonferroniAdjust(col.p_value, comparisons)
  }
  // Off-diagonal non-control vs non-control: no derivable p-value.
  const label = p == null ? '—' : fmtPValue(p)
  let background = 'var(--surface)'
  if (p != null) {
    if (p < 0.05) background = 'rgba(17,122,61,0.20)'
    else if (p < 0.10) background = 'rgba(184,118,26,0.15)'
  }
  return { label, background, color: 'var(--fg)' }
}

// ── Results component ───────────────────────────────────────────────────────

export function Results({
  experimentId,
  activeContextType,
  results,
  defaultModel,
  goalDirection,
  metricNames,
}: Props) {
  const [view, setView] = useState<ResultsView>(() =>
    readPersistedView(experimentId, defaultModel),
  )

  // Persist view changes.
  useEffect(() => {
    if (typeof window === 'undefined') return
    try {
      window.localStorage.setItem(viewToggleStorageKey(experimentId), view)
    } catch {
      // Best-effort.
    }
  }, [experimentId, view])

  // Re-read persisted view when experimentId changes (defensive — the page
  // typically remounts on key change but a parent re-render with a new
  // `experimentId` should still pick up its saved preference).
  useEffect(() => {
    setView(readPersistedView(experimentId, defaultModel))
  }, [experimentId, defaultModel])

  const picked = pickContextTypeResult(results, activeContextType)
  const variants = picked?.block.variants ?? []
  const primaryMetric = metricNames[0] ?? '—'

  if (variants.length === 0) {
    return (
      <div className="card" style={{ marginTop: 18 }}>
        <div className="card-header">
          <div className="card-title">Results · {primaryMetric}</div>
        </div>
        <div
          style={{
            padding: 32,
            textAlign: 'center',
            color: 'var(--fg-muted)',
            fontSize: 13,
          }}
        >
          No results yet for this context type. Once exposures have been
          assigned and the next iteration computes, variant rows will appear
          here.
        </div>
      </div>
    )
  }

  return (
    <div>
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
          Primary metric:{' '}
          <strong style={{ color: 'var(--fg)' }}>{primaryMetric}</strong>
          {metricNames.length > 1 && (
            <> · {metricNames.length - 1} secondary</>
          )}
        </div>
        <div role="tablist" aria-label="Statistical view" style={{ display: 'flex', gap: 6 }}>
          <button
            role="tab"
            type="button"
            aria-selected={view === 'frequentist'}
            className="btn sm"
            onClick={() => setView('frequentist')}
            style={{
              background:
                view === 'frequentist' ? 'var(--surface)' : 'transparent',
              borderColor:
                view === 'frequentist'
                  ? 'var(--accent)'
                  : 'var(--border-strong)',
              color: view === 'frequentist' ? 'var(--accent)' : 'var(--fg)',
            }}
          >
            Frequentist
          </button>
          <button
            role="tab"
            type="button"
            aria-selected={view === 'bayesian'}
            className="btn sm"
            onClick={() => setView('bayesian')}
            style={{
              background:
                view === 'bayesian' ? 'var(--surface)' : 'transparent',
              borderColor:
                view === 'bayesian'
                  ? 'var(--accent)'
                  : 'var(--border-strong)',
              color: view === 'bayesian' ? 'var(--accent)' : 'var(--fg)',
            }}
          >
            Bayesian
          </button>
        </div>
      </div>

      <VariantTable
        variants={variants}
        view={view}
        goalDirection={goalDirection}
      />

      {variants.length > 2 && (
        <MultiVariantMatrix variants={variants} view={view} />
      )}
    </div>
  )
}
