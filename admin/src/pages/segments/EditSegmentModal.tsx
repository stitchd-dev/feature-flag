import { useState, useEffect } from 'react'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { api } from '../../lib/api'
import type { Segment } from './types'
import { isListSegment } from './helpers'

interface Props {
  segment: Segment
  onClose: () => void
  onSaved: (segment: Segment) => void
}

export function EditSegmentModal({ segment, onClose, onSaved }: Props) {
  const [name, setName] = useState(segment.name)
  const [description, setDescription] = useState(segment.description ?? '')
  const [tagsInput, setTagsInput] = useState(segment.tags.join(', '))
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [nameError, setNameError] = useState(false)
  const [fresh, setFresh] = useState<Segment | null>(null)
  const [contextType, setContextType] = useState(segment.context_type ?? 'user')

  const resolvedFresh = fresh ?? segment
  const isListType = isListSegment(resolvedFresh)

  useEffect(() => {
    setLoading(true) // eslint-disable-line react-hooks/set-state-in-effect
    api.get<Segment>(`/v1/segments/${segment.id}`)
      .then(({ data }) => {
        setFresh(data)
        setName(data.name)
        setDescription(data.description ?? '')
        setTagsInput(data.tags.join(', '))
        setContextType(data.context_type ?? 'user')
      })
      .catch(() => {
        setFresh(segment)
      })
      .finally(() => setLoading(false))
  }, [segment])

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    if (!name.trim()) {
      setNameError(true)
      setError('Name is required')
      return
    }
    setNameError(false)
    setError(null)

    const tags = tagsInput
      .split(',')
      .map((t) => t.trim())
      .filter((t) => t.length > 0)

    setSaving(true)
    try {
      const body = {
        name: name.trim(),
        description: description.trim() || undefined,
        tags,
        condition_expr: (fresh ?? segment).condition_expr,
        user_list: [],
        context_type: isListType ? contextType : undefined,
      }
      const { data } = await api.put<Segment>(`/v1/segments/${segment.id}`, body)
      onSaved(data)
    } catch (err: unknown) {
      const e = err as { response?: { data?: { message?: string } }; message?: string }
      setError(e?.response?.data?.message ?? e?.message ?? 'Failed to update segment')
    } finally {
      setSaving(false)
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
      <button className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose}>Cancel</button>
      <button type="submit" className="btn primary" form="edit-segment-form" disabled={saving}>
        {saving ? 'Saving…' : <><I.check size={13} /> Save changes</>}
      </button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
      {loading ? (
        <div style={{ padding: 48, display: 'flex', justifyContent: 'center' }}>
          <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading…</span>
        </div>
      ) : (
        <form id="edit-segment-form" onSubmit={submit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
          {error && (
            <div style={{ padding: '10px 14px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 6, color: 'var(--danger)', fontSize: 13 }}>
              <I.alert size={13} style={{ verticalAlign: 'middle', marginRight: 6 }} />{error}
            </div>
          )}

          <div>
            <label className="label">
              Name <span style={{ color: 'var(--danger)' }}>*</span>
            </label>
            <input
              className="input"
              style={{ width: '100%', borderColor: nameError ? 'var(--danger)' : undefined }}
              placeholder="e.g. Beta Users"
              value={name}
              onChange={(e) => { setName(e.target.value); if (e.target.value.trim()) setNameError(false) }}
              autoFocus
            />
          </div>

          <div>
            <label className="label">Description</label>
            <textarea
              className="input"
              style={{ width: '100%', minHeight: 64, resize: 'vertical' }}
              placeholder="Optional description of what this segment represents"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>

          <div>
            <label className="label">Tags</label>
            <input
              className="input"
              style={{ width: '100%' }}
              placeholder="Comma-separated tags, e.g. beta, internal, us-only"
              value={tagsInput}
              onChange={(e) => setTagsInput(e.target.value)}
            />
            <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
              Separate tags with commas.
            </div>
          </div>

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
        </form>
      )}
    </Modal>
  )
}
