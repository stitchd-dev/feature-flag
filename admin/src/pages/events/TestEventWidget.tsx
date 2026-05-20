/**
 * TestEventWidget — fire a synthetic event from the event detail page.
 *
 * Renders a small Formik form letting the admin POST a one-off event to
 * `POST /v1/events/track` for SDK / pipeline debugging. The form's `value`
 * input shape is constrained by the event's `metric_type`:
 *
 *   - `count`                     → no value (counter)
 *   - `numeric`/`revenue`/`duration` → numeric input → wrapped `{double: N}`
 *   - `conversion`                → boolean toggle  → wrapped `{bool: B}`
 *   - `custom`                    → free JSON       → forwarded raw
 *
 * Pure builders (`buildTrackBody`, `parseTypedValue`, `validProperties`) are
 * exported for the Vitest suite in `TestEventWidget.test.ts` to exercise
 * directly — mirrors the pattern used by `EditEventModal` / `EventDetail`.
 *
 * ## Admin-auth path
 *
 * Posts to `POST /v1/admin/events/track` (JWT-auth tier), NOT the SDK-auth
 * `/v1/events/track` route. The admin path resolves env_id from the JWT's
 * `RbacContext` and stamps every event with `properties._test = "true"` so
 * production aggregates can filter the test firings out. Backend landed in
 * beads `feature-flag-gda`.
 *
 * ## Known gaps
 *
 * 1. **Live-preview from ClickHouse**: The "did my event land?" round-trip is
 *    blocked on the `/v1/events/{key}/firings` endpoint (beads bug
 *    `feature-flag-uz3`). Until it lands, the success banner just says
 *    "Event fired — see firings log above for confirmation".
 *
 * @see feature-flag-7an.6.4 — this task
 * @see feature-flag-gda     — backend: admin-auth path for /v1/events/track (landed)
 * @see feature-flag-uz3     — backend gap: firings + stats endpoints
 */
import { useState } from 'react'
import { Formik, Form, FieldArray, type FormikHelpers } from 'formik'
import { I } from '../../components/icons'
import { FormField } from '../../components/form/FormField'
import { FormSelect } from '../../components/form/FormSelect'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'

// ── Types ──────────────────────────────────────────────────────────────────────

/** One context-dimension row in the form. Maps 1:1 to the wire-format
 *  `EventContextBody` on `POST /v1/admin/events/track`. */
export interface TestEventContextRow {
  context_type: string
  context_key: string
}

export interface TestEventFormValues {
  /**
   * One or more attribution dimensions for this firing. Defaults to a
   * single row for the common case; the "Add context" button appends
   * more so a single event can be attributed to e.g. user + account +
   * session simultaneously (see `feature-flag-5wr`).
   */
  contexts: TestEventContextRow[]
  /** Free-text form input; shape interpreted via `metric_type` at submit time. */
  value: string
  /** Optional JSON object literal. Empty string = "no properties". */
  properties: string
}

/**
 * Typed value envelope sent on the wire — matches the gateway's
 * `TypedValue` enum:
 *
 *   - `{"bool": B}`   → `MetricValue::BoolValue`
 *   - `{"int":  N}`   → `MetricValue::IntValue`   (we never emit this from the UI)
 *   - `{"double": N}` → `MetricValue::DoubleValue`
 *
 * For `custom` metric types we forward the parsed JSON value as-is — the
 * analytics service decides the canonical shape downstream.
 */
type WireValue =
  | { bool: boolean }
  | { int: number }
  | { double: number }
  | unknown

export interface TrackEvent {
  event_key: string
  /** Multi-dimensional attribution. The gateway prefers this over the
   *  deprecated singular fields and the analytics-service forwards the
   *  full list into the ClickHouse `Array(Tuple(String, String))` column. */
  contexts: TestEventContextRow[]
  value?: WireValue
  properties?: Record<string, unknown>
}

