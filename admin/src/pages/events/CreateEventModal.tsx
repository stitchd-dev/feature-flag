import { Formik, Form } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormField } from '../../components/form/FormField'
import { FormSelect } from '../../components/form/FormSelect'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import { eventDefinitionSchema, METRIC_TYPES } from '../../lib/validation/eventDefinitionSchema'
import type { EventDefinitionFormValues } from '../../lib/validation/eventDefinitionSchema'

const METRIC_TYPE_OPTIONS = METRIC_TYPES.map((t) => ({
  value: t,
  label: t.charAt(0).toUpperCase() + t.slice(1),
}))

interface EventDefinitionResponse {
  event_key: string
  metric_type: string
  description: string
  schema: string | null
  created_at: string
}

interface Props {
  onClose: () => void
  onCreated: (event: EventDefinitionResponse) => void
}

export function CreateEventModal({ onClose, onCreated }: Props) {
  const initialValues: EventDefinitionFormValues = {
    key: '',
    metric_type: 'count',
    description: '',
    schema: '',
  }

  async function handleSubmit(
    values: EventDefinitionFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    try {
      const body = {
        event_key: values.key.trim(),
        metric_type: values.metric_type,
        description: values.description?.trim() || undefined,
        schema: values.schema?.trim() ? JSON.parse(values.schema.trim()) : null,
      }
      const { data } = await api.post<EventDefinitionResponse>('/v1/events', body)
      onCreated(data)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  const header = (
    <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
      <div className="card-title"><I.zap size={15} /> Register event</div>
      <button type="button" className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose}>Cancel</button>
      <FormSubmit label="Register event" loadingLabel="Registering…" form="create-event-form" />
    </div>
  )

  return (
    <Formik
      initialValues={initialValues}
      validationSchema={eventDefinitionSchema}
      onSubmit={handleSubmit}
    >
      <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
        <Form id="create-event-form" style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
          <FormErrorBanner />

          <FormField
            name="key"
            label="Event key"
            placeholder="e.g. checkout.completed"
            hint="Used in SDK tracking calls. Immutable after registration."
            style={{ fontFamily: 'var(--font-mono)' }}
            autoFocus
          />

          <FormSelect
            name="metric_type"
            label="Metric type"
            options={METRIC_TYPE_OPTIONS}
            hint="How this event is aggregated in experiments."
          />

          <FormTextarea
            name="description"
            label="Description"
            placeholder="What this event represents"
            style={{ minHeight: 64 }}
          />

          <FormTextarea
            name="schema"
            label="JSON Schema (optional)"
            placeholder='{"type": "object", "properties": {...}}'
            hint="Optional JSON schema for payload validation."
            style={{ fontFamily: 'var(--font-mono)', fontSize: 12, minHeight: 80 }}
          />
        </Form>
      </Modal>
    </Formik>
  )
}
