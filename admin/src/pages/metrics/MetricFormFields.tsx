import { createContext, useContext, useEffect, useState } from 'react'
import { useFormikContext, FieldArray } from 'formik'
import { I } from '../../components/icons'
import { FormField } from '../../components/form/FormField'
import { FormSelect } from '../../components/form/FormSelect'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormCheckbox } from '../../components/form/FormCheckbox'
import { EventKeyField } from '../../components/form/EventKeyField'
import { api } from '../../lib/api'
import { AGGREGATORS, aggregatorUsesField } from '../../lib/validation/metricSchema'
import type { MetricFormValues } from '../../lib/validation/metricSchema'
import type { MetricResponse } from './MetricsList'

/** Shape of a row in the GET /v1/events list response — only the keys we
 *  need from the typeahead. Defined inline (rather than imported) so this
 *  module stays decoupled from the EventsList page module. */
interface EventListItem { event_key: string }
interface ListEventsResponse {
  items: EventListItem[]
  total?: number
}

/** In-modal cache of the env's registered event keys, populated once on
 *  mount and consumed by both AggregationFields and FunnelFields. Empty
 *  array (loaded=true) means "fetched, env has no events" — distinct from
 *  loaded=false ("still fetching"). */
interface EventKeysCtx {
  keys: string[]
  loading: boolean
  loadError: string | null
}
const EventKeysContext = createContext<EventKeysCtx>({
  keys: [],
  loading: true,
  loadError: null,
})

interface MetricFormFieldsProps {
  envId: string
  /** When true, the `key` field is rendered as a read-only span. */
  keyReadOnly?: boolean
  /** ID of the metric being edited — excluded from ratio dropdowns. */
  excludeMetricId?: string
}

interface ListMetricsResponse {
  items: MetricResponse[]
  total: number
  page: number
  per_page: number
}

const AGGREGATOR_OPTIONS = AGGREGATORS.map((a) => ({ value: a, label: a }))

const GOAL_OPTIONS = [
  { value: 'increase', label: 'Increase (↑ higher is better)' },
  { value: 'decrease', label: 'Decrease (↓ lower is better)' },
  { value: 'neutral', label: 'Neutral (→ tracking only)' },
]

const KIND_OPTIONS: { value: 'aggregation' | 'ratio' | 'funnel'; label: string; icon: 'metric' | 'ratio' | 'funnel'; description: string }[] = [
  {
    value: 'aggregation',
    label: 'Aggregation',
    icon: 'metric',
    description: 'count / sum / avg / percentile over one event stream',
  },
  {
    value: 'ratio',
    label: 'Ratio',
    icon: 'ratio',
    description: 'numerator metric / denominator metric',
  },
  {
    value: 'funnel',
    label: 'Funnel',
    icon: 'funnel',
    description: 'ordered sequence of events within a window',
  },
]

/**
 * Renders the kind-discriminated fields of the metric form.
 *
 * Architecture:
 *   - Reads the current `kind` from Formik context (subscribed via
 *     `useFormikContext`)
 *   - For ratio metrics, fetches the list of aggregation metrics in the
 *     env and renders a `<select>` for numerator + denominator. The
 *     metric currently being edited (if any) is excluded.
 *   - Step rows for funnel metrics are managed via `<FieldArray>`.
 */
