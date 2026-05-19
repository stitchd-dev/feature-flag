import { useState, useEffect } from 'react'
import { Formik, Form } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormField } from '../../components/form/FormField'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import type { Segment } from './types'
import { isListSegment } from './helpers'

interface Props {
  segment: Segment
  onClose: () => void
  onSaved: (segment: Segment) => void
}

interface EditFormValues {
  name: string
  description: string
  tags: string
  context_type: string
}

export function EditSegmentModal({ segment, onClose, onSaved }: Props) {
  const [loading, setLoading] = useState(false)
  const [fresh, setFresh] = useState<Segment | null>(null)

  const resolvedFresh = fresh ?? segment
  const isListType = isListSegment(resolvedFresh)

  useEffect(() => {
    setLoading(true)
    api.get<Segment>(`/v1/segments/${segment.id}`)
      .then(({ data }) => { setFresh(data) })
      .catch(() => { setFresh(segment) })
      .finally(() => setLoading(false))
  }, [segment])

  const initialValues: EditFormValues = {
    name: resolvedFresh.name,
    description: resolvedFresh.description ?? '',
    tags: resolvedFresh.tags.join(', '),
    context_type: resolvedFresh.context_type ?? 'user',
  }

  async function handleSubmit(
    values: EditFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    if (!values.name.trim()) {
      setStatus({ error: 'Name is required' })
      return
    }
    const tags = values.tags
      .split(',')
      .map((t) => t.trim())
      .filter((t) => t.length > 0)

    try {
      const body = {
        name: values.name.trim(),
        description: values.description.trim() || undefined,
        tags,
        condition_expr: (fresh ?? segment).condition_expr,
        user_list: [],
        context_type: isListType ? values.context_type : undefined,
      }
      const { data } = await api.put<Segment>(`/v1/segments/${segment.id}`, body)
      onSaved(data)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  const typeBadge = (
    <span style={{
      fontSize: 11,
      fontWeight: 600,
      padding: '2px 8px',
      borderRadius: 4,
      background: isListType ? 'rgba(22,163,74,0.1)' : 'rgba(59,130,246,0.1)',
      color: isListType ? '#16a34a' : '#2563eb',
      border: `1px solid ${isListType ? 'rgba(22,163,74,0.25)' : 'rgba(59,130,246,0.25)'}`,
    }}>
      {isListType ? 'List-based' : 'Rule-based'}
    </span>
  )

  const header = (
    <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <div className="card-title"><I.pencil size={15} /> Edit segment</div>
        {typeBadge}
      </div>
      <button type="button" className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="md" title={header} footer={null}>
      {loading ? (
        <div style={{ padding: 48, display: 'flex', justifyContent: 'center' }}>
          <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading…</span>
        </div>
      ) : (
        <Formik
          initialValues={initialValues}
          enableReinitialize
          onSubmit={handleSubmit}
        >
          <Form id="edit-segment-form" style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
            <FormErrorBanner />

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

            {isListType ? (
              <div style={{ padding: '12px 14px', background: 'var(--bg-sunken)', borderRadius: 8, fontSize: 12, color: 'var(--fg-muted)', display: 'flex', gap: 8, alignItems: 'flex-start' }}>
                <I.info size={13} style={{ marginTop: 1, flexShrink: 0, color: 'var(--info)' }} />
                <span>
                  Include / exclude keys and CSV imports are managed from the{' '}
                  <strong style={{ color: 'var(--fg)' }}>segment detail page</strong>.
                  Save name/description/tags changes here.
                </span>
              </div>
            ) : (
              <div style={{ padding: '12px 14px', background: 'var(--bg-sunken)', borderRadius: 8, fontSize: 12, color: 'var(--fg-muted)', display: 'flex', gap: 8, alignItems: 'flex-start' }}>
                <I.info size={13} style={{ marginTop: 1, flexShrink: 0 }} />
                <span>
                  Targeting rules are managed from the segment detail page where you can build
                  AND / OR condition trees.
                </span>
              </div>
            )}

            <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
              <button type="button" className="btn" onClick={onClose}>Cancel</button>
              <FormSubmit label="Save changes" loadingLabel="Saving…" />
            </div>
          </Form>
        </Formik>
      )}
    </Modal>
  )
}
