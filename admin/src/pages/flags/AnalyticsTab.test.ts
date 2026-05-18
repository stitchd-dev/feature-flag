import { describe, it, expect } from 'vitest'

// ── Types mirrored from AnalyticsTab.tsx ──────────────────────────────────────

interface EvalStatsBucket {
  ts: string
  total: number
  by_variant: Record<string, number>
  disabled_count: number
  unique_context_keys: number
}

interface EvalStatsResponse {
  granularity: string
  buckets: EvalStatsBucket[]
}

// ── Logic mirrored from AnalyticsTab.tsx ──────────────────────────────────────

const RANGES = [
  { key: '1h',  label: 'Last 1h',  hours: 1 },
  { key: '6h',  label: 'Last 6h',  hours: 6 },
  { key: '24h', label: 'Last 24h', hours: 24 },
  { key: '7d',  label: 'Last 7d',  hours: 168 },
  { key: '30d', label: 'Last 30d', hours: 720 },
]

function resolveGranularity(hours: number): string {
  return hours > 24 ? 'day' : 'hour'
}

function buildChartData(stats: EvalStatsResponse, variantKeys: string[]): Record<string, number | string>[] {
  return stats.buckets.map((b) => {
    const row: Record<string, number | string> = {
      ts: b.ts,
      disabled: b.disabled_count,
      unique_ctx: b.unique_context_keys,
    }
    for (const vk of variantKeys) {
      row[vk] = b.by_variant[vk] ?? 0
    }
    return row
  })
}

function deriveVariantKeys(stats: EvalStatsResponse): string[] {
  return Array.from(new Set(stats.buckets.flatMap((b) => Object.keys(b.by_variant))))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('resolveGranularity', () => {
  it('returns hour for ranges up to 24h', () => {
    expect(resolveGranularity(1)).toBe('hour')
    expect(resolveGranularity(6)).toBe('hour')
    expect(resolveGranularity(24)).toBe('hour')
  })

  it('switches to day when range exceeds 24h', () => {
    expect(resolveGranularity(25)).toBe('day')
    expect(resolveGranularity(168)).toBe('day')
    expect(resolveGranularity(720)).toBe('day')
  })

  it('exactly 24h stays as hour (not strictly greater)', () => {
    expect(resolveGranularity(24)).toBe('hour')
  })
})

describe('RANGES', () => {
  it('covers all expected range keys', () => {
    const keys = RANGES.map((r) => r.key)
    expect(keys).toContain('1h')
    expect(keys).toContain('6h')
    expect(keys).toContain('24h')
    expect(keys).toContain('7d')
    expect(keys).toContain('30d')
  })

  it('last 30d resolves to day granularity', () => {
    const r30d = RANGES.find((r) => r.key === '30d')!
    expect(resolveGranularity(r30d.hours)).toBe('day')
  })

  it('last 24h resolves to hour granularity', () => {
    const r24h = RANGES.find((r) => r.key === '24h')!
    expect(resolveGranularity(r24h.hours)).toBe('hour')
  })
})

describe('deriveVariantKeys', () => {
  it('returns empty array when there are no buckets', () => {
    const stats: EvalStatsResponse = { granularity: 'hour', buckets: [] }
    expect(deriveVariantKeys(stats)).toEqual([])
  })

  it('collects all distinct variant keys across buckets', () => {
    const stats: EvalStatsResponse = {
      granularity: 'hour',
      buckets: [
        { ts: 't1', total: 5, by_variant: { on: 3, off: 2 }, disabled_count: 0, unique_context_keys: 2 },
        { ts: 't2', total: 4, by_variant: { on: 4 }, disabled_count: 0, unique_context_keys: 3 },
      ],
    }
    const keys = deriveVariantKeys(stats)
    expect(keys).toContain('on')
    expect(keys).toContain('off')
    expect(keys).toHaveLength(2)
  })

  it('deduplicates variant keys', () => {
    const stats: EvalStatsResponse = {
      granularity: 'hour',
      buckets: [
        { ts: 't1', total: 2, by_variant: { on: 2 }, disabled_count: 0, unique_context_keys: 1 },
        { ts: 't2', total: 3, by_variant: { on: 3 }, disabled_count: 0, unique_context_keys: 2 },
      ],
    }
    expect(deriveVariantKeys(stats)).toHaveLength(1)
  })
})

describe('buildChartData', () => {
  it('returns empty array for empty buckets (empty state)', () => {
    const stats: EvalStatsResponse = { granularity: 'hour', buckets: [] }
    expect(buildChartData(stats, [])).toHaveLength(0)
  })

  it('maps bucket fields to chart rows', () => {
    const stats: EvalStatsResponse = {
      granularity: 'hour',
      buckets: [
        {
          ts: '2026-05-15T10:00:00Z',
          total: 10,
          by_variant: { on: 7, off: 3 },
          disabled_count: 1,
          unique_context_keys: 5,
        },
      ],
    }
    const rows = buildChartData(stats, ['on', 'off'])
    expect(rows).toHaveLength(1)
    expect(rows[0].on).toBe(7)
    expect(rows[0].off).toBe(3)
    expect(rows[0].disabled).toBe(1)
    expect(rows[0].unique_ctx).toBe(5)
  })

  it('defaults missing variant keys to 0', () => {
    const stats: EvalStatsResponse = {
      granularity: 'hour',
      buckets: [
        { ts: 't1', total: 3, by_variant: { on: 3 }, disabled_count: 0, unique_context_keys: 1 },
      ],
    }
    const rows = buildChartData(stats, ['on', 'off'])
    expect(rows[0].off).toBe(0)
  })

  it('renders series keys in output rows', () => {
    const stats: EvalStatsResponse = {
      granularity: 'day',
      buckets: [
        { ts: 't1', total: 5, by_variant: { beta: 5 }, disabled_count: 2, unique_context_keys: 3 },
        { ts: 't2', total: 8, by_variant: { beta: 8 }, disabled_count: 0, unique_context_keys: 5 },
      ],
    }
    const keys = deriveVariantKeys(stats)
    const rows = buildChartData(stats, keys)
    expect(rows.every((r) => 'beta' in r)).toBe(true)
    expect(rows.every((r) => 'disabled' in r)).toBe(true)
    expect(rows.every((r) => 'unique_ctx' in r)).toBe(true)
  })
})
