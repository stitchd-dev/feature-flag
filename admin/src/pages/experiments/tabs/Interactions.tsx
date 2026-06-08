/**
 * Interactions tab — N-way cross-experiment interactions (P5, P10/P12).
 *
 * Renders interaction estimates of ANY order (pairwise, three-way, and
 * operator-bounded four-way and higher) between this experiment and other
 * concurrently-running experiments, scoped by context type + metric. Each row
 * shows participating experiments, an order badge, the interaction term,
 * context type, metric, shared exposure count, the frequentist estimate/p-value,
 * and Bayesian columns.
 *
 * Rows are grouped by interaction order (ascending) then sorted by metric_key
 * for readability; nothing in this component assumes a maximum order of 3.
 *
 * Pure-render — the parent (`ExperimentDetail`) fetches the data and passes it
 * in, mirroring the Exposures / Iterations tab convention. Exercised via
 * `renderToString` in tests.
 */
import { I } from '../../../components/icons'
import { LoadingSpinner } from '../../../components/LoadingSpinner'
import { ErrorBanner } from '../../../components/ErrorBanner'
import { EmptyState } from '../../../components/EmptyState'
import type { ExperimentInteraction } from '../../../lib/api/exclusionGroups'

interface Props {
  interactions: ExperimentInteraction[]
  loading: boolean
  error: string | null
  /**
   * True when THIS experiment is running in bandit mode. Surfaces a note that
   * SRM is bandit-skipped and interactions are estimated under a shifting
   * allocation (the n-weighted estimators handle unequal cells — see
   * Phase 10 learnings).
   */
  isBandit?: boolean
}

/** Format a p-value to 4 decimals, or "—" when null/NaN. */
export function formatPValue(p: number | null | undefined): string {
  if (p == null || Number.isNaN(p)) return '—'
  return p.toFixed(4)
}

/** Format an interaction estimate with a sign, to 4 decimals. */
export function formatEstimate(estimate: number | null | undefined): string {
  if (estimate == null || Number.isNaN(estimate)) return '—'
  const fixed = estimate.toFixed(4)
  return estimate > 0 ? `+${fixed}` : fixed
}

/** Format a Bayesian probability as a percentage, to 1 decimal place. */
export function formatBayesProb(prob: number | null | undefined): string {
  if (prob == null || Number.isNaN(prob)) return '—'
  return `${(prob * 100).toFixed(1)}%`
}

/** Format a Bayesian credible interval as "[low, high]" to 4 decimals each. */
export function formatBayesCI(
  low: number | null | undefined,
  high: number | null | undefined,
): string {
  if (low == null || high == null || Number.isNaN(low) || Number.isNaN(high))
    return '—'
  return `[${low.toFixed(4)}, ${high.toFixed(4)}]`
}

/**
 * The first 26 factor labels (A..Z); falls back to "F{n}" beyond Z so a
 * (hypothetical) >26-way term never collides. Used to render an A×B×C×… label.
 */
function factorLabels(n: number): string {
  const letters: string[] = []
  for (let i = 0; i < n; i++) {
    letters.push(i < 26 ? String.fromCharCode(65 + i) : `F${i + 1}`)
  }
  return letters.join('×')
}

/**
 * Humanize the interaction term for display. Generic over interaction order —
 * does NOT cap at three-way.
 *
 *   main:<uuid>          → "Main effect"
 *   2way:…               → "A×B"
 *   3way:…               → "A×B×C"
 *   {N}way:… (N ≥ 4)     → "A×B×C×D…" (N factors)
 *   Fallback (order ≥ 1) → A×B×… derived from `order`
 *   Fallback (unknown)   → raw term string
 */
export function formatTerm(term: string, order: number): string {
  // The term prefix is authoritative (it names the actual effect); `order` is
  // only a fallback for an unprefixed term.
  if (term.startsWith('main:')) return 'Main effect'
  // Match any `{N}way:` prefix (handles 2way/3way and operator-bounded 4+).
  const m = /^(\d+)way:/.exec(term)
  if (m) {
    const n = Number(m[1])
    return n <= 1 ? 'Main effect' : factorLabels(n)
  }
  if (order === 1) return 'Main effect'
  if (order >= 2) return factorLabels(order)
  return term
}

/** Human label for an interaction order, generic over any N (no 3-way cap). */
export function orderLabel(order: number): string {
  if (order === 1) return 'Main'
  return `${order}-way`
}

/**
 * True when the experiment has a *real* significant cross-experiment
 * interaction — i.e. a non-insufficient INTERACTION term (2-way/3-way, never a
 * `main:` effect) that is frequentist-significant after the sweep's
 * Benjamini–Hochberg correction.
 *
 * Deliberately excludes: (a) `main:` rows — a single-experiment main effect is
 * not a cross-experiment interaction and would otherwise fire the banner on any
 * well-powered experiment; (b) the per-row Bayesian `bayes_prob` — it is an
 * UNcorrected directional posterior (≈0.5 under the null, easily >0.95 for a
 * modest effect) and is not multiplicity-controlled, so gating the page-level
 * warning on it produces false alarms and contradicts the per-row frequentist
 * badge. Bayesian columns remain shown per row for context.
 */
export function hasSignificantInteraction(
  interactions: ExperimentInteraction[],
): boolean {
  return interactions.some(
    (i) =>
      !i.insufficient_data && i.significant && !i.term.startsWith('main:'),
  )
}

/**
 * Sort interactions by interaction_order ascending (so rows group by order:
 * main → pairwise → three-way → four-way → …), then metric_key alphabetically,
 * then by term for stable ordering. Generic over any order — never caps at 3.
 */
