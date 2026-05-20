/**
 * Unit tests for CreateExperimentModal helpers — pure logic only.
 *
 * Mirrors the pattern in MetricsList.test.ts: extract behaviour to pure
 * functions exported from the modal, then test them with Vitest.
 *
 * Covers:
 *   • Yup `experimentSchema` cutover from `primary_metric: string` →
 *     `metric_ids: string[]` (min 1, max 5, UUIDs).
 *   • Metric picker helpers (kind chip + search filter).
 *   • Request body shape sent to `POST /v1/environments/{id}/experiments`.
 */
import { describe, it, expect } from 'vitest'
import { ValidationError } from 'yup'
import { experimentSchema } from '../../lib/validation/experimentSchema'
import {
  buildExperimentCreateBody,
  filterMetricOptions,
  metricKindChipStyle,
  type MetricPickerOption,
} from './CreateExperimentModal.helpers'

// ─── Validation helper ───────────────────────────────────────────────────────

interface FormShape {
  name: string
  key: string
  flag_key: string
  description?: string
  model: 'bayesian' | 'frequentist'
  metric_ids: string[]
  duration_days: number
  variants: { key: string; allocation: number }[]
}

const validBase: FormShape = {
  name: 'Checkout button colour',
  key: 'checkout-btn-colour',
  flag_key: 'checkout-btn-colour',
  description: 'Test the button colour',
  model: 'bayesian',
  metric_ids: ['11111111-1111-1111-1111-111111111111'],
  duration_days: 14,
  variants: [
    { key: 'control', allocation: 50 },
    { key: 'treatment', allocation: 50 },
  ],
}

async function validate(
  patch: Partial<FormShape>,
): Promise<{ ok: true } | { ok: false; errors: Record<string, string> }> {
  try {
    await experimentSchema.validate({ ...validBase, ...patch }, { abortEarly: false })
    return { ok: true }
  } catch (err) {
    if (err instanceof ValidationError) {
      const errors: Record<string, string> = {}
      for (const inner of err.inner) {
        if (inner.path && !(inner.path in errors)) errors[inner.path] = inner.message
      }
      return { ok: false, errors }
    }
    throw err
  }
}

// ─── Validation tests ────────────────────────────────────────────────────────

describe('test_validation_requires_at_least_one_metric_id', () => {
  it('rejects an empty metric_ids array', async () => {
    const result = await validate({ metric_ids: [] })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.errors.metric_ids).toMatch(/at least 1|required/i)
  })

  it('accepts a single UUID', async () => {
    const result = await validate({
      metric_ids: ['11111111-1111-1111-1111-111111111111'],
    })
    expect(result.ok).toBe(true)
  })
})

describe('test_validation_rejects_more_than_5_metric_ids', () => {
  it('accepts exactly 5 metric_ids', async () => {
    const ids = [
      '11111111-1111-1111-1111-111111111111',
      '22222222-2222-2222-2222-222222222222',
      '33333333-3333-3333-3333-333333333333',
      '44444444-4444-4444-4444-444444444444',
      '55555555-5555-5555-5555-555555555555',
    ]
    const result = await validate({ metric_ids: ids })
    expect(result.ok).toBe(true)
  })

  it('rejects 6 metric_ids', async () => {
    const ids = [
      '11111111-1111-1111-1111-111111111111',
      '22222222-2222-2222-2222-222222222222',
      '33333333-3333-3333-3333-333333333333',
      '44444444-4444-4444-4444-444444444444',
      '55555555-5555-5555-5555-555555555555',
      '66666666-6666-6666-6666-666666666666',
    ]
    const result = await validate({ metric_ids: ids })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.errors.metric_ids).toMatch(/at most 5|max/i)
  })
})

describe('test_validation_rejects_non_uuid_metric_id', () => {
  it('rejects a non-UUID string', async () => {
    const result = await validate({ metric_ids: ['not-a-uuid'] })
    expect(result.ok).toBe(false)
    if (!result.ok) {
      // Either the parent or the element path fails — both indicate a bad value.
      const errKey = Object.keys(result.errors).find((k) => k.startsWith('metric_ids'))
      expect(errKey).toBeDefined()
    }
  })
})

// ─── Picker rendering helpers ────────────────────────────────────────────────

