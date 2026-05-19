import { useEffect, useState } from 'react'
import { useFormikContext, FieldArray } from 'formik'
import { I } from '../../components/icons'
import { FormField } from '../../components/form/FormField'
import { FormSelect } from '../../components/form/FormSelect'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormCheckbox } from '../../components/form/FormCheckbox'
import { api } from '../../lib/api'
import { AGGREGATORS, aggregatorRequiresField } from '../../lib/validation/metricSchema'
import type { MetricFormValues } from '../../lib/validation/metricSchema'
import type { MetricResponse } from './MetricsList'

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
  offset: number
  limit: number
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

  return (
    <>
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
    </>
  )
}

// ─── Aggregation fields ──────────────────────────────────────────────────────

function AggregationFields() {
  const { values } = useFormikContext<MetricFormValues>()
  const needsField = aggregatorRequiresField(values.aggregator)

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
      <FormField name="event_key" label="Event key" placeholder="e.g. checkout_completed" />
      <FormSelect name="aggregator" label="Aggregator" options={AGGREGATOR_OPTIONS} />
      <FormField
        name="on_field"
        label="On field"
        placeholder={needsField ? 'e.g. revenue (numeric property)' : 'Not used for count'}
        disabled={!needsField}
        hint={
          needsField
            ? 'Numeric property name within the event payload.'
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
      .get<ListMetricsResponse>(`/v1/metrics?env_id=${envId}&limit=200&kind=aggregation`)
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
                <FormField
                  name={`steps.${idx}.event_key`}
                  label="Event key"
                  placeholder="e.g. checkout_started"
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
