/**
 * MetricPreview — pure helper tests.
 *
 * The MetricPreview component (Phase 6 Task 6.6) calls
 * `POST /v1/metrics/{id}/preview` and renders a sparkline + numeric summary.
 *
 * Following the codebase pattern set by `EventDetail.test.ts`, the pure
 * helpers are MIRRORED here (rather than imported) so the component file
 * stays clean of non-component exports (`react-refresh/only-export-components`).
 * Each mirror MUST stay in lock-step with the implementation in
 * `MetricPreview.tsx` — any change to the helpers there should be reflected
 * here, and vice versa.
 *
 * @see feature-flag-7an.6.6 — this task
 * @see feature-flag-7an.4.2 — Phase 4 backend implementation (preview returns
 *      empty buckets + warning until that lands)
 */
import { describe, it, expect } from 'vitest'

// ── Types mirrored from MetricPreview.tsx ─────────────────────────────────────

export interface PreviewBucket {
  day: string
  value: number
}

export interface BucketSummary {
  count: number
  last: number
  min: number
  max: number
  mean: number
}

// ── Pure helpers mirrored from MetricPreview.tsx ──────────────────────────────

function safeValue(v: unknown): number {
  const n = typeof v === 'number' ? v : Number.NaN
  return Number.isFinite(n) ? n : 0
}

export function summariseBuckets(buckets: PreviewBucket[]): BucketSummary {
  if (!buckets || buckets.length === 0) {
    return { count: 0, last: 0, min: 0, max: 0, mean: 0 }
  }
  const values = buckets.map((b) => safeValue(b.value))
  const sum = values.reduce((a, b) => a + b, 0)
  return {
    count: values.length,
    last: values[values.length - 1],
    min: Math.min(...values),
    max: Math.max(...values),
    mean: sum / values.length,
  }
}

export function previewUrl(metricId: string): string {
  return `/v1/metrics/${encodeURIComponent(metricId)}/preview`
}

export function previewBodyShape(days: number): { days: number } {
  return { days: days > 0 ? days : 7 }
}

// ── summariseBuckets ──────────────────────────────────────────────────────────

