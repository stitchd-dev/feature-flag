import { Formik, Form } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import {
  metricCreateSchema,
  initialMetricValues,
  parseWhereClause,
} from '../../lib/validation/metricSchema'
import type { MetricFormValues } from '../../lib/validation/metricSchema'
import { MetricFormFields } from './MetricFormFields'
import type { MetricResponse } from './MetricsList'

interface Props {
  envId: string
  onClose: () => void
  onCreated: (metric: MetricResponse) => void
}

/**
 * Map `MetricFormValues` → `CreateMetricBody` (gateway) for `POST /v1/metrics`.
 * Mirrors `buildMetricRequestBody` in MetricsList.test.ts (kept in sync).
 */
function buildCreateBody(values: MetricFormValues, envId: string): Record<string, unknown> {
  const common: Record<string, unknown> = {
    environment_id: envId,
    key: values.key.trim(),
    name: values.name.trim(),
    description: values.description.trim() || undefined,
    goal_direction: values.goal_direction,
    kind: values.kind,
  }
  switch (values.kind) {
    case 'aggregation': {
      const wc = parseWhereClause(values.where_clause)
      return {
        ...common,
        event_key: values.event_key.trim(),
        aggregator: values.aggregator,
        // on_field is optional — send it when non-empty; absent = canonical value columns.
        on_field: values.on_field.trim() || undefined,
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

export function CreateMetricModal({ envId, onClose, onCreated }: Props) {
  async function handleSubmit(
    values: MetricFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    if (!values.kind) {
      setStatus({ error: 'Choose a metric kind first.' })
      return
    }
    try {
      const body = buildCreateBody(values, envId)
      const { data } = await api.post<MetricResponse>('/v1/metrics', body)
      onCreated(data)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

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
      <div className="card-title">
        <I.metric size={15} /> New metric
      </div>
      <button type="button" className="icon-btn" onClick={onClose}>
        <I.x size={16} />
      </button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="lg" title={header} footer={null}>
      <Formik
        initialValues={initialMetricValues}
        validationSchema={metricCreateSchema}
        onSubmit={handleSubmit}
      >
        {({ isSubmitting, values }) => (
          // `key={values.kind}` resets Formik state when the kind changes
          // — required to flush stale errors from fields irrelevant to the
          // newly selected kind. Matches the pattern noted in
          // `conductor/patterns.md`.
          <Form
            key={values.kind || 'empty'}
            style={{ display: 'flex', flexDirection: 'column', gap: 14 }}
          >
            <FormErrorBanner />
            <MetricFormFields envId={envId} />

            <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
              <button type="button" className="btn" onClick={onClose}>
                Cancel
              </button>
              <FormSubmit
                label="Create metric"
                loadingLabel="Creating…"
                className={`btn primary${!values.kind || isSubmitting ? ' disabled' : ''}`}
              />
            </div>
          </Form>
        )}
      </Formik>
    </Modal>
  )
}
