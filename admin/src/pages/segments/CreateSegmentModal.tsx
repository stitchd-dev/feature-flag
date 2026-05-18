import { useState } from 'react'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import type { Segment, SegmentType } from './types'

interface Props {
  envId: string
  orgId: string
  projectId: string
  onClose: () => void
  onCreated: (segment: Segment) => void
}

const TYPE_OPTIONS: { value: SegmentType; label: string; icon: keyof typeof I; description: string }[] = [
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
  const [contextType, setContextType] = useState('user')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [tagsInput, setTagsInput] = useState('')
  const [userListInput, setUserListInput] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [nameError, setNameError] = useState(false)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    if (!segmentType) return
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

    const user_list =
      segmentType === 'list'
        ? userListInput.split('\n').map((u) => u.trim()).filter((u) => u.length > 0)
        : []

    setSaving(true)
    try {
      const body = {
        name: name.trim(),
        description: description.trim() || undefined,
        tags,
        segment_type: segmentType,
        context_type: segmentType === 'list' ? contextType : undefined,
        condition_expr: null,
        user_list,
        env_id: envId,
        org_id: orgId,
        project_id: projectId,
      }
      const { data } = await api.post<Segment>('/v1/segments', body)
      onCreated(data)
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  const header = (
    <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
      <div className="card-title"><I.segment size={15} /> New segment</div>
      <button className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose}>Cancel</button>
      <button type="submit" className="btn primary" form="create-segment-form" disabled={saving || segmentType === null}>
        {saving ? 'Creating…' : <><I.plus size={13} /> Create segment</>}
      </button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
      <form id="create-segment-form" onSubmit={submit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
        {error && (
          <div style={{ padding: '10px 14px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 6, color: 'var(--danger)', fontSize: 13 }}>
            <I.alert size={13} style={{ verticalAlign: 'middle', marginRight: 6 }} />{error}
          </div>
        )}

        {/* ── Segment type selector ── */}
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
                  onClick={() => setSegmentType(opt.value)}
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

        {/* ── Common fields (shown once type is chosen) ── */}
        {segmentType !== null && (
          <>
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

            {/* ── Type-specific section ── */}
            {segmentType === 'list' ? (
              <>
                <div>
                  <label className="label">
                    Context Type <span style={{ color: 'var(--danger)' }}>*</span>
                  </label>
                  <input
                    className="input"
                    style={{ width: '100%' }}
                    placeholder="e.g. user, org, device"
                    value={contextType}
                    onChange={(e) => setContextType(e.target.value)}
                  />
                  <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
                    The type of entity this list targets (e.g. <code>user</code>, <code>org</code>, <code>device</code>).
                  </div>
                </div>
                <div>
                  <label className="label">Keys</label>
                  <textarea
                    className="input"
                    style={{ width: '100%', minHeight: 100, resize: 'vertical', fontFamily: 'var(--font-mono)', fontSize: 12 }}
                    placeholder={"key-1\nkey-2\nkey-3"}
                    value={userListInput}
                    onChange={(e) => setUserListInput(e.target.value)}
                    rows={5}
                  />
                  <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
                    One <code>{contextType || 'context'}</code> key per line — these will always match this segment.
                  </div>
                </div>
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
      </form>
    </Modal>
  )
}