describe('test_metric_picker_shows_kind_chip', () => {
  it('uses blue for aggregation, purple for ratio, green for funnel', () => {
    expect(metricKindChipStyle('aggregation').color).toBe('#2563eb')
    expect(metricKindChipStyle('ratio').color).toBe('#9333ea')
    expect(metricKindChipStyle('funnel').color).toBe('#16a34a')
  })

  it('returns a distinct background per kind', () => {
    const a = metricKindChipStyle('aggregation')
    const r = metricKindChipStyle('ratio')
    const f = metricKindChipStyle('funnel')
    expect(a.background).not.toBe(r.background)
    expect(r.background).not.toBe(f.background)
    expect(a.background).not.toBe(f.background)
  })
})

describe('test_metric_picker_search_filters_by_name_and_key', () => {
  const opts: MetricPickerOption[] = [
    { id: 'a', key: 'checkout_completed', name: 'Checkout completed', kind: 'aggregation' },
    { id: 'b', key: 'cart_value', name: 'Cart value (USD)', kind: 'aggregation' },
    { id: 'c', key: 'signup_funnel', name: 'Signup funnel', kind: 'funnel' },
    { id: 'd', key: 'conversion_rate', name: 'Conversion ratio', kind: 'ratio' },
  ]

  it('returns all options when query is empty', () => {
    expect(filterMetricOptions(opts, '')).toHaveLength(4)
  })

  it('filters by name (case-insensitive substring)', () => {
    const r = filterMetricOptions(opts, 'cart')
    expect(r.map((o) => o.id)).toEqual(['b'])
  })

  it('filters by key (case-insensitive substring)', () => {
    const r = filterMetricOptions(opts, 'CONVERSION')
    expect(r.map((o) => o.id)).toEqual(['d'])
  })

  it('returns empty when nothing matches', () => {
    expect(filterMetricOptions(opts, 'nothing-here')).toHaveLength(0)
  })

  it('matches against both name and key — name wins if both match', () => {
    const r = filterMetricOptions(opts, 'signup')
    expect(r.map((o) => o.id)).toEqual(['c'])
  })

  it('hides options already selected', () => {
    const r = filterMetricOptions(opts, '', new Set(['a', 'b']))
    expect(r.map((o) => o.id)).toEqual(['c', 'd'])
  })
})

// ─── Request body shape ─────────────────────────────────────────────────────

describe('test_form_submits_metric_ids_array_to_backend', () => {
  it('builds a body with metric_ids array (NOT primary_metric)', () => {
    const body = buildExperimentCreateBody(validBase, 'env-1')
    expect(body.metric_ids).toEqual(['11111111-1111-1111-1111-111111111111'])
    expect(body.primary_metric).toBeUndefined()
  })

  it('preserves all required fields from the form', () => {
    const body = buildExperimentCreateBody(validBase, 'env-42')
    expect(body).toMatchObject({
      key: 'checkout-btn-colour',
      name: 'Checkout button colour',
      flag_key: 'checkout-btn-colour',
      model: 'bayesian',
      duration_days: 14,
      environment_id: 'env-42',
    })
  })

  it('maps allocation percentages to 0–1 floats', () => {
    const body = buildExperimentCreateBody(validBase, 'env-1')
    const variants = body.variants as { key: string; allocation: number }[]
    expect(variants[0].allocation).toBe(0.5)
    expect(variants[1].allocation).toBe(0.5)
  })

  it('trims whitespace from name, key, flag_key, description', () => {
    const body = buildExperimentCreateBody(
      { ...validBase, name: '  Name  ', key: '  k  ', flag_key: '  f  ', description: '  d  ' },
      'env-1',
    )
    expect(body.name).toBe('Name')
    expect(body.key).toBe('k')
    expect(body.flag_key).toBe('f')
    expect(body.description).toBe('d')
  })

  it('passes through multiple metric_ids in order', () => {
    const ids = [
      '11111111-1111-1111-1111-111111111111',
      '22222222-2222-2222-2222-222222222222',
      '33333333-3333-3333-3333-333333333333',
    ]
    const body = buildExperimentCreateBody({ ...validBase, metric_ids: ids }, 'env-1')
    expect(body.metric_ids).toEqual(ids)
  })
})
