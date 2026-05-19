import { useState } from 'react'
import { Formik, Form } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormField } from '../../components/form/FormField'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import { segmentSchema } from '../../lib/validation/segmentSchema'
import type { SegmentFormValues } from '../../lib/validation/segmentSchema'
import type { Segment, SegmentType } from './types'

interface Props {
  envId: string
  orgId: string
  projectId: string
  onClose: () => void
  onCreated: (segment: Segment) => void
}

const TYPE_OPTIONS: { value: SegmentType; label: string; icon: 'users' | 'flag'; description: string }[] = [
  {
    value: 'list',
    label: 'List-based',
    icon: 'users',
    description: 'Match users by an explicit list of user keys.',
  },
  {
    value: 'rule',
    label: 'Rule-based',
    icon: 'flag',
    description: 'Match users using attribute conditions and rules.',
  },
]

export function CreateSegmentModal({ envId, orgId, projectId, onClose, onCreated }: Props) {
  const [segmentType, setSegmentType] = useState<SegmentType | null>(null)

  const initialValues: SegmentFormValues = {
    name: '',
    segment_type: '',
    description: '',
    tags: '',
    context_type: 'user',
    user_list: '',
  }

  async function handleSubmit(
    values: SegmentFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    if (!segmentType) return

    const tags = (values.tags ?? '')
      .split(',')
      .map((t) => t.trim())
      .filter((t) => t.length > 0)

    const user_list =
      segmentType === 'list'
        ? (values.user_list ?? '').split('\n').map((u) => u.trim()).filter((u) => u.length > 0)
        : []

    try {
      const body = {
        name: values.name.trim(),
        description: values.description?.trim() || undefined,
        tags,
        segment_type: segmentType,
        context_type: segmentType === 'list' ? (values.context_type ?? 'user') : undefined,
        condition_expr: null,
        user_list,
        env_id: envId,
        org_id: orgId,
        project_id: projectId,
      }
      const { data } = await api.post<Segment>('/v1/segments', body)
      onCreated(data)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  const header = (
    <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
      <div className="card-title"><I.segment size={15} /> New segment</div>
      <button type="button" className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  // Build yup schema that also validates segment_type required when submitted
  const schema = segmentType
    ? segmentSchema.shape({
        segment_type: segmentSchema.fields.segment_type,
      })
    : segmentSchema

  return (
    <Modal isOpen onClose={onClose} size="md" title={header} footer={null}>
      <Formik
        initialValues={initialValues}
        validationSchema={schema}
        onSubmit={handleSubmit}
      >
        {({ isSubmitting, setFieldValue }) => (
          <Form style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
            <FormErrorBanner />

            {/* Segment type selector */}
            <div>
              <label className="label">
                Segment type <span style={{ color: 'var(--danger)' }}>*</span>
              </label>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10, marginTop: 6 }}>
                {TYPE_OPTIONS.map((opt) => {
                  const Ic = I[opt.icon] as React.FC<{ size?: number; style?: React.CSSProperties }>
                  const selected = segmentType === opt.value
                  return (
                    <button
                      key={opt.value}
                      type="button"
                      onClick={() => {
                        setSegmentType(opt.value)
                        // Sync React-state choice into Formik values so the Yup
                        // segment_type.oneOf(['list','rule']) validator passes.
                        // Without this the submit fails silently because
                        // values.segment_type stays '' (initial).
                        void setFieldValue('segment_type', opt.value)
                      }}
                      style={{
                        display: 'flex',
                        flexDirection: 'column',
                        alignItems: 'flex-start',
                        gap: 6,
                        padding: '12px 14px',
                        border: `2px solid ${selected ? 'var(--primary)' : 'var(--border)'}`,
                        borderRadius: 8,
                        background: selected ? 'var(--primary-bg, rgba(229,79,53,0.06))' : 'var(--surface)',
                        cursor: 'pointer',
                        textAlign: 'left',
                        transition: 'border-color 0.15s, background 0.15s',
                      }}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: 6, fontWeight: 600, fontSize: 13, color: selected ? 'var(--primary)' : 'var(--fg)' }}>
                        <Ic size={14} />
                        {opt.label}
                      </div>
                      <div style={{ fontSize: 11, color: 'var(--fg-muted)', lineHeight: 1.4 }}>
                        {opt.description}
                      </div>
                    </button>
                  )
                })}
              </div>
            </div>

            {/* Common fields — shown once type chosen */}
            {segmentType !== null && (
              <>
                <FormField
                  name="name"
                  label="Name"
                  placeholder="e.g. Beta Users"
                  autoFocus
                />

                <FormTextarea
                  name="description"
                  label="Description"
                  placeholder="Optional description of what this segment represents"
                  style={{ minHeight: 64 }}
                />

                <FormField
                  name="tags"
                  label="Tags"
                  placeholder="Comma-separated tags, e.g. beta, internal, us-only"
                  hint="Separate tags with commas."
                />

                {segmentType === 'list' ? (
                  <>
                    <FormField
                      name="context_type"
                      label="Context Type"
                      placeholder="e.g. user, org, device"
                      hint="The type of entity this list targets (e.g. user, org, device)."
                    />
                    <FormTextarea
                      name="user_list"
                      label="Keys"
                      placeholder={"key-1\nkey-2\nkey-3"}
                      rows={5}
                      style={{ fontFamily: 'var(--font-mono)', fontSize: 12, minHeight: 100 }}
                    />
                  </>
                ) : (
                  <div style={{ padding: '12px 14px', background: 'var(--bg-sunken)', borderRadius: 8, fontSize: 12, color: 'var(--fg-muted)', display: 'flex', gap: 8, alignItems: 'flex-start' }}>
                    <I.info size={13} style={{ marginTop: 1, flexShrink: 0 }} />
                    <span>
                      Targeting rules can be configured after creation from the segment detail page.
                      You can combine attribute conditions using AND / OR logic.
                    </span>
                  </div>
                )}
              </>
            )}

            <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
              <button type="button" className="btn" onClick={onClose}>Cancel</button>
              <FormSubmit
                label="Create segment"
                loadingLabel="Creating…"
                className={`btn primary${isSubmitting || segmentType === null ? ' disabled' : ''}`}
              />
            </div>
          </Form>
        )}
      </Formik>
    </Modal>
  )
}
