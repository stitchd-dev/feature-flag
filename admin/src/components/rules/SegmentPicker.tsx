import { useState, useEffect, useRef } from 'react'
import { api } from '../../lib/api'
import type { Segment } from '../../pages/segments/types'

interface Props {
  /** Currently selected segment ID (empty string = none selected). */
  value: string
  /** Called when the user picks a segment. Passes (segmentId, segmentName). */
  onChange: (segmentId: string, segmentName: string) => void
  /** Environment ID used to list segments. */
  envId: string | null
  /** Org ID used to build the segment detail link. */
  orgId: string
  /** When true the picker is read-only: show a coloured badge that links to the segment. */
  readOnly?: boolean
  /** Segment name to display in read-only mode (avoids re-fetching just for the name). */
  segmentName?: string
}

// ─── SegmentBadge (read-only chip that links to the segment detail page) ────

export function SegmentBadge({
  segmentId, segmentName, orgId,
}: {
  segmentId: string
  segmentName: string
  orgId: string
}) {
  const href = `/org/${orgId}/segments/${segmentId}`
  return (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      className="badge accent"
      style={{
        textDecoration: 'none',
        cursor: 'pointer',
        borderBottom: '1px dotted var(--accent)',
      }}
      title={`View segment: ${segmentName || segmentId}`}
    >
      {segmentName || segmentId}
    </a>
  )
}

// ─── SegmentPicker ────────────────────────────────────────────────────────────

export function SegmentPicker({ value, onChange, envId, orgId, readOnly, segmentName }: Props) {
  const [open, setOpen] = useState(false)
  const [segments, setSegments] = useState<Segment[]>([])
  const [loading, setLoading] = useState(false)
  const [search, setSearch] = useState('')
  const [error, setError] = useState<string | null>(null)
  const didFetch = useRef(false)
  const containerRef = useRef<HTMLDivElement>(null)

  // Lazy-load segments when dropdown first opens
  function handleOpen() {
    if (readOnly) return
    setOpen(true)
    if (!didFetch.current && envId) {
      didFetch.current = true
      setLoading(true)
      setError(null)
      api.get<Segment[]>(`/v1/segments?env_id=${envId}`)
        .then(({ data }) => setSegments(data))
        .catch(() => setError('Failed to load segments'))
        .finally(() => setLoading(false))
    }
  }

  // Close on click outside
  useEffect(() => {
    if (!open) return
    function onClickOutside(e: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onClickOutside)
    return () => document.removeEventListener('mousedown', onClickOutside)
  }, [open])

  const filtered = search
    ? segments.filter((s) => s.name.toLowerCase().includes(search.toLowerCase()))
    : segments

  // Resolve display name: prefer passed segmentName, else look up in loaded list
  const displayName = segmentName
    || segments.find((s) => s.id === value)?.name
    || (value || '')

  if (readOnly) {
    if (!value) return <span style={{ fontSize: 12, color: 'var(--fg-subtle)' }}>—</span>
    return <SegmentBadge segmentId={value} segmentName={displayName} orgId={orgId} />
  }

  return (
    <div ref={containerRef} style={{ position: 'relative', display: 'inline-block' }}>
      <button
        type="button"
        className="input"
        style={{
          width: 200,
          textAlign: 'left',
          fontFamily: 'var(--font-mono)',
          fontSize: 12,
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 4,
        }}
        onClick={handleOpen}
        disabled={!envId}
        title={envId ? undefined : 'Select an environment first'}
      >
        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', flex: 1 }}>
          {value ? (displayName || value) : <span style={{ color: 'var(--fg-subtle)' }}>Select segment…</span>}
        </span>
        <span style={{ fontSize: 10, color: 'var(--fg-subtle)', flexShrink: 0 }}>▾</span>
      </button>

      {open && (
        <div style={{
          position: 'absolute',
          top: '100%',
          left: 0,
          zIndex: 200,
          background: 'var(--surface)',
          border: '1px solid var(--border-strong)',
          borderRadius: 6,
          boxShadow: 'var(--shadow-md)',
          minWidth: 220,
          maxHeight: 260,
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
        }}>
          <div style={{ padding: '6px 8px', borderBottom: '1px solid var(--border-faint)' }}>
            <input
              autoFocus
              className="input"
              style={{ width: '100%', fontSize: 12 }}
              placeholder="Search segments…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>

          <div style={{ overflowY: 'auto', flex: 1 }}>
            {loading && (
              <div style={{ padding: '10px 12px', fontSize: 12, color: 'var(--fg-subtle)', fontStyle: 'italic' }}>
                Loading…
              </div>
            )}
            {error && (
              <div style={{ padding: '10px 12px', fontSize: 12, color: 'var(--danger)' }}>
                {error}
              </div>
            )}
            {!loading && !error && filtered.length === 0 && (
              <div style={{ padding: '10px 12px', fontSize: 12, color: 'var(--fg-subtle)' }}>
                {search ? 'No matching segments' : 'No segments found'}
              </div>
            )}
            {!loading && filtered.map((seg) => (
              <button
                key={seg.id}
                type="button"
                onClick={() => {
                  onChange(seg.id, seg.name)
                  setOpen(false)
                  setSearch('')
                }}
                style={{
                  display: 'block',
                  width: '100%',
                  textAlign: 'left',
                  padding: '7px 12px',
                  fontSize: 12,
                  fontFamily: 'var(--font-mono)',
                  background: seg.id === value ? 'var(--bg-sunken)' : 'transparent',
                  border: 'none',
                  borderBottom: '1px solid var(--border-faint)',
                  cursor: 'pointer',
                  color: 'var(--fg)',
                }}
                onMouseEnter={(e) => { (e.currentTarget as HTMLButtonElement).style.background = 'var(--surface-2)' }}
                onMouseLeave={(e) => { (e.currentTarget as HTMLButtonElement).style.background = seg.id === value ? 'var(--bg-sunken)' : 'transparent' }}
              >
                {seg.name}
                {seg.description && (
                  <span style={{ marginLeft: 6, fontSize: 11, color: 'var(--fg-subtle)', fontFamily: 'var(--font-sans)' }}>
                    — {seg.description}
                  </span>
                )}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
