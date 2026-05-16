import { I } from './icons'

interface PaginationProps {
  page: number
  perPage: number
  total: number
  onChange: (page: number) => void
}

export function Pagination({ page, perPage, total, onChange }: PaginationProps) {
  const totalPages = Math.max(1, Math.ceil(total / perPage))
  const isFirst = page <= 1
  const isLast = page >= totalPages

  if (totalPages <= 1 && total <= perPage) return null

  const from = Math.min((page - 1) * perPage + 1, total)
  const to = Math.min(page * perPage, total)

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
      <span>{from}–{to} of {total}</span>
      <button
        className="icon-btn"
        disabled={isFirst}
        onClick={() => !isFirst && onChange(page - 1)}
        aria-label="Previous page"
        style={{ opacity: isFirst ? 0.35 : 1 }}
      >
        <I.chevronLeft size={14} />
      </button>
      <span style={{ fontVariantNumeric: 'tabular-nums' }}>
        {page} / {totalPages}
      </span>
      <button
        className="icon-btn"
        disabled={isLast}
        onClick={() => !isLast && onChange(page + 1)}
        aria-label="Next page"
        style={{ opacity: isLast ? 0.35 : 1 }}
      >
        <I.chevronRight size={14} />
      </button>
    </div>
  )
}