describe('summariseBuckets', () => {
  /** test_summariseBuckets_empty_returns_zeros_with_count_0 */
  it('returns zeros and count 0 for an empty buckets array', () => {
    const out = summariseBuckets([])
    expect(out.count).toBe(0)
    expect(out.last).toBe(0)
    expect(out.min).toBe(0)
    expect(out.max).toBe(0)
    expect(out.mean).toBe(0)
  })

  /** test_summariseBuckets_single_value */
  it('mirrors the single value to every summary field', () => {
    const buckets: PreviewBucket[] = [{ day: '2026-05-19', value: 0.5 }]
    const out = summariseBuckets(buckets)
    expect(out.count).toBe(1)
    expect(out.last).toBe(0.5)
    expect(out.min).toBe(0.5)
    expect(out.max).toBe(0.5)
    expect(out.mean).toBe(0.5)
  })

  /** test_summariseBuckets_multiple_values_mean_min_max */
  it('computes mean/min/max/last across multiple buckets', () => {
    const buckets: PreviewBucket[] = [
      { day: '2026-05-15', value: 10 },
      { day: '2026-05-16', value: 20 },
      { day: '2026-05-17', value: 30 },
      { day: '2026-05-18', value: 40 },
    ]
    const out = summariseBuckets(buckets)
    expect(out.count).toBe(4)
    expect(out.last).toBe(40) // chronological — last in array
    expect(out.min).toBe(10)
    expect(out.max).toBe(40)
    expect(out.mean).toBe(25) // (10+20+30+40)/4
  })

  /** test_summariseBuckets_negative_values */
  it('handles negative values correctly (e.g. metric with goal_direction=decrease)', () => {
    const buckets: PreviewBucket[] = [
      { day: '2026-05-15', value: -5 },
      { day: '2026-05-16', value: -10 },
      { day: '2026-05-17', value: 0 },
    ]
    const out = summariseBuckets(buckets)
    expect(out.count).toBe(3)
    expect(out.last).toBe(0)
    expect(out.min).toBe(-10)
    expect(out.max).toBe(0)
    expect(out.mean).toBeCloseTo(-5, 6) // (-5 + -10 + 0) / 3
  })

  /** test_summariseBuckets_handles_NaN_gracefully */
  it('treats NaN values as 0 and never leaks NaN through the summary', () => {
    const buckets: PreviewBucket[] = [
      { day: '2026-05-15', value: 10 },
      { day: '2026-05-16', value: Number.NaN },
      { day: '2026-05-17', value: 20 },
    ]
    const out = summariseBuckets(buckets)
    expect(Number.isNaN(out.last)).toBe(false)
    expect(Number.isNaN(out.min)).toBe(false)
    expect(Number.isNaN(out.max)).toBe(false)
    expect(Number.isNaN(out.mean)).toBe(false)
    // NaN coerced to 0 → values are {10, 0, 20}
    expect(out.count).toBe(3)
    expect(out.min).toBe(0)
    expect(out.max).toBe(20)
    expect(out.last).toBe(20)
    expect(out.mean).toBe(10) // (10+0+20)/3
  })

  it('treats undefined / null values defensively as 0', () => {
    const buckets = [
      { day: '2026-05-15', value: 5 },
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      { day: '2026-05-16', value: undefined as any },
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      { day: '2026-05-17', value: null as any },
    ] as PreviewBucket[]
    const out = summariseBuckets(buckets)
    expect(Number.isNaN(out.mean)).toBe(false)
    expect(out.count).toBe(3)
    expect(out.last).toBe(0)
  })

  it('returns last as the most recent (last) bucket value, not the largest', () => {
    const buckets: PreviewBucket[] = [
      { day: '2026-05-15', value: 100 },
      { day: '2026-05-16', value: 50 },
      { day: '2026-05-17', value: 25 },
    ]
    expect(summariseBuckets(buckets).last).toBe(25)
  })
})

// ── previewUrl ────────────────────────────────────────────────────────────────

describe('previewUrl', () => {
  /** test_previewUrl_encodes_metric_id */
  it('produces the canonical preview path for a metric id', () => {
    expect(previewUrl('abc123')).toBe('/v1/metrics/abc123/preview')
  })

  it('encodes special characters in the metric id defensively', () => {
    expect(previewUrl('a b/c?d')).toBe('/v1/metrics/a%20b%2Fc%3Fd/preview')
  })

  it('handles a typical UUID without mangling it', () => {
    const id = '550e8400-e29b-41d4-a716-446655440000'
    expect(previewUrl(id)).toBe(`/v1/metrics/${id}/preview`)
  })

  it('does not embed `days` in the path (days lives in the request body)', () => {
    // Path is days-independent — re-call to confirm idempotence.
    expect(previewUrl('m1')).toBe('/v1/metrics/m1/preview')
    expect(previewUrl('m1')).not.toContain('30')
    expect(previewUrl('m1')).not.toContain('days')
  })
})

// ── previewBodyShape ──────────────────────────────────────────────────────────

describe('previewBodyShape', () => {
  /** test_previewBodyShape_default_days_7 */
  it('returns { days: 7 } by default', () => {
    expect(previewBodyShape(7)).toEqual({ days: 7 })
  })

  it('echoes the days parameter into the body shape', () => {
    expect(previewBodyShape(14)).toEqual({ days: 14 })
    expect(previewBodyShape(30)).toEqual({ days: 30 })
  })

  it('clamps non-positive days to 7 (server-side default)', () => {
    expect(previewBodyShape(0)).toEqual({ days: 7 })
    expect(previewBodyShape(-1)).toEqual({ days: 7 })
  })
})
