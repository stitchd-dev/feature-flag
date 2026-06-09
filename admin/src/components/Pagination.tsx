import { I } from './icons'

interface PaginationProps {
  hasPrev: boolean
  hasNext: boolean
  onPrev: () => void
  onNext: () => void
}

export function Pagination({ hasPrev, hasNext, onPrev, onNext }: PaginationProps) {
  // Nothing to navigate to in either direction — hide the control entirely.
  if (!hasPrev && !hasNext) return null

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'flex-end',
        gap: 8,
        padding: '10px 16px',
        borderTop: '1px solid var(--border-faint)',
        fontSize: 13,
        color: 'var(--fg-muted)',
      }}
    >
      <button
        className="icon-btn"
        disabled={!hasPrev}
        onClick={() => hasPrev && onPrev()}
        aria-label="Previous page"
        style={{ opacity: hasPrev ? 1 : 0.35 }}
      >
        <I.chevronLeft size={14} />
      </button>
      <button
        className="icon-btn"
        disabled={!hasNext}
        onClick={() => hasNext && onNext()}
        aria-label="Next page"
        style={{ opacity: hasNext ? 1 : 0.35 }}
      >
        <I.chevronRight size={14} />
      </button>
    </div>
  )
}