interface TrackEventsBody {
  events: TrackEvent[]
  /** Sent so the admin-auth handler can resolve env_id when the JWT is
   *  org-scoped (the common case — admin JWTs don't carry an env scope). */
  environment_id?: string
}

interface Props {
  /** Event key from the URL — wired in to the request body verbatim. */
  eventKey: string
  /** Event's `metric_type` — constrains the `value` field's input shape. */
  metricType: string
  /** Currently-selected env from `OrgContext` — wired into the request body
   *  so the admin-auth handler can fire to the right environment. */
  environmentId?: string
  /** Optional refresh-firings-log hook. Called after a 2xx response. */
  onSubmitted?: () => void
}

// ── Pure builders (mirrored / exported for Vitest) ────────────────────────────

/**
 * Parse the raw form `value` string into the wire-format `value` envelope
 * appropriate for the event's `metric_type`. Returns `null` when:
 *   - `metric_type === 'count'` (counters have no value)
 *   - the raw input fails to parse for the given metric_type
 */
export function parseTypedValue(raw: string, metricType: string): WireValue | null {
  switch (metricType) {
    case 'count':
      return null

    case 'numeric':
    case 'revenue':
    case 'duration': {
      const trimmed = raw.trim()
      if (!trimmed) return null
      const n = Number(trimmed)
      if (!Number.isFinite(n)) return null
      return { double: n }
    }

    case 'conversion': {
      if (raw === 'true') return { bool: true }
      if (raw === 'false') return { bool: false }
      return null
    }

    case 'custom': {
      const trimmed = raw.trim()
      if (!trimmed) return null
      try {
        return JSON.parse(trimmed) as WireValue
      } catch {
        return null
      }
    }

    default:
      return null
  }
}

/**
 * Returns true iff `rawJson` is either empty/whitespace OR a valid JSON
 * **object**. Arrays / scalars / `null` are rejected — `properties` must be a
 * map per the wire contract.
 */
export function validProperties(rawJson: string): boolean {
  const trimmed = rawJson.trim()
  if (!trimmed) return true
  try {
    const parsed = JSON.parse(trimmed)
    return (
      parsed !== null &&
      typeof parsed === 'object' &&
      !Array.isArray(parsed)
    )
  } catch {
    return false
  }
}

/**
 * Build the `POST /v1/events/track` body from form values + the page-level
 * `event_key` + `metric_type`. Pure — no IO.
 *
 * Each context row gets trimmed; rows that are entirely blank are dropped
 * so an empty "Add context" placeholder doesn't sneak onto the wire. The
 * caller is responsible for validating that at least one row remains
 * (see the `validate` function in the component).
 */
export function buildTrackBody(
  values: TestEventFormValues,
  eventKey: string,
  metricType: string,
  environmentId?: string,
): TrackEventsBody {
  const contexts = values.contexts
    .map((c) => ({
      context_type: c.context_type.trim(),
      context_key: c.context_key.trim(),
    }))
    .filter((c) => c.context_type !== '' && c.context_key !== '')

  const event: TrackEvent = {
    event_key: eventKey,
    contexts,
  }

  const typed = parseTypedValue(values.value, metricType)
  if (typed !== null) event.value = typed

  const propsRaw = values.properties.trim()
  if (propsRaw) {
    try {
      const parsed = JSON.parse(propsRaw)
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        event.properties = parsed as Record<string, unknown>
      }
    } catch {
      // validProperties() already guards the submit path; defensively skip.
    }
  }

  return environmentId ? { events: [event], environment_id: environmentId } : { events: [event] }
}

// ── Component ──────────────────────────────────────────────────────────────────

function valueFieldLabel(metricType: string): string {
  switch (metricType) {
    case 'count':
      return 'Value (none — counter)'
    case 'conversion':
      return 'Value (boolean)'
    case 'revenue':
      return 'Value (currency amount)'
    case 'duration':
      return 'Value (seconds)'
    case 'numeric':
      return 'Value (number)'
    case 'custom':
      return 'Value (JSON)'
    default:
      return 'Value'
  }
}

