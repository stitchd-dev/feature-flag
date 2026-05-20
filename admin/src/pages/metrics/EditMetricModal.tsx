import { useEffect, useState } from 'react'
import { Formik, Form } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import {
  metricSchema,
  initialMetricValues,
  parseWhereClause,
  aggregatorRequiresField,
} from '../../lib/validation/metricSchema'
import type { MetricFormValues } from '../../lib/validation/metricSchema'
import { MetricFormFields } from './MetricFormFields'
import { MetricPreview } from '../../components/metrics/MetricPreview'
import type { MetricResponse } from './MetricsList'

interface Props {
  metric: MetricResponse
  onClose: () => void
  onSaved: (metric: MetricResponse) => void
  onDeleted?: (metricId: string) => void
}

/** Map a fetched MetricResponse → initial Formik values. */
function metricToFormValues(metric: MetricResponse): MetricFormValues {
  const out: MetricFormValues = { ...initialMetricValues }
  out.kind = metric.kind
  out.key = metric.key
  out.name = metric.name
  out.description = metric.description ?? ''
  out.goal_direction = metric.goal_direction

  if (metric.kind === 'aggregation') {
    out.event_key = metric.event_key ?? ''
    out.aggregator = (metric.aggregator as MetricFormValues['aggregator']) || 'count'
    out.on_field = metric.on_field ?? ''
    out.where_clause = metric.where_clause
      ? JSON.stringify(metric.where_clause, null, 2)
      : ''
  } else if (metric.kind === 'ratio') {
    out.numerator_metric_id = metric.numerator_metric_id ?? ''
    out.denominator_metric_id = metric.denominator_metric_id ?? ''
    out.min_denominator = String(metric.min_denominator ?? 50)
  } else if (metric.kind === 'funnel') {
    out.steps =
      metric.steps && metric.steps.length > 0
        ? metric.steps.map((s) => ({
            event_key: s.event_key,
            where_clause: s.where_clause
              ? JSON.stringify(s.where_clause, null, 2)
              : '',
          }))
        : initialMetricValues.steps
    out.window_seconds = String(metric.window_seconds ?? 86400)
    out.count_repeats = metric.count_repeats ?? false
  }
  return out
}

/** Build the `UpdateMetricBody` payload for `PATCH /v1/metrics/{id}`. */
function buildUpdateBody(values: MetricFormValues, expectedVersion: number): Record<string, unknown> {
  const common: Record<string, unknown> = {
    expected_version: expectedVersion,
    name: values.name.trim(),
    description: values.description.trim() || undefined,
    goal_direction: values.goal_direction,
    kind: values.kind,
  }
  switch (values.kind) {
    case 'aggregation': {
      const wc = parseWhereClause(values.where_clause)
      const requiresField = aggregatorRequiresField(values.aggregator)
      return {
        ...common,
        event_key: values.event_key.trim(),
        aggregator: values.aggregator,
        on_field: requiresField ? (values.on_field.trim() || undefined) : undefined,
        where_clause: wc.ok ? wc.value : undefined,
      }
    }
    case 'ratio':
      return {
        ...common,
        numerator_metric_id: values.numerator_metric_id,
        denominator_metric_id: values.denominator_metric_id,
        min_denominator: Number.parseInt(values.min_denominator || '0', 10),
      }
    case 'funnel':
      return {
        ...common,
        steps: values.steps.map((s) => {
          const wc = parseWhereClause(s.where_clause)
          return {
            event_key: s.event_key.trim(),
            where_clause: wc.ok ? wc.value : undefined,
          }
        }),
        window_seconds: Number.parseInt(values.window_seconds || '0', 10),
        count_repeats: values.count_repeats,
      }
    default:
      return common
  }
}

