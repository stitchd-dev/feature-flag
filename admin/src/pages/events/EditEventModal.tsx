import { Formik, Form } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormSelect } from '../../components/form/FormSelect'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import {
  METRIC_TYPES,
  eventDefinitionEditSchema,
} from '../../lib/validation/eventDefinitionSchema'
import type { EventDefinitionEditValues } from '../../lib/validation/eventDefinitionSchema'

const METRIC_TYPE_OPTIONS = METRIC_TYPES.map((t) => ({
  value: t,
  label: t.charAt(0).toUpperCase() + t.slice(1),
}))

/**
 * Shape of an event definition as returned by the admin API. The `version`
 * field carries the row's optimistic-locking version which the PATCH call
 * must echo back so the server can detect concurrent edits.
 */
export interface EventDefinition {
  event_key: string
  /// Environment ID — wire-required for the `?env_id=` query param on
  /// PATCH/DELETE since the gateway needs to scope key→id resolution.
  environment_id?: string
  metric_type: string
  description: string
  schema: string | null
  version: number
  created_at: string
  updated_at?: string
}

interface Props {
  /** The event being edited — supplies key (read-only) + initial values. */
  event: EventDefinition
  onClose: () => void
  /** Fired after a successful PATCH with the server's updated row. */
  onSaved: (updated: EventDefinition) => void
}

/**
 * Edit modal for an existing `event_definition`. The `event_key` is
 * immutable (rendered as a disabled field showing the key for context);
 * editable fields are `metric_type`, `description`, and `schema`.
 *
 * Carries `version` through to the PATCH payload so the gateway can return
 * HTTP 409 on a stale update; the 409 surface is rendered as a banner via
 * `setStatus({ error })`, matching the existing pattern in `FlagDetail`.
 */
export function EditEventModal({ event, onClose, onSaved }: Props) {
  const initialValues: EventDefinitionEditValues = {
    metric_type: event.metric_type,
    description: event.description ?? '',
    schema: event.schema ?? '',
  }

  async function handleSubmit(
    values: EventDefinitionEditValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    try {
      const body = {
        metric_type: values.metric_type,
        description: values.description?.trim() || undefined,
        schema: values.schema?.trim() ? JSON.parse(values.schema.trim()) : null,
        expected_version: event.version,
      }
      const { data } = await api.patch<EventDefinition>(
        `/v1/events/${encodeURIComponent(event.event_key)}?env_id=${event.environment_id ?? ''}`,
        body,
      )
      onSaved(data)
    } catch (err: unknown) {
      // The gateway returns 409 for stale-version conflicts; the underlying
      // error message is surfaced as-is here. `extractErrorMessage` already
      // unwraps the axios payload shape so the banner reads cleanly.
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
        <I.zap size={15} /> Edit event
      </div>
      <button type="button" className="icon-btn" onClick={onClose}>
        <I.x size={16} />
      </button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose}>
        Cancel
      </button>
      <FormSubmit label="Save changes" loadingLabel="Saving…" form="edit-event-form" />
    </div>
  )

  return (
    <Formik
      initialValues={initialValues}
      validationSchema={eventDefinitionEditSchema}
      enableReinitialize
      onSubmit={handleSubmit}
    >
      <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
        <Form id="edit-event-form" style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
          <FormErrorBanner />

          {/* Read-only key field — anchors the user's mental model but cannot be changed. */}
          <div>
            <label className="label" htmlFor="edit-event-key-readonly">
              Event key
            </label>
            <input
              id="edit-event-key-readonly"
              className="input"
              value={event.event_key}
              disabled
              readOnly
              data-testid="edit-event-key-readonly"
              style={{ fontFamily: 'var(--font-mono)', opacity: 0.7 }}
            />
            <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
              Immutable after registration.
            </div>
          </div>

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
