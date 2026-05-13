import { useState, useEffect } from 'react'
import { I } from '../../components/icons'
import { api } from '../../lib/api'
import type { Segment } from './types'

interface Props {
  segment: Segment
  onClose: () => void
  onSaved: (segment: Segment) => void
}

export function EditSegmentModal({ segment, onClose, onSaved }: Props) {
  const [name, setName] = useState(segment.name)
  const [description, setDescription] = useState(segment.description ?? '')
  const [tagsInput, setTagsInput] = useState(segment.tags.join(', '))
  const [userListInput, setUserListInput] = useState(segment.user_list.join('\n'))
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [nameError, setNameError] = useState(false)
  const [fresh, setFresh] = useState<Segment | null>(null)

  // Fetch latest data for the segment
  useEffect(() => {
    setLoading(true) // eslint-disable-line react-hooks/set-state-in-effect
    api.get<Segment>(`/v1/segments/${segment.id}`)
      .then(({ data }) => {
        setFresh(data)
        setName(data.name)
        setDescription(data.description ?? '')
        setTagsInput(data.tags.join(', '))
        setUserListInput(data.user_list.join('\n'))
      })
      .catch(() => {
        // Use the passed segment data if fetch fails
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

    const user_list = userListInput
      .split('\n')
      .map((u) => u.trim())
      .filter((u) => u.length > 0)

    setSaving(true)
    try {
      const body = {
        name: name.trim(),
        description: description.trim() || undefined,
        tags,
        condition_expr: (fresh ?? segment).condition_expr,
        user_list,
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

  return (
    <div style={{ position: 'fixed', inset: 0, zIndex: 200, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
      <div style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }} onClick={onClose} />
      <div className="card" style={{ position: 'relative', width: 520, maxHeight: '90vh', overflow: 'auto', zIndex: 1, padding: 0 }}>
        <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)' }}>
          <div className="card-title"><I.pencil size={15} /> Edit segment</div>
          <button className="icon-btn" onClick={onClose}><I.x size={16} /></button>
        </div>

        {loading ? (
          <div style={{ padding: 48, display: 'flex', justifyContent: 'center' }}>
            <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading…</span>
          </div>
        ) : (
          <form onSubmit={submit} style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 16 }}>
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

            <div>
              <label className="label">User List (optional)</label>
              <textarea
                className="input"
                style={{ width: '100%', minHeight: 80, resize: 'vertical', fontFamily: 'var(--font-mono)', fontSize: 12 }}
                placeholder={"user-key-1\nuser-key-2\nuser-key-3"}
                value={userListInput}
                onChange={(e) => setUserListInput(e.target.value)}
                rows={5}
              />
              <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
                Enter one user key per line. These users will always match this segment.
              </div>
            </div>

            <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
              <button type="button" className="btn" onClick={onClose}>Cancel</button>
              <button type="submit" className="btn primary" disabled={saving}>
                {saving ? 'Saving…' : <><I.check size={13} /> Save changes</>}
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  )
}
