import type { ReactNode } from 'react'

interface Props {
  message: string
  icon?: ReactNode
  onDismiss?: () => void
}

export function ErrorBanner({ message, icon, onDismiss }: Props) {
  return (
    <div
      className="error-banner"
      role="alert"
      style={{
        display: 'flex',
        alignItems: 'flex-start',
        gap: 8,
        padding: '12px 16px',
        background: 'var(--danger-bg)',
        border: '1px solid rgba(196,43,28,0.3)',
        borderRadius: 8,
        color: 'var(--danger)',
        fontSize: 13,
        marginBottom: 16,
      }}
    >
      {icon && <span style={{ flexShrink: 0, marginTop: 1 }}>{icon}</span>}
      <span style={{ flex: 1 }}>{message}</span>
      {onDismiss && (
        <button
          onClick={onDismiss}
          aria-label="Dismiss error"
          style={{
            flexShrink: 0,
            background: 'none',
            border: 'none',
            cursor: 'pointer',
            color: 'inherit',
            padding: '0 2px',
            lineHeight: 1,
            opacity: 0.7,
          }}
        >
          ✕
        </button>
      )}
    </div>
  )
}
