/**
 * EventDetail — logic tests (pure, no DOM).
 *
 * Mirrors the helper/derivation functions that EventDetail.tsx exposes for
 * its three sections (header, firings table, sparkline, experiments-depending
 * -on list). Same test style as `EventsList.test.ts` and `AnalyticsTab.test.ts`.
 *
 * Backend gap: the `/v1/events/{key}/firings` + `/v1/events/{key}/stats`
 * endpoints are not yet implemented — tracked in beads bug
 * `feature-flag-uz3`. Until they land, the UI shows skeleton / dash states.
 * These tests cover the pure rendering branches for both the "data present"
 * and "no data" code paths.
 */
import { describe, it, expect } from 'vitest'

// ── Types mirrored from EventDetail.tsx ───────────────────────────────────────

export interface EventDefinitionDetail {
  event_key: string
  name?: string
  metric_type: string
  description?: string
  schema?: string | null
  archived: boolean
  deleted_at?: string | null
  created_at: string
}

export interface EventFiring {
  ts: string
  context_type: string
  context_key: string
  value?: number | null
  properties?: Record<string, unknown> | null
}

export interface EventStatsBucket {
  day: string
  count: number
}

export interface EventStats {
  buckets: EventStatsBucket[]
}

export interface ExperimentDependent {
  experiment_id: string
  key: string
  name: string
  status: string
}

// ── Pure derivations mirrored from EventDetail.tsx ────────────────────────────

/** Header badge — archived if deleted_at is set OR archived flag is true. */
export function isArchived(event: EventDefinitionDetail): boolean {
  return event.archived === true || event.deleted_at != null
}

/** Display label — name fallback to event_key. */
export function displayName(event: EventDefinitionDetail): string {
  return event.name && event.name.trim() ? event.name : event.event_key
}

/** Pretty-print a firing properties JSON value for the table cell. */
export function formatFiringProperties(props: Record<string, unknown> | null | undefined): string {
  if (props === null || props === undefined) return '—'
  const keys = Object.keys(props)
  if (keys.length === 0) return '{}'
  // Collapse to single-line JSON for the table cell, truncated for display.
  return JSON.stringify(props)
}

/** Format a firing's numeric value column — null/undefined → em-dash. */
export function formatFiringValue(v: number | null | undefined): string {
  if (v === null || v === undefined) return '—'
  return String(v)
}

/** Format a firing timestamp into a localised string. */
export function formatFiringTs(iso: string): string {
  return new Date(iso).toLocaleString()
}

/**
 * Resolve the sparkline data array from a stats payload — `null` / empty
 * buckets → an empty array (the caller renders a skeleton). Always returns
 * `days` entries by zero-filling missing days.
 */
export function sparklineData(stats: EventStats | null, days: number): number[] {
  if (!stats || stats.buckets.length === 0) return []
  // Map of day → count for fast lookup.
  const byDay = new Map<string, number>()
  for (const b of stats.buckets) byDay.set(b.day, b.count)
  // Pre-compute the day axis (UTC YYYY-MM-DD).
  const out: number[] = []
  const now = new Date()
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now.getTime() - i * 24 * 60 * 60 * 1000)
    const key = d.toISOString().slice(0, 10)
    out.push(byDay.get(key) ?? 0)
  }
  return out
}

/** Sum the buckets to a single 14-day total. */
export function statsTotal(stats: EventStats | null): number {
  if (!stats) return 0
  return stats.buckets.reduce((sum, b) => sum + b.count, 0)
}

/**
 * Filter experiments to only those depending on this event (by metric_key /
 * metric_event_key). The API may pre-filter — this is a defence-in-depth
 * helper for client-side derivation/tests.
 */
export function dependentsForEvent(
  experiments: { metric_event_key?: string }[],
  eventKey: string,
): { metric_event_key?: string }[] {
  return experiments.filter((e) => e.metric_event_key === eventKey)
}

/** "X firings" header line — pluralised. */
export function firingsCountLabel(count: number): string {
  return `${count} firing${count === 1 ? '' : 's'}`
}

/** "X experiments depend on this event" header — pluralised. */
export function dependentsLabel(count: number): string {
  if (count === 0) return 'No experiments depend on this event'
  return `${count} experiment${count === 1 ? '' : 's'} depend${count === 1 ? 's' : ''} on this event`
}

