import { I } from './icons'

interface Props {
  title: string
  message: string
  confirmLabel?: string
  confirmDanger?: boolean
  onConfirm: () => void
  onCancel: () => void
}

export function ConfirmDialog({ title, message, confirmLabel = 'Confirm', confirmDanger = false, onConfirm, onCancel }: Props) {
  return (
    <div style={{ position: 'fixed', inset: 0, zIndex: 200, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
      <div style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }} onClick={onCancel} />
      <div className="card" style={{ position: 'relative', width: 400, zIndex: 1, padding: 24 }}>
        <div style={{ display: 'flex', alignItems: 'flex-start', gap: 12, marginBottom: 12 }}>
          <div style={{ padding: 8, background: confirmDanger ? 'var(--danger-bg)' : 'var(--bg-sunken)', borderRadius: 8 }}>
            {confirmDanger
              ? <I.alert size={18} stroke="var(--danger)" />
              : <I.info size={18} stroke="var(--fg-muted)" />
            }
          </div>
          <div>
            <div style={{ fontSize: 15, fontWeight: 600, marginBottom: 4 }}>{title}</div>
            <div style={{ fontSize: 13, color: 'var(--fg-muted)', lineHeight: 1.5 }}>{message}</div>
          </div>
        </div>
        <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 8 }}>
          <button className="btn" onClick={onCancel}>Cancel</button>
          <button
            className={`btn ${confirmDanger ? 'danger' : 'primary'}`}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  )
}
