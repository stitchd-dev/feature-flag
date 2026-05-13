import { useState, useEffect } from 'react'
import { I } from '../../components/icons'
import { api } from '../../lib/api'
import type { Segment } from './types'

interface SegmentDetail extends Segment {
  flag_count?: number
}

interface Props {
  segment: Segment
  onClose: () => void
  onDeleted: () => void
}

export function DeleteSegmentModal({ segment, onClose, onDeleted }: Props) {
  const [detail, setDetail] = useState<SegmentDetail | null>(null)
  const [deleting, setDeleting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    api.get<SegmentDetail>(`/v1/segments/${segment.id}`)
      .then(({ data }) => setDetail(data))
      .catch(() => setDetail(null))
  }, [segment.id])

  async function handleDelete() {
    setDeleting(true)
    setError(null)
    try {
      await api.delete(`/v1/segments/${segment.id}`)
      onDeleted()
    } catch (err: unknown) {
      const e = err as { response?: { data?: { message?: string } }; message?: string }
      setError(e?.response?.data?.message ?? e?.message ?? 'Failed to delete segment')
      setDeleting(false)
    }
  }

  const flagCount = detail?.flag_count

  return (
    <div style={{ position: 'fixed', inset: 0, zIndex: 200, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
      <div style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }} onClick={onClose} />
      <div className="card" style={{ position: 'relative', width: 440, zIndex: 1, padding: 0 }}>
        <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)' }}>
          <div className="card-title" style={{ color: 'var(--danger)' }}>
            <I.alert size={15} /> Delete segment
          </div>
          <button className="icon-btn" onClick={onClose}><I.x size={16} /></button>
        </div>

        <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 14 }}>
          {error && (
            <div style={{ padding: '10px 14px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 6, color: 'var(--danger)', fontSize: 13 }}>
              <I.alert size={13} style={{ verticalAlign: 'middle', marginRight: 6 }} />{error}
            </div>
          )}

          <div style={{ fontSize: 14, lineHeight: 1.6 }}>
            Are you sure you want to delete segment{' '}
            <strong style={{ fontFamily: 'var(--font-mono)' }}>{segment.name}</strong>?
          </div>

          {flagCount !== undefined && flagCount > 0 && (
            <div style={{ padding: '10px 12px', background: 'var(--warning-bg)', border: '1px solid rgba(184,118,26,0.35)', borderRadius: 6, fontSize: 13 }}>
              <I.alert size={13} style={{ verticalAlign: 'middle', marginRight: 6 }} />
              This segment is used by <strong>{flagCount}</strong> flag{flagCount !== 1 ? 's' : ''}.
              Deleting it may affect targeting rules.
            </div>
          )}

          {flagCount === 0 && (
            <div style={{ padding: '10px 12px', background: 'var(--bg-sunken)', borderRadius: 6, fontSize: 13, color: 'var(--fg-muted)' }}>
              This segment is not currently used by any flags.
            </div>
          )}

          <div style={{ fontSize: 13, color: 'var(--fg-muted)' }}>
            This action cannot be undone.
          </div>

          <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
            <button
              type="button"
              className="btn"
              style={{ background: 'var(--danger)', color: 'white', borderColor: 'var(--danger)' }}
              disabled={deleting}
              onClick={handleDelete}
            >
              {deleting ? 'Deleting…' : <><I.trash size={13} /> Delete segment</>}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