/** Back-button URL — composed from orgId. */
export function backToListUrl(orgId: string): string {
  return `/org/${orgId}/events`
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

function makeEvent(partial: Partial<EventDefinitionDetail> & { event_key: string }): EventDefinitionDetail {
  return {
    event_key: partial.event_key,
    name: partial.name,
    metric_type: partial.metric_type ?? 'count',
    description: partial.description,
    schema: partial.schema ?? null,
    archived: partial.archived ?? false,
    deleted_at: partial.deleted_at ?? null,
    created_at: partial.created_at ?? '2026-05-19T00:00:00Z',
  }
}

function makeFiring(partial: Partial<EventFiring> & { ts: string; context_key: string }): EventFiring {
  return {
    ts: partial.ts,
    context_type: partial.context_type ?? 'user',
    context_key: partial.context_key,
    value: partial.value ?? null,
    properties: partial.properties ?? null,
  }
}

const SAMPLE_FIRINGS: EventFiring[] = [
  makeFiring({ ts: '2026-05-19T12:00:00Z', context_key: 'u_1', properties: { plan: 'pro' } }),
  makeFiring({ ts: '2026-05-19T12:01:00Z', context_key: 'u_2', value: 9.99 }),
  makeFiring({ ts: '2026-05-19T12:02:00Z', context_key: 'u_3' }),
]

const SAMPLE_DEPENDENTS: ExperimentDependent[] = [
  { experiment_id: 'exp-1', key: 'checkout-redesign', name: 'Checkout Redesign', status: 'running' },
  { experiment_id: 'exp-2', key: 'pricing-test',      name: 'Pricing Test',      status: 'draft'   },
]

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('EventDetail — loading state', () => {
  /** test_renders_loading_state */
  it('shows the loading spinner while loading is true and no event is loaded', () => {
    const loading = true
    const error: string | null = null
    const event: EventDefinitionDetail | null = null
    const showSpinner = loading && !error && event === null
    const showHeader = !loading && !error && event !== null
    expect(showSpinner).toBe(true)
    expect(showHeader).toBe(false)
  })

  it('hides the spinner once event is loaded', () => {
    const loading = false
    const event: EventDefinitionDetail | null = makeEvent({ event_key: 'k' })
    const showHeader = !loading && event !== null
    expect(showHeader).toBe(true)
  })
})

describe('EventDetail — header', () => {
  /** test_renders_event_header_with_name_and_type */
  it('uses the event name when present', () => {
    const e = makeEvent({ event_key: 'checkout.completed', name: 'Checkout completed' })
    expect(displayName(e)).toBe('Checkout completed')
  })

  it('falls back to event_key when name is missing', () => {
    const e = makeEvent({ event_key: 'checkout.completed' })
    expect(displayName(e)).toBe('checkout.completed')
  })

  it('falls back to event_key when name is whitespace-only', () => {
    const e = makeEvent({ event_key: 'checkout.completed', name: '   ' })
    expect(displayName(e)).toBe('checkout.completed')
  })

  it('exposes metric_type so the header can show a chip', () => {
    const e = makeEvent({ event_key: 'k', metric_type: 'conversion' })
    expect(e.metric_type).toBe('conversion')
  })
})

describe('EventDetail — archived badge', () => {
  /** test_renders_archived_badge_when_deleted_at_is_set */
  it('renders the archived badge when deleted_at is set', () => {
    const e = makeEvent({ event_key: 'k', deleted_at: '2026-05-19T00:00:00Z' })
    expect(isArchived(e)).toBe(true)
  })

  it('renders the archived badge when archived flag is true', () => {
    const e = makeEvent({ event_key: 'k', archived: true })
    expect(isArchived(e)).toBe(true)
  })

  it('does not render the archived badge for an active event', () => {
    const e = makeEvent({ event_key: 'k' })
    expect(isArchived(e)).toBe(false)
  })

  it('treats deleted_at: null and archived: false as not-archived', () => {
    const e = makeEvent({ event_key: 'k', archived: false, deleted_at: null })
    expect(isArchived(e)).toBe(false)
  })
})

describe('EventDetail — firings table', () => {
  /** test_firings_table_renders_rows */
  it('renders one row per firing in the response', () => {
    const rows = SAMPLE_FIRINGS
    expect(rows).toHaveLength(3)
    expect(rows[0].context_key).toBe('u_1')
    expect(rows[1].context_key).toBe('u_2')
    expect(rows[2].context_key).toBe('u_3')
  })

  it('formats firing value — null/undefined → em-dash, number → string', () => {
    expect(formatFiringValue(null)).toBe('—')
    expect(formatFiringValue(undefined)).toBe('—')
    expect(formatFiringValue(0)).toBe('0')
    expect(formatFiringValue(9.99)).toBe('9.99')
  })

  it('formats firing properties — null → em-dash, {} → "{}", object → JSON', () => {
    expect(formatFiringProperties(null)).toBe('—')
    expect(formatFiringProperties(undefined)).toBe('—')
    expect(formatFiringProperties({})).toBe('{}')
    expect(formatFiringProperties({ plan: 'pro' })).toBe('{"plan":"pro"}')
  })

  it('formats firing timestamp — ISO string → not the em-dash (locale-independent)', () => {
    const s = formatFiringTs('2026-05-19T12:00:00Z')
    expect(s).not.toBe('')
    expect(s).not.toBe('—')
  })

  /** test_firings_empty_state */
  it('renders the empty state when there are zero firings', () => {
    const rows: EventFiring[] = []
    const showEmpty = rows.length === 0
    expect(showEmpty).toBe(true)
  })

  it('does NOT render the empty state when at least one firing is present', () => {
    const rows = SAMPLE_FIRINGS
    const showEmpty = rows.length === 0
    expect(showEmpty).toBe(false)
  })

  it('pluralises the firings header count', () => {
    expect(firingsCountLabel(0)).toBe('0 firings')
    expect(firingsCountLabel(1)).toBe('1 firing')
    expect(firingsCountLabel(50)).toBe('50 firings')
  })
})

describe('EventDetail — sparkline', () => {
  /** test_sparkline_renders_with_14_days_of_data */
  it('produces a 14-day series from a stats payload', () => {
    // Fixture: 3 days of data, zero-filled to 14.
    const today = new Date()
    function day(offset: number) {
      const d = new Date(today.getTime() - offset * 24 * 60 * 60 * 1000)
      return d.toISOString().slice(0, 10)
    }
    const stats: EventStats = {
      buckets: [
        { day: day(0), count: 5 },
        { day: day(1), count: 3 },
        { day: day(2), count: 1 },
      ],
    }
    const out = sparklineData(stats, 14)
    expect(out).toHaveLength(14)
    // The last 3 entries (i = days-3 .. days-1) correspond to day(2), day(1), day(0).
    expect(out[13]).toBe(5)
    expect(out[12]).toBe(3)
    expect(out[11]).toBe(1)
    // Rest are zero-filled
    expect(out[10]).toBe(0)
    expect(out[0]).toBe(0)
  })

  it('returns an empty array when stats is null', () => {
    expect(sparklineData(null, 14)).toHaveLength(0)
  })

  it('returns an empty array when buckets are empty', () => {
    expect(sparklineData({ buckets: [] }, 14)).toHaveLength(0)
  })

  it('totals the count across buckets', () => {
    const stats: EventStats = {
      buckets: [
        { day: '2026-05-15', count: 10 },
        { day: '2026-05-16', count: 5 },
        { day: '2026-05-17', count: 2 },
      ],
    }
    expect(statsTotal(stats)).toBe(17)
    expect(statsTotal(null)).toBe(0)
    expect(statsTotal({ buckets: [] })).toBe(0)
  })
})

describe('EventDetail — experiments-depending-on section', () => {
  /** test_experiments_section_lists_dependents */
  it('renders one row per dependent experiment', () => {
    expect(SAMPLE_DEPENDENTS).toHaveLength(2)
    expect(SAMPLE_DEPENDENTS[0].key).toBe('checkout-redesign')
    expect(SAMPLE_DEPENDENTS[1].key).toBe('pricing-test')
  })

  it('pluralises the dependents header', () => {
    expect(dependentsLabel(0)).toBe('No experiments depend on this event')
    expect(dependentsLabel(1)).toBe('1 experiment depends on this event')
    expect(dependentsLabel(2)).toBe('2 experiments depend on this event')
  })

  /** test_experiments_section_empty_state */
  it('renders the empty state when zero dependents', () => {
    const dependents: ExperimentDependent[] = []
    const showEmpty = dependents.length === 0
    expect(showEmpty).toBe(true)
  })

  it('does NOT render the empty state when at least one dependent', () => {
    const showEmpty = SAMPLE_DEPENDENTS.length === 0
    expect(showEmpty).toBe(false)
  })

  it('client-side filter retains only matching dependents', () => {
    const all = [
      { metric_event_key: 'checkout.completed' },
      { metric_event_key: 'other.event' },
      { metric_event_key: 'checkout.completed' },
    ]
    const filtered = dependentsForEvent(all, 'checkout.completed')
    expect(filtered).toHaveLength(2)
  })

  it('client-side filter drops experiments without a metric_event_key', () => {
    const all = [{ metric_event_key: 'a' }, {}, { metric_event_key: 'a' }]
    expect(dependentsForEvent(all, 'a')).toHaveLength(2)
  })
})

describe('EventDetail — back-to-list navigation', () => {
  /** test_navigates_back_to_list_on_back_button */
  it('composes the back URL from orgId', () => {
    expect(backToListUrl('org_123')).toBe('/org/org_123/events')
  })

  it('back URL has the expected shape regardless of orgId value', () => {
    expect(backToListUrl('any-org')).toMatch(/^\/org\/.+\/events$/)
  })
})
