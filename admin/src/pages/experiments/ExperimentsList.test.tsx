/**
 * Tests for the Phase 10 ExperimentsList enhancements — pure helpers plus
 * module-shape grep against the source file.
 *
 * Coverage:
 *   - Status filter matcher.
 *   - Flag filter matcher (key OR UUID).
 *   - Days-remaining computation + formatter (boundary cases).
 *   - Metric-name lookup + primary-metric label resolution.
 *   - Module wiring: list reads `api.experiments.listExposures` for the
 *     exposure-count badge and renders status + flag URL-driven filters.
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './ExperimentsList.tsx?raw'
import {
  buildMetricNameLookup,
  computeDaysRemaining,
  formatDaysRemaining,
  matchesFlag,
  matchesStatus,
  pickExposureContextType,
  resolvePrimaryMetricLabel,
  STATUS_FILTERS,
  statusBadgeClass,
} from './ExperimentsList.helpers'
import type { ExperimentResponse } from '../../lib/types'

const baseExp: ExperimentResponse = {
  experiment_id: 'exp-1',
  environment_id: 'env-1',
  key: 'exp-1',
  name: 'Exp 1',
  description: '',
  flag_key: 'flag-1',
  flag_id: 'flag-uuid-1',
  status: 'running',
  model: 'bayesian',
  metric_ids: ['metric-1'],
  variants: 2,
  started_at: '2026-01-01T00:00:00Z',
  ended_at: null,
  scheduled_end_at: null,
  unit_context_types: ['user'],
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
}

// ─── Filter matchers ─────────────────────────────────────────────────────────

describe('matchesStatus', () => {
  it('returns true for "all" regardless of status', () => {
    expect(matchesStatus(baseExp, 'all')).toBe(true)
  })

  it('matches the experiment status', () => {
    expect(matchesStatus(baseExp, 'running')).toBe(true)
    expect(matchesStatus({ ...baseExp, status: 'stopped' }, 'running')).toBe(false)
    expect(matchesStatus({ ...baseExp, status: 'stopped' }, 'stopped')).toBe(true)
  })

  it('exposes the canonical filter list (including paused)', () => {
    expect(STATUS_FILTERS).toContain('paused')
    expect(STATUS_FILTERS).toContain('all')
    expect(STATUS_FILTERS[0]).toBe('all')
  })
})

describe('matchesFlag', () => {
  it('returns true when filter is null', () => {
    expect(matchesFlag(baseExp, null)).toBe(true)
  })

  it('matches by flag_key', () => {
    expect(matchesFlag(baseExp, 'flag-1')).toBe(true)
    expect(matchesFlag(baseExp, 'flag-2')).toBe(false)
  })

  it('matches by flag_id', () => {
    expect(matchesFlag(baseExp, 'flag-uuid-1')).toBe(true)
    expect(matchesFlag(baseExp, 'other-uuid')).toBe(false)
  })
})

// ─── Days remaining ──────────────────────────────────────────────────────────

describe('computeDaysRemaining', () => {
  const now = new Date('2026-05-21T12:00:00Z')

  it('returns null when scheduled_end_at is missing', () => {
    expect(computeDaysRemaining(null, now)).toBeNull()
    expect(computeDaysRemaining(undefined, now)).toBeNull()
  })

  it('returns 0 for a date in the past', () => {
    expect(computeDaysRemaining('2026-05-20T00:00:00Z', now)).toBe(0)
  })

  it('returns positive whole days for the future', () => {
    expect(computeDaysRemaining('2026-05-24T12:00:00Z', now)).toBe(3)
  })

  it('rounds up partial days', () => {
    // ~6 hours from now → ceiling to 1 day
    expect(computeDaysRemaining('2026-05-21T18:00:00Z', now)).toBe(1)
  })

  it('returns null on an unparseable date string', () => {
    expect(computeDaysRemaining('not-a-date', now)).toBeNull()
  })
})

describe('formatDaysRemaining', () => {
  it('formats null as "—"', () => {
    expect(formatDaysRemaining(null)).toBe('—')
  })

  it('formats 0 as "Ended"', () => {
    expect(formatDaysRemaining(0)).toBe('Ended')
  })

  it('formats 1 / N days', () => {
    expect(formatDaysRemaining(1)).toBe('1 day')
    expect(formatDaysRemaining(7)).toBe('7 days')
  })
})

// ─── Badge / context ─────────────────────────────────────────────────────────

describe('statusBadgeClass', () => {
  it('maps known statuses to badge variants', () => {
    expect(statusBadgeClass('running')).toBe('info')
    expect(statusBadgeClass('completed')).toBe('success')
    expect(statusBadgeClass('stopped')).toBe('danger')
    expect(statusBadgeClass('paused')).toBe('warning')
    expect(statusBadgeClass('draft')).toBe('')
  })
})

describe('pickExposureContextType', () => {
  it('returns the first unit_context_type', () => {
    expect(
      pickExposureContextType({
        ...baseExp,
        unit_context_types: ['account', 'user'],
      }),
    ).toBe('account')
  })

  it('defaults to "user" when none declared', () => {
    expect(pickExposureContextType({ ...baseExp, unit_context_types: undefined })).toBe('user')
  })
})

// ─── Metric lookup ───────────────────────────────────────────────────────────

describe('buildMetricNameLookup + resolvePrimaryMetricLabel', () => {
  const lookup = buildMetricNameLookup([
    { id: 'metric-1', name: 'Checkout completed', key: 'checkout_completed' },
    { id: 'metric-2', name: '', key: 'fallback_to_key' },
  ])

  it('uses name when present', () => {
    expect(resolvePrimaryMetricLabel(baseExp, lookup)).toBe('Checkout completed')
  })

  it('falls back to key when name is empty', () => {
    expect(
      resolvePrimaryMetricLabel({ ...baseExp, metric_ids: ['metric-2'] }, lookup),
    ).toBe('fallback_to_key')
  })

  it('returns id when not in lookup yet', () => {
    expect(
      resolvePrimaryMetricLabel({ ...baseExp, metric_ids: ['unknown'] }, lookup),
    ).toBe('unknown')
  })

  it('returns "—" when the experiment has no metrics', () => {
    expect(resolvePrimaryMetricLabel({ ...baseExp, metric_ids: [] }, lookup)).toBe('—')
  })
})

// ─── Module shape ────────────────────────────────────────────────────────────

describe('ExperimentsList (module shape)', () => {
  it('reads the exposure count from listExperimentExposures', () => {
    expect(SOURCE).toMatch(/listExperimentExposures/)
  })

  it('renders status + flag URL-driven filters', () => {
    // useSearchParams drives the filter state.
    expect(SOURCE).toMatch(/useSearchParams|searchParams/)
  })

  it('imports the helpers module', () => {
    expect(SOURCE).toMatch(/ExperimentsList\.helpers/)
  })

  it('displays days remaining', () => {
    expect(SOURCE).toMatch(/computeDaysRemaining|formatDaysRemaining/)
  })

  it('resolves primary metric name via the lookup helper', () => {
    expect(SOURCE).toMatch(/resolvePrimaryMetricLabel|buildMetricNameLookup/)
  })
})