export function sortInteractions(
  rows: ExperimentInteraction[],
): ExperimentInteraction[] {
  return [...rows].sort((a, b) => {
    if (a.interaction_order !== b.interaction_order)
      return a.interaction_order - b.interaction_order
    const byMetric = a.metric_key.localeCompare(b.metric_key)
    if (byMetric !== 0) return byMetric
    return a.term.localeCompare(b.term)
  })
}

/** The distinct interaction orders present, ascending. */
export function distinctOrders(rows: ExperimentInteraction[]): number[] {
  return Array.from(new Set(rows.map((r) => r.interaction_order))).sort(
    (a, b) => a - b,
  )
}

export function InteractionsTab({
  interactions,
  loading,
  error,
  isBandit = false,
}: Props) {
  if (loading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <LoadingSpinner label="Loading interactions…" />
      </div>
    )
  }

  if (error) {
    return <ErrorBanner message={error} icon={<I.alert size={14} />} />
  }

  if (interactions.length === 0) {
    return (
      <div className="card">
        <EmptyState
          icon={<I.branch size={20} />}
          title="No interactions detected"
          desc="No overlapping experiments share enough traffic to estimate an interaction effect."
        />
      </div>
    )
  }

  const sorted = sortInteractions(interactions)
  const orders = distinctOrders(sorted)
  const maxOrder = orders.length > 0 ? orders[orders.length - 1] : 0

  return (
    <div>
      {isBandit && (
        <div
          role="note"
          data-testid="bandit-interaction-note"
          style={{
            display: 'flex',
            alignItems: 'flex-start',
            gap: 8,
            padding: '10px 14px',
            marginBottom: 14,
            borderRadius: 8,
            background: 'var(--bg-sunken)',
            border: '1px solid var(--border)',
            color: 'var(--fg-muted)',
            fontSize: 12,
          }}
        >
          <I.beaker size={14} style={{ flexShrink: 0, marginTop: 1 }} />
          <span>
            This experiment is a bandit. SRM is bandit-skipped (the realized
            allocation shifts by design), and interaction estimates are computed
            on n-weighted cells, so unequal arm sizes under adaptive allocation
            are handled.
          </span>
        </div>
      )}
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: 10,
          fontSize: 12,
          color: 'var(--fg-muted)',
        }}
      >
        <span>
          {sorted.length} term{sorted.length === 1 ? '' : 's'} ·{' '}
          {orders.map((o) => orderLabel(o)).join(', ')}
        </span>
        {maxOrder >= 4 && (
          <span className="badge" data-testid="high-order-present">
            Includes {maxOrder}-way
          </span>
        )}
      </div>
      <div className="card" style={{ overflowX: 'auto' }}>
        <table className="table" style={{ marginBottom: 0 }}>
        <thead>
          <tr>
            <th>Interaction</th>
            <th>Order</th>
            <th>Term</th>
            <th>Context type</th>
            <th>Metric</th>
            <th style={{ textAlign: 'right' }}>Shared</th>
            <th style={{ textAlign: 'right' }}>Estimate</th>
            <th style={{ textAlign: 'right' }}>P-value</th>
            <th style={{ textAlign: 'right' }}>Bayes prob</th>
            <th style={{ textAlign: 'right' }}>Bayes expected</th>
            <th style={{ textAlign: 'right' }}>Credible interval</th>
            <th>Significance</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((row, i) => {
            const experimentLabel = row.experiment_names.join(' · ')

            return (
              <tr key={`${row.term}-${row.context_type}-${row.metric_key}-${i}`}>
                <td>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
                    <span style={{ fontWeight: 600, fontSize: 13 }}>
                      {experimentLabel}
                    </span>
                  </div>
                </td>
                <td>
                  <span
                    className="badge"
                    data-order={row.interaction_order}
                    style={{ fontSize: 10 }}
                  >
                    {orderLabel(row.interaction_order)}
                  </span>
                </td>
                <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                  {formatTerm(row.term, row.interaction_order)}
                </td>
                <td>
                  <span className="badge">{row.context_type}</span>
                </td>
                <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                  {row.metric_key}
                </td>
                <td
                  style={{
                    textAlign: 'right',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 12,
                  }}
                >
                  {Number(row.shared_count).toLocaleString('en-US')}
                </td>
                <td
                  style={{
                    textAlign: 'right',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 12,
                  }}
                >
                  {row.insufficient_data ? '—' : formatEstimate(row.interaction_estimate)}
                </td>
                <td
                  style={{
                    textAlign: 'right',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 12,
                  }}
                >
                  {row.insufficient_data ? '—' : formatPValue(row.p_value)}
                </td>
                <td
                  style={{
                    textAlign: 'right',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 12,
                  }}
                >
                  {row.insufficient_data ? '—' : formatBayesProb(row.bayes_prob)}
                </td>
                <td
                  style={{
                    textAlign: 'right',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 12,
                  }}
                >
                  {row.insufficient_data ? '—' : formatEstimate(row.bayes_expected)}
                </td>
                <td
                  style={{
                    textAlign: 'right',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 12,
                  }}
                >
                  {row.insufficient_data
                    ? '—'
                    : formatBayesCI(row.bayes_ci_low, row.bayes_ci_high)}
                </td>
                <td>
                  {row.insufficient_data ? (
                    <span className="badge" data-insufficient="true">
                      Insufficient data
                    </span>
                  ) : (
                    <span
                      className={`badge ${row.significant ? 'warning' : 'success'}`}
                      data-significant={row.significant}
                    >
                      {row.significant ? 'Significant' : 'Not significant'}
                    </span>
                  )}
                </td>
              </tr>
            )
          })}
        </tbody>
        </table>
      </div>
    </div>
  )
}
