/**
 * EditEventModal — logic tests (pure, no DOM).
 *
 * Mirrors the patterns established by `EventsList.test.ts` /
 * `EventDetail.test.ts`: test the pure derivations + payload builders, not
 * the React rendering surface (which Formik + Modal already cover at the
 * primitive level).
 */
import { describe, it, expect } from 'vitest'

// ── Types mirrored from EditEventModal.tsx ─────────────────────────────────────

export interface EventDefinition {
  event_key: string
  metric_type: string
  description: string
  schema: string | null
  version: number
}

export interface EditFormValues {
  metric_type: string
  description: string
  schema: string
}

// ── Pure builders mirrored from EditEventModal.tsx ─────────────────────────────

/**
 * Build the PATCH request body. Mirrors the inline construction in
 * `EditEventModal.handleSubmit`.
 */
export function buildEditBody(
  values: EditFormValues,
  event: EventDefinition,
): {
  metric_type: string
  description?: string
  schema: unknown
  expected_version: number
} {
  return {
    metric_type: values.metric_type,
    description: values.description?.trim() || undefined,
    schema: values.schema?.trim() ? JSON.parse(values.schema.trim()) : null,
    expected_version: event.version,
  }
}

/**
 * The PATCH URL for an event key, with proper URL encoding for keys that
 * contain dots / colons / hyphens (all valid per `eventDefinitionSchema`).
 */
export function buildEditUrl(eventKey: string): string {
  return `/v1/events/${encodeURIComponent(eventKey)}`
}

/** Initial-values derivation from a server-returned event. */
export function initialValuesFromEvent(event: EventDefinition): EditFormValues {
  return {
    metric_type: event.metric_type,
    description: event.description ?? '',
    schema: event.schema ?? '',
  }
}

// ── Fixtures ───────────────────────────────────────────────────────────────────

const baseEvent: EventDefinition = {
  event_key: 'checkout.completed',
  metric_type: 'count',
  description: 'Order placed',
  schema: null,
  version: 3,
}

// ── Tests ──────────────────────────────────────────────────────────────────────

describe('EditEventModal — buildEditBody', () => {
  it('forwards metric_type unchanged', () => {
    const body = buildEditBody(
      { metric_type: 'revenue', description: '', schema: '' },
      baseEvent,
    )
    expect(body.metric_type).toBe('revenue')
  })

  it('trims description and omits it when empty', () => {
    const trimmed = buildEditBody(
      { metric_type: 'count', description: '  Updated  ', schema: '' },
      baseEvent,
    )
    expect(trimmed.description).toBe('Updated')

    const empty = buildEditBody(
      { metric_type: 'count', description: '   ', schema: '' },
      baseEvent,
    )
    expect(empty.description).toBeUndefined()
  })

  it('parses schema as JSON when non-empty; null otherwise', () => {
    const withSchema = buildEditBody(
      {
        metric_type: 'count',
        description: '',
        schema: '{"type":"object"}',
      },
      baseEvent,
    )
    expect(withSchema.schema).toEqual({ type: 'object' })

    const noSchema = buildEditBody(
      { metric_type: 'count', description: '', schema: '' },
      baseEvent,
    )
    expect(noSchema.schema).toBeNull()
  })

  it('echoes back the event version for optimistic locking', () => {
    const body = buildEditBody(
      { metric_type: 'count', description: '', schema: '' },
      { ...baseEvent, version: 42 },
    )
    expect(body.expected_version).toBe(42)
  })

  it('does NOT carry event_key (key is immutable)', () => {
    const body = buildEditBody(
      { metric_type: 'count', description: '', schema: '' },
      baseEvent,
    )
    expect(body).not.toHaveProperty('event_key')
    expect(body).not.toHaveProperty('key')
  })
})

describe('EditEventModal — buildEditUrl', () => {
  it('encodes keys with dots/colons', () => {
    expect(buildEditUrl('checkout.completed')).toBe('/v1/events/checkout.completed')
    expect(buildEditUrl('user:signup')).toBe('/v1/events/user%3Asignup')
  })

  it('encodes keys with spaces (defence in depth — the schema rejects them but the URL builder still encodes)', () => {
    expect(buildEditUrl('a b')).toBe('/v1/events/a%20b')
  })
})

describe('EditEventModal — initialValuesFromEvent', () => {
  it('maps server fields to form fields', () => {
    expect(initialValuesFromEvent(baseEvent)).toEqual({
      metric_type: 'count',
      description: 'Order placed',
      schema: '',
    })
  })

  it('coerces null schema to empty string', () => {
    const v = initialValuesFromEvent({ ...baseEvent, schema: null })
    expect(v.schema).toBe('')
  })

  it('coerces non-null schema to the original string', () => {
    const v = initialValuesFromEvent({ ...baseEvent, schema: '{"type":"object"}' })
    expect(v.schema).toBe('{"type":"object"}')
  })

  it('coerces missing description to empty string (never undefined in form state)', () => {
    const v = initialValuesFromEvent({ ...baseEvent, description: '' })
    expect(v.description).toBe('')
  })
})
