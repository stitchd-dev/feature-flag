/**
 * Iterations history tab — Phase 9 Task 9.4.
 *
 * Renders:
 *   • A list of past iterations (number, started/ended, duration,
 *     unit_context_types snapshot) sorted by iteration_number descending.
 *   • A "Recompute now" button driving `POST /recompute` + polling against
 *     `GET /recompute/{job_id}` every 2 seconds until the job reaches a
 *     terminal status (succeeded / failed). Polling lifecycle is owned by the
 *     parent page so this component stays presentational + node-testable.
 *   • Click a row → in-pane past-iteration results (read-only). Reuses the
 *     <Results> component so the per-context-type rendering stays consistent
 *     with the live results tab.
 *
 * The component never fetches on its own — the page wires:
 *   • `iterations` from a future `listExperimentIterations()` wrapper (not
 *     part of Phase 8.1's contract; until the gateway surface lands, the
 *     parent passes `[]` and the tab renders its empty-state).
 *   • `selectedResults` from a future `getIterationResults(iter_id)` wrapper.
 *   • `recomputeStatus` from the parent's polling effect.
 *
 * IterationSummary is exported so the page can import the shape when wiring.
 */
import { Results } from './Results'
import type { ResultsView, GoalDirection } from './Results'
import type { ExperimentResults } from '../../../lib/types'

// ── Types ────────────────────────────────────────────────────────────────────

/**
 * Per-iteration summary surfaced in the history table. Field names match the
 * `experiment_iterations` PG row shape (see Phase 1 migration) so a future
 * `listExperimentIterations()` API wrapper can map 1:1.
 */
export interface IterationSummary {
  iteration_id: string
  iteration_number: number
  started_at: string
  /** Null when the iteration is still running (current iteration). */
  ended_at: string | null
  /** Snapshot of the experiment's unit_context_types at iteration start. */
  unit_context_types: string[]
  /** Timestamp when stats were last computed; null when no compute yet. */
  computed_at: string | null
}

// ── Pure helpers (exported for direct unit testing) ─────────────────────────

/**
 * Returns iterations sorted by iteration_number descending. Does NOT
 * mutate the input.
 */
export function sortIterationsDesc(
  iterations: IterationSummary[],
): IterationSummary[] {
  return [...iterations].sort(
    (a, b) => b.iteration_number - a.iteration_number,
  )
}

/**
 * Formats a [start, end] timestamp range into a short duration label
 * ("2h 14m", "3d 4h", "42m"). Returns "—" when start is null and
 * "ongoing" when start is set but end is null.
 */
export function formatDuration(
  startedAt: string | null,
  endedAt: string | null,
): string {
  if (!startedAt) return '—'
  if (!endedAt) return 'ongoing'
  const start = Date.parse(startedAt)
  const end = Date.parse(endedAt)
  if (!Number.isFinite(start) || !Number.isFinite(end)) return '—'
  let secs = Math.max(0, Math.floor((end - start) / 1000))
  const days = Math.floor(secs / 86400)
  secs -= days * 86400
  const hours = Math.floor(secs / 3600)
  secs -= hours * 3600
  const mins = Math.floor(secs / 60)
  if (days > 0) return `${days}d ${hours}h`
  if (hours > 0) return `${hours}h ${mins}m`
  return `${mins}m`
}

/** User-facing label for a recompute job status. */
export function recomputeStatusLabel(status: string): string {
  switch (status) {
    case 'queued':
      return 'Queued'
    case 'running':
      return 'Running…'
    case 'succeeded':
      return 'Completed'
    case 'failed':
      return 'Failed'
    default:
      return status
  }
}

/**
 * Whether the parent's poll loop should keep going for the given status.
 * Terminal states (succeeded / failed) stop polling. Unknown states stop
 * polling too — the safe default given we'd otherwise loop forever.
 */
export function shouldKeepPolling(status: string): boolean {
  return status === 'queued' || status === 'running'
}

// ── Formatters ───────────────────────────────────────────────────────────────

function fmtTimestamp(iso: string | null): string {
  if (!iso) return '—'
  try {
    return new Date(iso).toLocaleString()
  } catch {
    return iso
  }
}

// ── IterationsTab component ─────────────────────────────────────────────────

interface Props {
  iterations: IterationSummary[]
  loading: boolean
  error: string | null
  selectedIteration: IterationSummary | null
  selectedResults: ExperimentResults | null
  selectedLoading: boolean
  onIterationSelect: (iter: IterationSummary | null) => void
  onRecompute: () => void
  /** Current recompute job status. Null when no job in-flight. */
  recomputeStatus: string | null
  /** Job error message when recompute failed; null otherwise. */
  recomputeError: string | null
  /** Active context type — used to render past-iteration results. */
  activeContextType: string
  /** Goal direction — used to highlight winner in past-iteration results. */
  goalDirection: GoalDirection
  /** Experiment ID — passes through to the Results subview for storage scope. */
  experimentId: string
  /** Default Frequentist/Bayesian view for past-iteration drill-down. */
  defaultModel: ResultsView
  /** Display-name list passed through to <Results>. */
  metricNames: string[]
}

