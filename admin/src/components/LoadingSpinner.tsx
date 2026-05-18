import type { ReactNode } from 'react'

interface Props {
  size?: 'sm' | 'md' | 'lg'
  label?: string
}

const SIZES: Record<NonNullable<Props['size']>, { px: number; font: number }> = {
  sm: { px: 16, font: 12 },
  md: { px: 24, font: 14 },
  lg: { px: 36, font: 15 },
}

export function LoadingSpinner({ size = 'md', label = 'Loading…' }: Props): ReactNode {
  const { px, font } = SIZES[size]
  return (
    <div
      className="loading-spinner"
      style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 10, color: 'var(--fg-muted)', fontSize: font }}
      aria-label={label}
      role="status"
    >
      <svg
        width={px}
        height={px}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        aria-hidden="true"
        style={{ animation: 'spin 0.8s linear infinite' }}
      >
        <style>{`@keyframes spin { to { transform: rotate(360deg) } }`}</style>
        <circle cx="12" cy="12" r="10" strokeOpacity="0.25" />
        <path d="M12 2a10 10 0 0 1 10 10" />
      </svg>
      <span>{label}</span>
    </div>
  )
}