export function EditMetricModal({ metric, onClose, onSaved, onDeleted }: Props) {
  // Fresh re-fetch — picks up updates made by other users since the list
  // page rendered. Falls back to the prop on error.
  const [loading, setLoading] = useState(false)
  const [fresh, setFresh] = useState<MetricResponse | null>(null)
  const [conflictBanner, setConflictBanner] = useState<string | null>(null)
  const [confirmingDelete, setConfirmingDelete] = useState(false)

  const resolvedMetric = fresh ?? metric

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    api
      .get<MetricResponse>(`/v1/metrics/${metric.id}`)
      .then(({ data }) => {
        if (!cancelled) setFresh(data)
      })
      .catch(() => {
        if (!cancelled) setFresh(metric)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [metric])

  async function handleSubmit(
    values: MetricFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    setConflictBanner(null)
    try {
      const body = buildUpdateBody(values, resolvedMetric.version)
      const { data } = await api.patch<MetricResponse>(
        `/v1/metrics/${resolvedMetric.id}`,
        body,
      )
      onSaved(data)
    } catch (err: unknown) {
      // Optimistic-locking conflict — the analytics service returns
      // tonic::Code::Aborted, which the gateway maps to HTTP 409.
      const status =
        typeof err === 'object' && err && 'response' in err
          ? (err as { response?: { status?: number } }).response?.status
          : undefined
      if (status === 409) {
        setConflictBanner(
          'This metric was updated elsewhere. Reload to see the latest version before saving.',
        )
        // Re-fetch so the user can see what changed.
        try {
          const { data } = await api.get<MetricResponse>(`/v1/metrics/${metric.id}`)
          setFresh(data)
        } catch {
          /* ignore secondary failure */
        }
      } else {
        setStatus({ error: extractErrorMessage(err) })
      }
    }
  }

  async function handleDelete() {
    try {
      await api.delete(`/v1/metrics/${resolvedMetric.id}`)
      if (onDeleted) onDeleted(resolvedMetric.id)
      else onSaved(resolvedMetric)
    } catch (err: unknown) {
      setConflictBanner(extractErrorMessage(err))
      setConfirmingDelete(false)
    }
  }

  const kindChip = (
    <span
      style={{
        fontSize: 10,
        fontWeight: 600,
        padding: '2px 7px',
        borderRadius: 4,
        textTransform: 'uppercase',
        letterSpacing: 0.3,
        background:
          resolvedMetric.kind === 'aggregation'
            ? 'rgba(59,130,246,0.1)'
            : resolvedMetric.kind === 'ratio'
              ? 'rgba(168,85,247,0.1)'
              : 'rgba(34,197,94,0.1)',
        color:
          resolvedMetric.kind === 'aggregation'
            ? '#2563eb'
            : resolvedMetric.kind === 'ratio'
              ? '#9333ea'
              : '#16a34a',
      }}
    >
      {resolvedMetric.kind}
    </span>
  )

  const header = (
    <div
      className="card-header"
      style={{
        padding: '16px 20px',
        borderBottom: '1px solid var(--border)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <div className="card-title">
          <I.pencil size={15} /> Edit metric
        </div>
        {kindChip}
        <span
          style={{
            fontFamily: 'var(--font-mono)',
            fontSize: 11,
            color: 'var(--fg-muted)',
          }}
        >
          v{resolvedMetric.version}
        </span>
      </div>
      <button type="button" className="icon-btn" onClick={onClose}>
        <I.x size={16} />
      </button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="lg" title={header} footer={null}>
      {loading ? (
        <div style={{ padding: 48, display: 'flex', justifyContent: 'center' }}>
          <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading…</span>
        </div>
      ) : (
        <Formik
          initialValues={metricToFormValues(resolvedMetric)}
          enableReinitialize
          validationSchema={metricSchema}
          onSubmit={handleSubmit}
        >
          {({ isSubmitting }) => (
            <Form
              // `key={metric.id}` ensures a fresh Formik instance per metric.
              key={resolvedMetric.id}
              style={{ display: 'flex', flexDirection: 'column', gap: 14 }}
            >
              <FormErrorBanner />

              {conflictBanner && (
                <div
                  role="alert"
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    padding: '10px 14px',
                    background: 'var(--warning-bg, rgba(234,179,8,0.1))',
                    border: '1px solid rgba(234,179,8,0.4)',
                    borderRadius: 6,
                    color: '#a16207',
                    fontSize: 13,
                  }}
                >
                  <I.alert size={13} style={{ flexShrink: 0 }} />
                  <span>{conflictBanner}</span>
                </div>
              )}

              <MetricFormFields
                envId={resolvedMetric.environment_id}
                keyReadOnly
                excludeMetricId={resolvedMetric.id}
              />

              {/* Metric preview — POST /v1/metrics/{id}/preview (Task 6.6). */}
              <MetricPreview metricId={resolvedMetric.id} days={7} />

              <div
                style={{
                  display: 'flex',
                  gap: 10,
                  justifyContent: 'space-between',
                  paddingTop: 4,
                }}
              >
                <div>
                  {!confirmingDelete ? (
                    <button
                      type="button"
                      className="btn"
                      style={{ color: 'var(--danger)' }}
                      onClick={() => setConfirmingDelete(true)}
                    >
                      <I.trash size={12} /> Delete
                    </button>
                  ) : (
                    <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                      <span style={{ fontSize: 12, color: 'var(--fg-muted)' }}>Are you sure?</span>
                      <button
                        type="button"
                        className="btn sm"
                        onClick={() => setConfirmingDelete(false)}
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        className="btn sm"
                        style={{ background: 'var(--danger)', color: '#fff' }}
                        onClick={handleDelete}
                      >
                        Yes, delete
                      </button>
                    </div>
                  )}
                </div>
                <div style={{ display: 'flex', gap: 10 }}>
                  <button type="button" className="btn" onClick={onClose}>
                    Cancel
                  </button>
                  <FormSubmit
                    label="Save changes"
                    loadingLabel="Saving…"
                    className={`btn primary${isSubmitting ? ' disabled' : ''}`}
                  />
                </div>
              </div>
            </Form>
          )}
        </Formik>
      )}
    </Modal>
  )
}