function valueFieldHint(metricType: string): string {
  switch (metricType) {
    case 'count':
      return 'No value needed — every fire is one count.'
    case 'conversion':
      return 'Sent on the wire as { "bool": true|false }.'
    case 'revenue':
    case 'duration':
    case 'numeric':
      return 'Sent on the wire as { "double": N }.'
    case 'custom':
      return 'Free-form JSON forwarded to analytics-service as-is.'
    default:
      return ''
  }
}

interface FormStatus {
  error?: string
  success?: string
}

export function TestEventWidget({ eventKey, metricType, environmentId, onSubmitted }: Props) {
  const [collapsed, setCollapsed] = useState(true)

  const initialValues: TestEventFormValues = {
    contexts: [{ context_type: 'user', context_key: '' }],
    value: metricType === 'conversion' ? 'true' : '',
    properties: '',
  }

  function validate(
    values: TestEventFormValues,
  ): Partial<Record<keyof TestEventFormValues, string>> {
    const errors: Partial<Record<keyof TestEventFormValues, string>> = {}

    // At least one valid context — `buildTrackBody` drops blank rows, so
    // we require >=1 non-blank pair.
    const nonBlank = values.contexts.filter(
      (c) => c.context_type.trim() !== '' && c.context_key.trim() !== '',
    )
    if (nonBlank.length === 0) {
      errors.contexts = 'At least one context (type + key) is required'
    }

    if (metricType !== 'count') {
      const parsed = parseTypedValue(values.value, metricType)
      if (parsed === null && values.value.trim()) {
        errors.value = 'Value is not valid for this metric type'
      }
    }

    if (!validProperties(values.properties)) {
      errors.properties = 'Properties must be a JSON object'
    }

    return errors
  }

  async function handleSubmit(
    values: TestEventFormValues,
    { setStatus, resetForm }: FormikHelpers<TestEventFormValues>,
  ) {
    try {
      const body = buildTrackBody(values, eventKey, metricType, environmentId)
      // Admin-auth path — accepts the browser session's bearer JWT, resolves
      // env_id from RbacContext when env-scoped, falls back to body's
      // `environment_id` for org-scoped JWTs (the common case for the test
      // widget). Stamps `_test=true` on every event so analytics-service
      // rows are filterable. See feature-flag-gda.
      await api.post('/v1/admin/events/track', body)
      setStatus({ success: 'Event fired — see firings log above for confirmation.' })
      // Reset only the value/properties fields, keep the context so the admin
      // can fire a second event quickly.
      resetForm({
        values: {
          ...values,
          properties: '',
        },
      })
      onSubmitted?.()
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  const conversionOptions = [
    { value: 'true', label: 'true' },
    { value: 'false', label: 'false' },
  ]

  return (
    <div className="card" style={{ marginBottom: 18 }}>
      <div
        className="card-header"
        style={{ cursor: 'pointer', userSelect: 'none' }}
        onClick={() => setCollapsed((c) => !c)}
        role="button"
        aria-expanded={!collapsed}
      >
        <div className="card-title">
          <I.zap size={14} /> Fire a test event
        </div>
        <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
          {collapsed ? 'Expand' : 'Collapse'}
        </div>
      </div>

      {!collapsed && (
        <div style={{ padding: 20 }}>
          <Formik
            initialValues={initialValues}
            validate={validate}
            onSubmit={handleSubmit}
          >
            {({ status }) => {
              const formStatus = status as FormStatus | undefined
              return (
                <Form
                  id="test-event-form"
                  style={{ display: 'flex', flexDirection: 'column', gap: 14 }}
                >
                  <FormErrorBanner />

                  {formStatus?.success && (
                    <div
                      role="status"
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 8,
                        padding: '10px 14px',
                        background: 'var(--success-bg, #e6f4ea)',
                        border: '1px solid rgba(36,128,69,0.3)',
                        borderRadius: 6,
                        color: 'var(--success, #1e7e3b)',
                        fontSize: 13,
                      }}
                    >
                      <I.check size={13} style={{ flexShrink: 0 }} />
                      <span>{formStatus.success}</span>
                    </div>
                  )}

                  {/* Multi-context attribution — feature-flag-5wr. The
                      common case is one row; click "Add context" to
                      attribute a single firing to multiple dimensions
                      (e.g. user + account + session) without inflating
                      count metrics. */}
                  <FieldArray name="contexts">
                    {({ push, remove, form }) => {
                      const rows = (form.values as TestEventFormValues).contexts
                      return (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                            <label className="label">
                              Contexts <span style={{ color: 'var(--fg-muted)', fontWeight: 400 }}>(at least 1)</span>
                            </label>
                            <button
                              type="button"
                              className="btn sm"
                              onClick={() => push({ context_type: '', context_key: '' })}
                              data-testid="add-context"
                            >
                              <I.plus size={12} /> Add context
                            </button>
                          </div>
                          {rows.map((_, idx) => (
                            <div
                              key={idx}
                              data-testid={`context-row-${idx}`}
                              style={{
                                display: 'grid',
                                gridTemplateColumns: '1fr 1fr auto',
                                gap: 10,
                                alignItems: 'flex-end',
                              }}
                            >
                              <FormField
                                name={`contexts.${idx}.context_type`}
                                label={idx === 0 ? 'Context type' : ''}
                                placeholder="user"
                                hint={idx === 0 ? 'Dimension type (e.g. user, session, org).' : undefined}
                              />
                              <FormField
                                name={`contexts.${idx}.context_key`}
                                label={idx === 0 ? 'Context key' : ''}
                                placeholder="u-42"
                                hint={idx === 0 ? 'Dimension value (e.g. user-id, session-id).' : undefined}
                                style={{ fontFamily: 'var(--font-mono)' }}
                              />
                              {rows.length > 1 && (
                                <button
                                  type="button"
                                  className="icon-btn"
                                  onClick={() => remove(idx)}
                                  title="Remove context"
                                  style={{ color: 'var(--danger)', marginBottom: 12 }}
                                  data-testid={`remove-context-${idx}`}
                                >
                                  <I.trash size={12} />
                                </button>
                              )}
                            </div>
                          ))}
                        </div>
                      )
                    }}
                  </FieldArray>

                  {metricType === 'count' ? (
                    <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
                      <strong>{valueFieldLabel(metricType)}</strong>
                      <div style={{ marginTop: 4 }}>{valueFieldHint(metricType)}</div>
                    </div>
                  ) : metricType === 'conversion' ? (
                    <FormSelect
                      name="value"
                      label={valueFieldLabel(metricType)}
                      options={conversionOptions}
                      hint={valueFieldHint(metricType)}
                    />
                  ) : metricType === 'custom' ? (
                    <FormTextarea
                      name="value"
                      label={valueFieldLabel(metricType)}
                      placeholder='{"amount": 42, "currency": "USD"}'
                      hint={valueFieldHint(metricType)}
                      style={{ fontFamily: 'var(--font-mono)', fontSize: 12, minHeight: 64 }}
                    />
                  ) : (
                    <FormField
                      name="value"
                      label={valueFieldLabel(metricType)}
                      type="number"
                      placeholder="0"
                      hint={valueFieldHint(metricType)}
                    />
                  )}

                  <FormTextarea
                    name="properties"
                    label="Properties (optional JSON)"
                    placeholder='{"plan": "pro"}'
                    hint="Optional per-event metadata. Must be a JSON object."
                    style={{ fontFamily: 'var(--font-mono)', fontSize: 12, minHeight: 64 }}
                  />

                  <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
                    <FormSubmit
                      label="Fire event"
                      loadingLabel="Firing…"
                      form="test-event-form"
                    />
                  </div>
                </Form>
              )
            }}
          </Formik>
        </div>
      )}
    </div>
  )
}