export function IterationsTab({
  iterations,
  loading,
  error,
  selectedIteration,
  selectedResults,
  selectedLoading,
  onIterationSelect,
  onRecompute,
  recomputeStatus,
  recomputeError,
  activeContextType,
  goalDirection,
  experimentId,
  defaultModel,
  metricNames,
}: Props) {
  const sorted = sortIterationsDesc(iterations)
  const isJobActive =
    recomputeStatus != null && shouldKeepPolling(recomputeStatus)

  return (
    <div>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 14,
          padding: '10px 14px',
          background: 'var(--bg-sunken)',
          border: '1px solid var(--border)',
          borderRadius: 8,
        }}
      >
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
          Iteration history · click a row to view past-iteration results
          {recomputeStatus != null && (
            <span style={{ marginLeft: 12 }}>
              · Recompute:{' '}
              <strong
                style={{
                  color:
                    recomputeStatus === 'failed'
                      ? 'var(--danger)'
                      : recomputeStatus === 'succeeded'
                        ? 'var(--success)'
                        : 'var(--fg)',
                }}
              >
                {recomputeStatusLabel(recomputeStatus)}
              </strong>
            </span>
          )}
          {recomputeError && (
            <span style={{ marginLeft: 8, color: 'var(--danger)' }}>
              ({recomputeError})
            </span>
          )}
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <button
            type="button"
            data-testid="recompute-button"
            className="btn primary"
            disabled={isJobActive}
            onClick={onRecompute}
            style={{ opacity: isJobActive ? 0.6 : 1 }}
          >
            {isJobActive ? 'Recomputing…' : 'Recompute now'}
          </button>
        </div>
      </div>

      {error != null && (
        <div
          style={{
            padding: '12px 16px',
            background: 'var(--danger-bg)',
            color: 'var(--danger)',
            fontSize: 13,
            borderRadius: 8,
            marginBottom: 14,
          }}
        >
          {error}
        </div>
      )}

      <div className="card">
        <div className="card-header">
          <div className="card-title">Past iterations</div>
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
            {iterations.length === 0
              ? '—'
              : `${iterations.length} iteration${iterations.length === 1 ? '' : 's'}`}
          </span>
        </div>
        {loading ? (
          <div
            style={{
              padding: 24,
              textAlign: 'center',
              color: 'var(--fg-muted)',
              fontSize: 13,
            }}
          >
            Loading iterations…
          </div>
        ) : sorted.length === 0 ? (
          <div
            style={{
              padding: 24,
              textAlign: 'center',
              color: 'var(--fg-muted)',
              fontSize: 13,
            }}
          >
            No iterations yet for this experiment.
          </div>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>#</th>
                <th>Started</th>
                <th>Ended</th>
                <th>Duration</th>
                <th>Unit context types</th>
                <th>Computed at</th>
              </tr>
            </thead>
            <tbody>
              {sorted.map((iter) => {
                const isSelected =
                  selectedIteration?.iteration_id === iter.iteration_id
                return (
                  <tr
                    key={iter.iteration_id}
                    style={{
                      cursor: 'pointer',
                      background: isSelected
                        ? 'var(--accent-softer, var(--bg-sunken))'
                        : undefined,
                    }}
                    onClick={() =>
                      onIterationSelect(isSelected ? null : iter)
                    }
                  >
                    <td style={{ fontFamily: 'var(--font-mono)' }}>
                      #{iter.iteration_number}
                      <div
                        className="mono-key"
                        style={{ fontSize: 10, color: 'var(--fg-muted)' }}
                      >
                        {iter.iteration_id}
                      </div>
                    </td>
                    <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                      {fmtTimestamp(iter.started_at)}
                    </td>
                    <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                      {fmtTimestamp(iter.ended_at)}
                    </td>
                    <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                      {formatDuration(iter.started_at, iter.ended_at)}
                    </td>
                    <td>
                      <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
                        {iter.unit_context_types.map((ct) => (
                          <span key={ct} className="badge">
                            {ct}
                          </span>
                        ))}
                      </div>
                    </td>
                    <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                      {fmtTimestamp(iter.computed_at)}
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        )}
      </div>

      {selectedIteration != null && (
        <div
          data-testid="past-iteration-results"
          style={{ marginTop: 18 }}
        >
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              marginBottom: 10,
              padding: '10px 14px',
              background: 'var(--bg-sunken)',
              border: '1px solid var(--border)',
              borderRadius: 8,
              fontSize: 12,
              color: 'var(--fg-muted)',
            }}
          >
            <span>
              Iteration{' '}
              <strong style={{ color: 'var(--fg)' }}>
                #{selectedIteration.iteration_number}
              </strong>{' '}
              · Read-only snapshot
            </span>
            <span style={{ marginLeft: 'auto' }}>
              {fmtTimestamp(selectedIteration.started_at)}
              {selectedIteration.ended_at != null && (
                <> → {fmtTimestamp(selectedIteration.ended_at)}</>
              )}
            </span>
          </div>
          {selectedLoading ? (
            <div
              className="card"
              style={{
                padding: 32,
                textAlign: 'center',
                color: 'var(--fg-muted)',
                fontSize: 13,
              }}
            >
              Loading iteration results…
            </div>
          ) : (
            <Results
              experimentId={`${experimentId}-iter-${selectedIteration.iteration_number}`}
              activeContextType={activeContextType}
              results={selectedResults}
              defaultModel={defaultModel}
              goalDirection={goalDirection}
              metricNames={metricNames}
            />
          )}
        </div>
      )}
    </div>
  )
}
