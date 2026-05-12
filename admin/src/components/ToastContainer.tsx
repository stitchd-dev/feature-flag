import { useEffect } from 'react'
import { I } from './icons'
import type { ToastItem } from '../hooks/useToast'

const KIND_STYLES: Record<ToastItem['kind'], { bg: string; border: string; color: string; icon: keyof typeof import('./icons').I }> = {
  success: { bg: 'var(--success-bg)', border: 'rgba(17,122,61,0.3)', color: 'var(--success)', icon: 'check' },
  error: { bg: 'var(--danger-bg)', border: 'rgba(196,43,28,0.3)', color: 'var(--danger)', icon: 'alert' },
  info: { bg: 'var(--surface)', border: 'var(--border)', color: 'var(--fg)', icon: 'info' },
}

function SingleToast({ item, onDismiss }: { item: ToastItem; onDismiss: (id: string) => void }) {
  const s = KIND_STYLES[item.kind]

  useEffect(() => {
    const t = setTimeout(() => onDismiss(item.id), 4000)
    return () => clearTimeout(t)
  }, [item.id, onDismiss])

  const Icon = I[s.icon]

  return (
    <div style={{
      padding: '10px 14px', borderRadius: 8, fontSize: 13, maxWidth: 380, width: '100%',
      background: s.bg, border: `1px solid ${s.border}`, color: s.color,
      boxShadow: 'var(--shadow-md)',
      display: 'flex', alignItems: 'center', gap: 8,
      animation: 'slideUp 0.15s ease',
    }}>
      <Icon size={14} />
      <span style={{ flex: 1 }}>{item.message}</span>
      {item.action && (
        <button
          className="btn sm"
          style={{ borderColor: s.color, color: s.color, flexShrink: 0 }}
          onClick={() => { item.action!.onClick(); onDismiss(item.id) }}
        >
          {item.action.label}
        </button>
      )}
      <button className="icon-btn" style={{ color: s.color, flexShrink: 0 }} onClick={() => onDismiss(item.id)}>
        <I.x size={12} />
      </button>
    </div>
  )
}

interface Props {
  toasts: ToastItem[]
  onDismiss: (id: string) => void
}

export function ToastContainer({ toasts, onDismiss }: Props) {
  if (toasts.length === 0) return null
  return (
    <div style={{
      position: 'fixed', bottom: 72, right: 20, zIndex: 300,
      display: 'flex', flexDirection: 'column', gap: 8,
      pointerEvents: 'none',
    }}>
      {toasts.map((t) => (
        <div key={t.id} style={{ pointerEvents: 'auto' }}>
          <SingleToast item={t} onDismiss={onDismiss} />
        </div>
      ))}
    </div>
  )
}
