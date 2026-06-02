/**
 * Interactions tab — Phase 8 (xexp, P8.T3).
 *
 * Renders the pairwise interaction estimates between this experiment and other
 * concurrently-running experiments, scoped by context type + metric. Each row
 * shows the other experiment, context type, metric, shared exposure count, the
 * interaction estimate, p-value, and a significance badge.
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

/**
 * True when any interaction in the list is a *real* statistically significant
 * result. Rows flagged `insufficient_data` carry 0.0 sentinels for their
 * estimate/p-value and must never be counted as significant.
 */
export function hasSignificantInteraction(
  interactions: ExperimentInteraction[],
): boolean {
  return interactions.some((i) => i.significant && !i.insufficient_data)
}

export function InteractionsTab({ interactions, loading, error }: Props) {
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

  return (
    <div className="card">
      <table className="table" style={{ marginBottom: 0 }}>
        <thead>
          <tr>
            <th>Other experiment</th>
            <th>Context type</th>
            <th>Metric</th>
            <th style={{ textAlign: 'right' }}>Shared</th>
            <th style={{ textAlign: 'right' }}>Interaction estimate</th>
            <th style={{ textAlign: 'right' }}>P-value</th>
            <th>Significance</th>
          </tr>
        </thead>
        <tbody>
          {interactions.map((row, i) => (
            <tr key={`${row.experiment_id_b}-${row.context_type}-${row.metric_key}-${i}`}>
              <td>
                <span style={{ fontWeight: 600, fontSize: 13 }}>
                  {row.other_experiment_name}
                </span>
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
          ))}
        </tbody>
      </table>
    </div>
  )
}
