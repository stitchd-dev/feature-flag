/**
 * Iterations tab — Phase 9 Task 9.4 tests.
 *
 * Coverage:
 *   • sortIterationsDesc(): newest iteration_number first.
 *   • formatDuration(): "2h 14m" style label from ISO timestamps.
 *   • recomputeStatusLabel(): maps queued/running/succeeded/failed → label.
 *   • shouldKeepPolling(): polling stops on terminal status.
 *   • Component: renders list rows sorted desc, recompute button,
 *     past-iteration drilldown when a row is selected.
 *   • Recompute button is disabled while a job is in-flight.
 *   • Toast message rendered on job completion.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  IterationsTab,
  sortIterationsDesc,
  formatDuration,
  recomputeStatusLabel,
  shouldKeepPolling,
} from './Iterations'
import type { IterationSummary } from './Iterations'
import type { ExperimentResults } from '../../../lib/types'

// ── Fixtures ─────────────────────────────────────────────────────────────────

const ITER_1: IterationSummary = {
  iteration_id: 'iter-1',
  iteration_number: 1,
  started_at: '2026-04-10T08:00:00Z',
  ended_at: '2026-04-24T08:00:00Z',
  unit_context_types: ['user'],
  computed_at: '2026-04-24T08:14:00Z',
}

const ITER_2: IterationSummary = {
  iteration_id: 'iter-2',
  iteration_number: 2,
  started_at: '2026-04-25T08:00:00Z',
  ended_at: null,
  unit_context_types: ['user', 'account'],
  computed_at: '2026-05-19T08:14:00Z',
}

const RESULTS: ExperimentResults = {
  results_by_context_type: {
    user: {
      variants: [
        {
          variant_key: 'control',
          assigned_count: 100,
          mean: 0.10,
          sample_size: 100,
          p_value: null,
          ci_lower: 0.08,
          ci_upper: 0.12,
          prob_to_beat_control: null,
          expected_lift: null,
          is_winner: false,
        },
      ],
      srm: { per_variant: [], overall_chi_sq_p: 0.5, health: 'green' },
      guardrails: [],
    },
  },
  bound_target: { kind: 'rule', rule_id: 'r-1', label: 'rule' },
  pre_period_days: 0,
}

// ── sortIterationsDesc ───────────────────────────────────────────────────────

describe('sortIterationsDesc', () => {
  it('sorts by iteration_number descending', () => {
    const sorted = sortIterationsDesc([ITER_1, ITER_2])
    expect(sorted.map((i) => i.iteration_number)).toEqual([2, 1])
  })

  it('handles empty arrays', () => {
    expect(sortIterationsDesc([])).toEqual([])
  })

  it('does not mutate the input', () => {
    const input = [ITER_1, ITER_2]
    sortIterationsDesc(input)
    expect(input).toEqual([ITER_1, ITER_2])
  })
})

// ── formatDuration ───────────────────────────────────────────────────────────

describe('formatDuration', () => {
  it('returns "—" when started_at is null', () => {
    expect(formatDuration(null, null)).toBe('—')
  })

  it('returns "ongoing" when ended_at is null and started_at is set', () => {
    expect(formatDuration('2026-04-10T08:00:00Z', null)).toMatch(/ongoing/i)
  })

  it('formats hour + minute', () => {
    expect(
      formatDuration('2026-04-10T08:00:00Z', '2026-04-10T10:14:00Z'),
    ).toBe('2h 14m')
  })

  it('formats day + hour for >24h windows', () => {
    expect(
      formatDuration('2026-04-10T08:00:00Z', '2026-04-12T10:00:00Z'),
    ).toBe('2d 2h')
  })

  it('handles sub-hour windows', () => {
    expect(
      formatDuration('2026-04-10T08:00:00Z', '2026-04-10T08:42:00Z'),
    ).toBe('42m')
  })
})

// ── recomputeStatusLabel + shouldKeepPolling ─────────────────────────────────

describe('recomputeStatusLabel', () => {
  it('maps known statuses to label', () => {
    expect(recomputeStatusLabel('queued')).toMatch(/queued/i)
    expect(recomputeStatusLabel('running')).toMatch(/running/i)
    expect(recomputeStatusLabel('succeeded')).toMatch(/succeed|complete/i)
    expect(recomputeStatusLabel('failed')).toMatch(/fail/i)
  })

  it('echoes unknown statuses verbatim', () => {
    expect(recomputeStatusLabel('weird-state')).toBe('weird-state')
  })
})

describe('shouldKeepPolling', () => {
  it('returns true for queued + running', () => {
    expect(shouldKeepPolling('queued')).toBe(true)
    expect(shouldKeepPolling('running')).toBe(true)
  })

  it('returns false for terminal states', () => {
    expect(shouldKeepPolling('succeeded')).toBe(false)
    expect(shouldKeepPolling('failed')).toBe(false)
  })

  it('returns false for unknown states (safest)', () => {
    expect(shouldKeepPolling('weird')).toBe(false)
  })
})

// ── IterationsTab (component) ────────────────────────────────────────────────

describe('IterationsTab', () => {
  it('renders rows sorted descending by iteration_number', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[ITER_1, ITER_2]}
        loading={false}
        error={null}
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus={null}
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    // iter-2 (#2) should appear before iter-1 (#1) in the HTML.
    const idx2 = html.indexOf('iter-2')
    const idx1 = html.indexOf('iter-1')
    expect(idx2).toBeGreaterThan(-1)
    expect(idx1).toBeGreaterThan(idx2)
  })

  it('renders the recompute button', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[]}
        loading={false}
        error={null}
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus={null}
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/data-testid="recompute-button"/)
  })

  it('disables recompute button while job is running', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[]}
        loading={false}
        error={null}
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus="running"
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/data-testid="recompute-button"[^>]*disabled/)
  })

  it('shows past-iteration results when a row is selected', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[ITER_1]}
        loading={false}
        error={null}
        selectedIteration={ITER_1}
        selectedResults={RESULTS}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus={null}
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/data-testid="past-iteration-results"/)
    // Read-only marker mentioned in the past-iteration card.
    expect(html).toMatch(/Read-only|read-only/)
  })

  it('shows unit_context_types in the iteration row', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[ITER_2]}
        loading={false}
        error={null}
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus={null}
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    // unit_context_types: ['user', 'account'] → both keys must render.
    expect(html).toMatch(/user/)
    expect(html).toMatch(/account/)
  })

  it('renders error banner when error is set', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[]}
        loading={false}
        error="something exploded"
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus={null}
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/something exploded/)
  })

  it('renders empty state when no iterations exist', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[]}
        loading={false}
        error={null}
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus={null}
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/No iterations|no iterations/i)
  })

  it('renders the recompute status when set', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[]}
        loading={false}
        error={null}
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus="succeeded"
        recomputeError={null}
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/succeeded|complete/i)
  })

  it('renders the recompute error message when failure', () => {
    const html = renderToString(
      <IterationsTab
        iterations={[]}
        loading={false}
        error={null}
        selectedIteration={null}
        selectedResults={null}
        selectedLoading={false}
        onIterationSelect={() => {}}
        onRecompute={() => {}}
        recomputeStatus="failed"
        recomputeError="stats-service connection refused"
        activeContextType="user"
        goalDirection="increase"
        experimentId="exp-1"
        defaultModel="frequentist"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/stats-service connection refused/)
  })
})