export function MetricFormFields({ envId, keyReadOnly = false, excludeMetricId }: MetricFormFieldsProps) {
  const { values, setFieldValue } = useFormikContext<MetricFormValues>()
  const kind = values.kind

  // Load event-key catalog once per envId so aggregation + funnel pickers
  // both consume the same list. per_page=200 matches the gateway cap; the
  // realistic upper bound for events-per-env is well below this.
  const [eventKeys, setEventKeys] = useState<string[]>([])
  const [eventsLoading, setEventsLoading] = useState(true)
  const [eventsLoadError, setEventsLoadError] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    setEventsLoading(true)
    setEventsLoadError(null)
    api
      .get<ListEventsResponse>(`/v1/events?env_id=${envId}&page=1&per_page=200`)
      .then(({ data }) => {
        if (cancelled) return
        const keys = (data.items ?? [])
          .map((e) => e.event_key)
          .filter((k): k is string => typeof k === 'string' && k.length > 0)
        setEventKeys(keys)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        setEventsLoadError(
          err instanceof Error ? err.message : 'Failed to load registered events',
        )
      })
      .finally(() => {
        if (!cancelled) setEventsLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [envId])

  return (
    <EventKeysContext.Provider
      value={{ keys: eventKeys, loading: eventsLoading, loadError: eventsLoadError }}
    >
      {/* Kind selector — shown for create mode; on edit the kind is
          locked because the server treats it as immutable. */}
      {!keyReadOnly && (
        <div>
          <label className="label" style={{ display: 'block', marginBottom: 6 }}>
            Metric kind <span style={{ color: 'var(--danger)' }}>*</span>
          </label>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 8 }}>
            {KIND_OPTIONS.map((opt) => {
              const Ic = I[opt.icon]
              const selected = kind === opt.value
              return (
                <button
                  key={opt.value}
                  type="button"
                  onClick={() => {
                    void setFieldValue('kind', opt.value)
                  }}
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    alignItems: 'flex-start',
                    gap: 4,
                    padding: '10px 12px',
                    border: `2px solid ${selected ? 'var(--primary)' : 'var(--border)'}`,
                    borderRadius: 8,
                    background: selected ? 'var(--primary-bg, rgba(229,79,53,0.06))' : 'var(--surface)',
                    cursor: 'pointer',
                    textAlign: 'left',
                    transition: 'border-color 0.15s, background 0.15s',
                  }}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      fontWeight: 600,
                      fontSize: 12,
                      color: selected ? 'var(--primary)' : 'var(--fg)',
                    }}
                  >
                    <Ic size={13} />
                    {opt.label}
                  </div>
                  <div style={{ fontSize: 10, color: 'var(--fg-muted)', lineHeight: 1.4 }}>
                    {opt.description}
                  </div>
                </button>
              )
            })}
          </div>
        </div>
      )}

      {kind && (
        <>
          {/* ─── Common fields ─────────────────────────────────────── */}
          {keyReadOnly ? (
            <div>
              <label className="label" style={{ display: 'block', marginBottom: 4 }}>
                Key
              </label>
              <div
                style={{
                  fontFamily: 'var(--font-mono)',
                  fontSize: 13,
                  padding: '6px 10px',
                  background: 'var(--bg-sunken)',
                  border: '1px solid var(--border)',
                  borderRadius: 6,
                  color: 'var(--fg-muted)',
                }}
              >
                {values.key}
              </div>
            </div>
          ) : (
            <FormField
              name="key"
              label="Key"
              placeholder="e.g. checkout_rate"
              hint="Unique per environment. Lowercase letters, digits, hyphens, underscores."
              autoFocus
            />
          )}

          <FormField name="name" label="Name" placeholder="e.g. Checkout Rate" />

          <FormTextarea
            name="description"
            label="Description"
            placeholder="Optional description of what this metric represents"
            style={{ minHeight: 60 }}
          />

          <FormSelect name="goal_direction" label="Goal direction" options={GOAL_OPTIONS} />

          {/* ─── Kind-specific blocks ──────────────────────────────── */}
          {kind === 'aggregation' && <AggregationFields />}
          {kind === 'ratio' && <RatioFields envId={envId} excludeMetricId={excludeMetricId} />}
          {kind === 'funnel' && <FunnelFields />}
        </>
      )}
    </EventKeysContext.Provider>
  )
}

// ─── Aggregation fields ──────────────────────────────────────────────────────

function AggregationFields() {
  const { values } = useFormikContext<MetricFormValues>()
  // Whether the aggregator makes use of a field at all — count never does.
  const usesField = aggregatorUsesField(values.aggregator)
  const { keys: eventKeys, loading: eventsLoading, loadError } = useContext(EventKeysContext)

  return (
    <div
      style={{
        padding: 12,
        border: '1px solid var(--border)',
        borderRadius: 8,
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
      }}
    >
      {loadError && (
        <div
          role="alert"
          style={{ fontSize: 11, color: 'var(--danger)' }}
          data-testid="event-keys-load-error"
        >
          {loadError}
        </div>
      )}
      <EventKeyField
        name="event_key"
        label="Event key"
        placeholder={
          eventsLoading
            ? 'Loading registered events…'
            : eventKeys.length === 0
              ? 'No events registered yet — register one first'
              : 'Start typing to filter…'
        }
        eventKeys={eventKeys}
        hint="Must be a pre-registered event in this environment."
        disabled={eventsLoading}
        testId="event-key-agg"
      />
      <FormSelect name="aggregator" label="Aggregator" options={AGGREGATOR_OPTIONS} />
      <FormField
        name="on_field"
        label="On field (optional)"
        placeholder={usesField ? 'e.g. revenue — leave blank for event numeric value' : 'Not used for count'}
        disabled={!usesField}
        hint={
          usesField
            ? 'Optional: property name to aggregate. Leave blank to use the event\'s built-in numeric value (value_double / value_int).'
            : 'Field is ignored for count — it just counts matching rows.'
        }
      />
      <FormTextarea
        name="where_clause"
        label="Where clause (JSON, optional)"
        placeholder='e.g. {"==":[{"var":"currency"},"USD"]}'
        hint="Optional JsonLogic filter applied to events before aggregation."
        style={{ minHeight: 64, fontFamily: 'var(--font-mono)', fontSize: 12 }}
      />
    </div>
  )
}

// ─── Ratio fields ────────────────────────────────────────────────────────────

