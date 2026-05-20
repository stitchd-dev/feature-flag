/**
 * TestEventWidget — logic tests (pure, no DOM).
 *
 * Tests the pure builders that the widget uses to construct
 * `POST /v1/admin/events/track` payloads. Follows the same pattern as
 * `EventDetail.test.ts` / `EditEventModal.test.ts`: helpers exported from the
 * component module are exercised here, while React rendering is left to the
 * Formik / Modal primitives (already covered).
 *
 * See `TestEventWidget.tsx` for the live helpers; this file imports them
 * directly so tests catch drift between mirrors and implementation.
 */
import { describe, it, expect } from 'vitest'
import {
  buildTrackBody,
  parseTypedValue,
  validProperties,
  type TestEventFormValues,
} from './TestEventWidget'

// ── Fixtures ───────────────────────────────────────────────────────────────────

function baseValues(overrides: Partial<TestEventFormValues> = {}): TestEventFormValues {
  return {
    context_type: 'user',
    context_key: 'u-42',
    value: '',
    properties: '',
    ...overrides,
  }
}

// ── buildTrackBody ─────────────────────────────────────────────────────────────

describe('TestEventWidget — buildTrackBody', () => {
  it('buildTrackBody_count_omits_value', () => {
    const body = buildTrackBody(baseValues({ value: '' }), 'checkout.completed', 'count')
    expect(body.events).toHaveLength(1)
    const ev = body.events[0]
    expect(ev.event_key).toBe('checkout.completed')
    expect(ev.context_type).toBe('user')
    expect(ev.context_key).toBe('u-42')
    expect(ev.value).toBeUndefined()
  })

  it('buildTrackBody_numeric_wraps_in_double', () => {
    const body = buildTrackBody(baseValues({ value: '12.5' }), 'revenue.usd', 'revenue')
    expect(body.events[0].value).toEqual({ double: 12.5 })
  })

  it('buildTrackBody_numeric_metric_duration_wraps_in_double', () => {
    const body = buildTrackBody(baseValues({ value: '300' }), 'page.duration', 'duration')
    expect(body.events[0].value).toEqual({ double: 300 })
  })

  it('buildTrackBody_conversion_wraps_in_bool', () => {
    const trueBody = buildTrackBody(baseValues({ value: 'true' }), 'signup', 'conversion')
    expect(trueBody.events[0].value).toEqual({ bool: true })

    const falseBody = buildTrackBody(baseValues({ value: 'false' }), 'signup', 'conversion')
    expect(falseBody.events[0].value).toEqual({ bool: false })
  })

  it('buildTrackBody_custom_parses_json', () => {
    const body = buildTrackBody(
      baseValues({ value: '{"amount": 42, "currency": "USD"}' }),
      'order.placed',
      'custom',
    )
    // Custom values are forwarded raw — analytics-service decides the shape.
    expect(body.events[0].value).toEqual({ amount: 42, currency: 'USD' })
  })

  it('buildTrackBody_includes_properties_when_present', () => {
    const body = buildTrackBody(
      baseValues({ value: '', properties: '{"plan": "pro", "region": "us-east"}' }),
      'login',
      'count',
    )
    expect(body.events[0].properties).toEqual({ plan: 'pro', region: 'us-east' })
  })

  it('buildTrackBody_omits_properties_when_empty', () => {
    const body = buildTrackBody(baseValues({ properties: '' }), 'login', 'count')
    expect(body.events[0].properties).toBeUndefined()
  })

  it('buildTrackBody_omits_properties_when_whitespace_only', () => {
    const body = buildTrackBody(baseValues({ properties: '   ' }), 'login', 'count')
    expect(body.events[0].properties).toBeUndefined()
  })
})

// ── parseTypedValue ────────────────────────────────────────────────────────────

describe('TestEventWidget — parseTypedValue', () => {
  it('parseTypedValue_count_returns_null', () => {
    // Counters never carry a value — even if the form somehow has one, it's
    // ignored at submit time.
    expect(parseTypedValue('123', 'count')).toBeNull()
    expect(parseTypedValue('', 'count')).toBeNull()
  })

  it('parseTypedValue_numeric_rejects_non_number', () => {
    expect(parseTypedValue('not-a-number', 'revenue')).toBeNull()
    expect(parseTypedValue('', 'revenue')).toBeNull()
    expect(parseTypedValue('   ', 'revenue')).toBeNull()
  })

  it('parseTypedValue_numeric_accepts_valid_number', () => {
    expect(parseTypedValue('42', 'revenue')).toEqual({ double: 42 })
    expect(parseTypedValue('3.14', 'revenue')).toEqual({ double: 3.14 })
    expect(parseTypedValue('0', 'revenue')).toEqual({ double: 0 })
    expect(parseTypedValue('-1.5', 'duration')).toEqual({ double: -1.5 })
  })

  it('parseTypedValue_conversion_parses_truthy', () => {
    expect(parseTypedValue('true', 'conversion')).toEqual({ bool: true })
    expect(parseTypedValue('false', 'conversion')).toEqual({ bool: false })
    // Anything else → null (form should not allow this, but be defensive).
    expect(parseTypedValue('yes', 'conversion')).toBeNull()
  })

  it('parseTypedValue_custom_parses_json', () => {
    expect(parseTypedValue('{"a":1}', 'custom')).toEqual({ a: 1 })
    expect(parseTypedValue('[1,2,3]', 'custom')).toEqual([1, 2, 3])
    // Invalid JSON → null
    expect(parseTypedValue('not json', 'custom')).toBeNull()
  })
})

// ── validProperties ────────────────────────────────────────────────────────────

describe('TestEventWidget — validProperties', () => {
  it('validProperties_accepts_empty_string', () => {
    expect(validProperties('')).toBe(true)
    expect(validProperties('   ')).toBe(true)
  })

  it('validProperties_accepts_valid_json_object', () => {
    expect(validProperties('{"a":1}')).toBe(true)
    expect(validProperties('{}')).toBe(true)
  })

  it('validProperties_rejects_invalid_json', () => {
    expect(validProperties('{not json')).toBe(false)
    expect(validProperties('}{')).toBe(false)
  })

  it('validProperties_rejects_non_object_json', () => {
    // Properties must be a JSON object — arrays/scalars rejected.
    expect(validProperties('[1,2,3]')).toBe(false)
    expect(validProperties('42')).toBe(false)
    expect(validProperties('"string"')).toBe(false)
    expect(validProperties('null')).toBe(false)
  })
})