function RatioFields({ envId, excludeMetricId }: { envId: string; excludeMetricId?: string }) {
  const [metrics, setMetrics] = useState<MetricResponse[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setLoadError(null)
    api
      .get<ListMetricsResponse>(`/v1/metrics?env_id=${envId}&per_page=200&kind=aggregation`)
      .then(({ data }) => {
        if (cancelled) return
        setMetrics(data.items ?? [])
      })
      .catch((err: unknown) => {
        if (cancelled) return
        setLoadError(err instanceof Error ? err.message : 'Failed to load metrics')
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [envId])

  const options = metrics
    .filter((m) => m.id !== excludeMetricId)
    .map((m) => ({ value: m.id, label: `${m.key} — ${m.name}` }))

  return (
    <div
      style={{
        padding: 12,
        border: '1px solid var(--border)',
        borderRadius: 8,
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
      }}
    >
      {loading ? (
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>Loading aggregation metrics…</div>
      ) : loadError ? (
        <div style={{ fontSize: 12, color: 'var(--danger)' }}>{loadError}</div>
      ) : metrics.length === 0 ? (
        <div
          style={{
            padding: '10px 12px',
            background: 'var(--bg-sunken)',
            borderRadius: 6,
            fontSize: 12,
            color: 'var(--fg-muted)',
            display: 'flex',
            gap: 8,
            alignItems: 'flex-start',
          }}
        >
          <I.info size={13} style={{ marginTop: 1, flexShrink: 0 }} />
          <span>
            You need at least one aggregation metric in this environment before you can create a ratio.
          </span>
        </div>
      ) : (
        <>
          <FormSelect
            name="numerator_metric_id"
            label="Numerator metric"
            options={[{ value: '', label: '— select —' }, ...options]}
          />
          <FormSelect
            name="denominator_metric_id"
            label="Denominator metric"
            options={[{ value: '', label: '— select —' }, ...options]}
          />
          <FormField
            name="min_denominator"
            label="Minimum denominator"
            type="number"
            hint="Below this value the metric reports 'Insufficient data' instead of a noisy ratio."
          />
        </>
      )}
    </div>
  )
}

// ─── Funnel fields ───────────────────────────────────────────────────────────

function FunnelFields() {
  const { values } = useFormikContext<MetricFormValues>()
  const { keys: eventKeys, loading: eventsLoading } = useContext(EventKeysContext)
  return (
    <div
      style={{
        padding: 12,
        border: '1px solid var(--border)',
        borderRadius: 8,
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
      }}
    >
      <FieldArray name="steps">
        {(arrayHelpers) => (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
              <label className="label">
                Steps <span style={{ color: 'var(--fg-muted)', fontWeight: 400 }}>(min 2)</span>
              </label>
              <button
                type="button"
                className="btn sm"
                onClick={() => arrayHelpers.push({ event_key: '', where_clause: '' })}
              >
                <I.plus size={12} /> Add step
              </button>
            </div>
            {values.steps.map((_, idx) => (
              <div
                key={idx}
                style={{
                  padding: 10,
                  background: 'var(--bg-sunken)',
                  borderRadius: 6,
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 8,
                  position: 'relative',
                }}
              >
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                  }}
                >
                  <div style={{ fontSize: 11, fontWeight: 600, color: 'var(--fg-muted)' }}>
                    Step {idx + 1}
                  </div>
                  {values.steps.length > 2 && (
                    <button
                      type="button"
                      className="icon-btn"
                      onClick={() => arrayHelpers.remove(idx)}
                      title="Remove step"
                      style={{ color: 'var(--danger)' }}
                    >
                      <I.trash size={12} />
                    </button>
                  )}
                </div>
                <EventKeyField
                  name={`steps.${idx}.event_key`}
                  label="Event key"
                  placeholder={
                    eventsLoading
                      ? 'Loading…'
                      : eventKeys.length === 0
                        ? 'No events registered yet'
                        : 'Start typing to filter…'
                  }
                  eventKeys={eventKeys}
                  disabled={eventsLoading}
                  testId={`event-key-step-${idx}`}
                />
                <FormTextarea
                  name={`steps.${idx}.where_clause`}
                  label="Where clause (JSON, optional)"
                  placeholder='e.g. {"==":[{"var":"plan"},"pro"]}'
                  style={{ minHeight: 50, fontFamily: 'var(--font-mono)', fontSize: 11 }}
                />
              </div>
            ))}
          </div>
        )}
      </FieldArray>
      <FormField
        name="window_seconds"
        label="Window (seconds)"
        type="number"
        hint="Maximum elapsed time for a context to count as converted (default 86400 = 1 day)."
      />
      <FormCheckbox
        name="count_repeats"
        label="Count repeated events as progression"
        hint="When unchecked, ClickHouse uses strict_order mode (the default for product funnels)."
      />
    </div>
  )
}
